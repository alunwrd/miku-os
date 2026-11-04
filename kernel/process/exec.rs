extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::process::elf_loader::{self, LoadError};
use crate::process::elf_source::{ElfSource, FileSource};
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

/// The file an exec loads: open and read by range (`elf_source`), or, for a
/// file the VFS cannot open, read whole by `vfs_read`'s ext fallback with its
/// 64 MiB ceiling. Dropped as soon as the load is done: the open file is a
/// descriptor in the caller's table
pub enum Program {
    File(FileSource),
    Bytes(Vec<u8>),
}

impl Program {
    pub fn open(path: &str) -> Result<Self, ExecError> {
        if let Some(f) = FileSource::open(path) {
            return Ok(Program::File(f));
        }
        Ok(Program::Bytes(vfs_read::read_file_strict(path)?))
    }

    /// The first `n` bytes, or as many as there are
    fn head(&self, n: usize) -> Vec<u8> {
        let mut v = alloc::vec![0u8; n.min(self.len())];
        let got = self.read_at(0, &mut v);
        v.truncate(got);
        v
    }
}

impl ElfSource for Program {
    fn len(&self) -> usize {
        match self {
            Program::File(f) => f.len(),
            Program::Bytes(b) => b.len(),
        }
    }

    fn read_at(&self, off: usize, out: &mut [u8]) -> usize {
        match self {
            Program::File(f) => f.read_at(off, out),
            Program::Bytes(b) => b.read_at(off, out),
        }
    }
}

/// The dynamic linker the loader asks for by `PT_INTERP`. `ld-miku` is part
/// of the kernel image and is lent, not copied; anything else is read whole,
/// which an interpreter is small enough for
pub fn read_interp(path: &str) -> Option<alloc::borrow::Cow<'static, [u8]>> {
    if path.contains("ld-miku") || path.contains("ld.so") {
        return Some(alloc::borrow::Cow::Borrowed(crate::process::ldso::LDSO_BYTES));
    }
    vfs_read::read_file(path).map(alloc::borrow::Cow::Owned)
}

/// If `prog` is a `#!` script, open the interpreter binary and rebuild argv
/// per POSIX: [interp, optional-arg, script-path, original argv[1..]].
/// Returns the program to load plus the owned argv to run it with. An ELF
/// passes through untouched. One level only: an interpreter that is itself a
/// script is rejected
pub fn resolve_shebang(
    path: &str,
    prog: Program,
    args: &[&str],
) -> Result<(Program, Vec<String>), ExecError> {
    let head = prog.head(SHEBANG_MAX);
    if head.len() < 3 || &head[..2] != b"#!" {
        let mut argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        if argv.is_empty() {
            argv.push(path.to_string());
        }
        return Ok((prog, argv));
    }

    let line_end = head
        .iter()
        .position(|&b| b == b'\n')
        .ok_or(ExecError::ShebangInvalid)?;
    let line = core::str::from_utf8(&head[2..line_end])
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

    let interp_prog = Program::open(interp)?;
    if interp_prog.head(2) == b"#!" {
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

    crate::trace_println!("[exec] shebang: '{}' -> {} (argc={})", path, interp, argv.len());
    Ok((interp_prog, argv))
}

/// Files the loaded image under its pid, so a crash report can turn a faulting
/// address back into `function+offset`. Both exec paths, the shell's and
/// execve's, go through this
pub fn register_image(pid: u64, path: &str, image: &elf_loader::ElfImage) {
    crate::process::image::register(pid, crate::process::image::ImageInfo {
        path:         path.to_string(),
        load_bias:    image.load_bias,
        interp_base:  image.interp_base,
        exe_lo:       image.exe_lo,
        exe_hi:       image.exe_hi,
        entry:        image.exe_entry,
        build_id:     image.build_id,
        build_id_len: image.build_id_len,
        x86_features: image.x86_features,
    });
}

/// Starts a program as root, for mikuD's services and the machine-level shell
/// commands
pub fn exec(path: &str, args: &[&str], envs: &[&str]) -> Result<u64, ExecError> {
    exec_as(path, args, envs, crate::process::cred::Cred::root())
}

/// Starts a program with a chosen identity.
///
/// `Process::new_elf` produces `Cred::root()`; a program the user asked for
/// should be the user. The credential is applied before the process is handed
/// to the scheduler, so it has not executed an instruction as anything else
pub fn exec_as(
    path: &str,
    args: &[&str],
    envs: &[&str],
    cred: crate::process::cred::Cred,
) -> Result<u64, ExecError> {
    let prog = Program::open(path)?;
    let (prog, argv) = resolve_shebang(path, prog, args)?;
    let arg_refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();

    let aspace = AddressSpace::new_user().ok_or(ExecError::NoAddressSpace)?;

    let image = elf_loader::load(&prog, &aspace, path, &arg_refs, envs, Some(&read_interp), cred);
    // Closed before anything can inherit the caller's descriptor table
    drop(prog);
    let image = image.map_err(ExecError::Load)?;

    crate::trace_println!(
        "[exec] loaded '{}': entry={:#x} sp={:#x} brk={:#x} bias={:#x} tls={:#x} interp={}",
        path, image.entry, image.stack_top, image.brk, image.load_bias,
        image.tls_base, image.has_interp,
    );

    let cr3 = aspace.into_raw();

    crate::mm::mmap::vma_set_brk(cr3, image.brk);

    // new_elf takes ownership of the address space and gives it back by
    // freeing it on failure, so the only thing left to undo here is the
    // VMA table keyed on this cr3
    let mut proc = match Process::new_elf(image.entry, image.stack_top, AddressSpace::from_raw(cr3)) {
        Some(p) => p,
        None    => {
            crate::mm::mmap::vma_cleanup(cr3);
            return Err(ExecError::SpawnFailed);
        }
    };

    if image.tls_base != 0 {
        let rsp = proc.rsp.load(Ordering::Relaxed);
        unsafe {
            (rsp as *mut u64).add(FRAME_R8_SLOT).write(image.tls_base);
        }
        crate::trace_println!("[exec] TLS base={:#x} -> r8 in initial frame", image.tls_base);
    }

    let pid = proc.pid;
    // Before add_user_process: once the scheduler has it, it can run
    *proc.cred.write() = cred;
    // What it is called, while the box is still exclusively ours. `new_elf`
    // names every process `user-elf`
    proc.name = crate::process::Name::from_path(path);
    register_image(pid, path, &image);
    // Not set_foreground here: who owns the keyboard is the caller's business.
    // A shell starting a program the user waits on wants it, and a service
    // manager starting a daemon does not. `crate::shell::commands::cmd_exec`
    // takes it and gives it back around the wait
    crate::sched::add_user_process(proc);

    // One line per program started, in the log and not on the wire: at 115200
    // baud it would be half of a spawn
    crate::kdebug!("[exec] spawned pid={} from '{}' argc={}", pid, path, arg_refs.len());
    Ok(pid)
}
