extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::process::elf_loader::{self, LoadError};
use crate::fs::vfs_read::{self, ReadError};
use crate::mm::vmm::AddressSpace;
use crate::process::Process;
use core::sync::atomic::Ordering;

const FRAME_R8_SLOT: usize = 7;

/// Environment handed to a process when the caller does not supply one
/// (shell `exec`, syscall 46). execve (syscall 67) passes its own
pub const DEFAULT_ENV: &[&str] = &["PATH=/bin", "HOME=/", "TERM=miku", "OS=MikuOS"];

/// Longest interpreter line we accept in a `#!` script
const SHEBANG_MAX: usize = 128;

#[derive(Debug)]
pub enum ExecError {
    FsNotMounted,
    FileNotFound,
    NotRegularFile,
    IoError,
    Load(LoadError),
    NoAddressSpace,
    SpawnFailed,
    ShebangInvalid,
}

impl ExecError {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FsNotMounted => "filesystem not mounted",
            Self::FileNotFound => "file not found",
            Self::NotRegularFile => "not a regular file",
            Self::IoError => "I/O error",
            Self::Load(e) => e.as_str(),
            Self::NoAddressSpace => "failed to create address space",
            Self::SpawnFailed => "failed to spawn process",
            Self::ShebangInvalid => "bad #! interpreter line",
        }
    }
}

impl From<ReadError> for ExecError {
    fn from(e: ReadError) -> Self {
        match e {
            ReadError::FsNotMounted => Self::FsNotMounted,
            ReadError::FileNotFound => Self::FileNotFound,
            ReadError::NotRegularFile => Self::NotRegularFile,
            ReadError::IoError => Self::IoError,
        }
    }
}

/// If `data` is a `#!` script, read the interpreter binary and rebuild argv
/// per POSIX: [interp, optional-arg, script-path, original argv[1..]].
/// Returns the ELF to load plus the owned argv to run it with. Plain ELF
/// data passes through untouched. One level only: an interpreter that is
/// itself a script is rejected
pub fn resolve_shebang(
    path: &str,
    data: Vec<u8>,
    args: &[&str],
) -> Result<(Vec<u8>, Vec<String>), ExecError> {
    if data.len() < 3 || &data[..2] != b"#!" {
        let mut argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        if argv.is_empty() {
            argv.push(path.to_string());
        }
        return Ok((data, argv));
    }

    let line_end = data[..data.len().min(SHEBANG_MAX)]
        .iter()
        .position(|&b| b == b'\n')
        .ok_or(ExecError::ShebangInvalid)?;
    let line = core::str::from_utf8(&data[2..line_end])
        .map_err(|_| ExecError::ShebangInvalid)?
        .trim();

    // "#!/bin/sh" or "#!/bin/sh -e": interpreter plus at most one argument
    // block (POSIX keeps everything after the interpreter as ONE argument)
    let (interp, opt_arg) = match line.split_once(char::is_whitespace) {
        Some((i, rest)) => (i.trim(), Some(rest.trim())),
        None => (line, None),
    };
    if interp.is_empty() || !interp.starts_with('/') {
        return Err(ExecError::ShebangInvalid);
    }

    let interp_data = vfs_read::read_file_strict(interp)?;
    if interp_data.len() >= 2 && &interp_data[..2] == b"#!" {
        return Err(ExecError::ShebangInvalid); // no nested scripts
    }

    let mut argv: Vec<String> = Vec::new();
    argv.push(interp.to_string());
    if let Some(a) = opt_arg {
        if !a.is_empty() {
            argv.push(a.to_string());
        }
    }
    argv.push(path.to_string());
    for a in args.iter().skip(1) {
        argv.push(a.to_string());
    }

    crate::serial_println!("[exec] shebang: '{}' -> {} (argc={})", path, interp, argv.len());
    Ok((interp_data, argv))
}

pub fn exec(path: &str, args: &[&str], envs: &[&str]) -> Result<u64, ExecError> {
    let file_data = vfs_read::read_file_strict(path)?;
    let (file_data, argv) = resolve_shebang(path, file_data, args)?;
    let arg_refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();

    let aspace = AddressSpace::new_user().ok_or(ExecError::NoAddressSpace)?;

    let read_file = |interp_path: &str| -> Option<Vec<u8>> {
        if interp_path.contains("ld-miku") || interp_path.contains("ld.so") {
            return Some(crate::process::ldso::LDSO_BYTES.to_vec());
        }
        vfs_read::read_file(interp_path)
    };

    let image = elf_loader::load(&file_data, &aspace, path, &arg_refs, envs, Some(&read_file))
        .map_err(ExecError::Load)?;

    crate::serial_println!(
        "[exec] loaded '{}': entry={:#x} sp={:#x} brk={:#x} bias={:#x} tls={:#x} interp={}",
        path, image.entry, image.stack_top, image.brk, image.load_bias,
        image.tls_base, image.has_interp,
    );

    let cr3 = aspace.into_raw();

    crate::mm::mmap::vma_set_brk(cr3, image.brk);

    let mut proc = Process::new_elf(image.entry, image.stack_top, AddressSpace::from_raw(cr3))
        .ok_or_else(|| {
            crate::mm::mmap::vma_cleanup(cr3);
            AddressSpace::from_raw(cr3).free_address_space_manual();
            ExecError::SpawnFailed
        })?;

    if image.tls_base != 0 {
        let rsp = proc.rsp.load(Ordering::Relaxed);
        unsafe {
            (rsp as *mut u64).add(FRAME_R8_SLOT).write(image.tls_base);
        }
        crate::serial_println!("[exec] TLS base={:#x} -> r8 in initial frame", image.tls_base);
    }

    let pid = proc.pid;
    crate::io::input::user_stdin::set_foreground(pid);
    crate::sched::add_user_process(proc);

    crate::serial_println!("[exec] spawned pid={} from '{}' argc={}", pid, path, arg_refs.len());
    Ok(pid)
}
