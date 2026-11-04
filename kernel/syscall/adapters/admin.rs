// The machine's own commands, from ring 3
//
// `nvidia`, `mount`, `mkfs.ext4`, `sv`, `gpt`, `blkstat`, the selftests are
// functions inside the kernel's command interpreter, because each needs ring 0:
// a PCI register, a block driver, a superblock, the service table.
//
// This is the channel: a caller hands over a command line and the kernel runs
// it as if typed at the console. `msh` uses it for any name it cannot find in
// /bin, and `mikudo` likewise, so from a terminal:
//
//      nvidia list                     anybody; it only looks
//      mikudo mount /dev/blk0 /mnt     changes the machine, so: a password
//
// # Who may
//
// The command decides, not this call. There is no capability check here beyond
// the two exclusions below, because the kernel's own dispatch has one
// (`shell::commands::gated_by`) that answers per command: reading what a GPU
// says about itself is not an administrative act, and writing a partition table
// wants `CAP_SYS_ADMIN`.
//
// What runs, runs on the caller's thread: its credential, its working
// directory, its terminal. So the gate reads the right identity, every VFS
// check is made as whoever asked, and the output lands where they are looking.
//
// # What this widens
//
// The unprivileged commands' parsers are reachable from any process. The
// privileged ones are not: the gate is checked before a command's arguments are
// parsed, so an ordinary caller reaches `split_whitespace` and a refusal. The
// string is bounded and screened for control characters first, and `mikudo`
// records who asked when it asks.
//
// # It should shrink
//
// The end state is Linux's: `mount` is a syscall, `mkfs` is a program opening
// `/dev/blk0` and writing a filesystem with `lib/extfs`, `nvidia` is a program
// asking `gpu_info`. It must not be the preferred way to add an administrative
// operation: nobody can review the arguments of a syscall you pass a sentence
// to.

use crate::syscall::errno::{err, EFAULT, EINVAL, EPERM};
use crate::syscall::usercopy::{current_cr3, current_pid, user_ptr_mapped};

/// The longest command line this takes: twice the console editor's 256, and a
/// bound rather than a buffer to grow
const MAX_CMD: u64 = 512;

// 104  kcmd(ptr, len) -> 0 / -errno
pub fn sys_kcmd(ptr: u64, len: u64) -> u64 {
    if len == 0 || len > MAX_CMD {
        return err(EINVAL);
    }

    let cr3 = current_cr3();
    let _ua = match user_ptr_mapped(cr3, ptr, len) {
        Some(g) => g,
        None => return err(EFAULT),
    };
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };

    // Copied out of user memory before anything looks at it twice: a string in
    // another address space can change between the check and the use
    let mut buf = [0u8; MAX_CMD as usize];
    buf[..bytes.len()].copy_from_slice(bytes);
    let text = match core::str::from_utf8(&buf[..bytes.len()]) {
        Ok(t) => t,
        Err(_) => return err(EINVAL),
    };

    // One line, and printable: a newline would let one call carry two commands
    if text.bytes().any(|b| b < 0x20 && b != b'\t') {
        return err(EINVAL);
    }

    let first = text.split_whitespace().next().unwrap_or("");
    if first.is_empty() {
        return err(EINVAL);
    }
    // Two commands are about the console's own session and mean nothing from a
    // process: `login` starts the greeter on the caller's terminal, and
    // `logout` gives up the console's authority
    if first == "login" || first == "logout" {
        return err(EPERM);
    }

    // Per call, on the log rather than the screen. Not behind `trace`: a
    // machine command run by a program is what somebody reading a journal
    // needs, and it is rare
    crate::serial_println!("[kcmd] pid={} ran '{}'", current_pid(), text);

    // It runs on this thread, with this process's credential and controlling
    // terminal, so `print!` lands on the terminal that asked and every VFS
    // check is made as the caller.
    //
    // Marked for the duration, so the console's "unknown command" message is
    // left to the caller, who has a better one and an exit status
    let me = current_pid();
    let prev = IN_KCMD.swap(me, core::sync::atomic::Ordering::Relaxed);
    let answered = crate::shell::commands::execute(text);
    IN_KCMD.store(prev, core::sync::atomic::Ordering::Relaxed);

    if answered { 0 } else { err(crate::syscall::errno::ENOENT) }
}

/// Which process is running a command through here, if any
///
/// A side channel because the one reader is four calls deep in a large match,
/// and a parameter would go through every arm.
///
/// A pid rather than a flag, so a long command run this way
/// (`nvidia gsp-rm-boot`) does not silence the console's own "unknown command"
/// meanwhile. The previous value is put back, because one can reach the other
/// through `run_external`
static IN_KCMD: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// Is the running task here because a program asked, rather than a keyboard?
pub fn in_kcmd() -> bool {
    let who = IN_KCMD.load(core::sync::atomic::Ordering::Relaxed);
    who != 0 && who == current_pid()
}
