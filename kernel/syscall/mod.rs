// MikuOS syscall surface.
//
// Layout:
//      errno      POSIX error codes + VfsError -> errno mapping
//      user_mem   user pointer validation, path copy-in, current cr3/pid
//      abi        userspace byte layouts (stat, dirent, statfs)
//      io         read, write, write_file
//      file       open, close, seek, fsize, dup/dup2, truncate, pipe,
//                 fallocate, fsync, punch_hole
//      fs         stat, fstat, mkdir/rmdir/unlink, readdir, rename, link,
//                 chmod/chown, symlink/readlink, chdir, statfs, xattr,
//                 utimensat
//      memory     mmap, munmap, mprotect, brk, set_tls, get_tls, map_lib, getcwd
//      process    exit, sleep, uptime, fork, wait4, kill, exec
//
// init programs the SYSCALL/SYSRET MSRs and dispatch is the C-ABI entry point
// invoked by the naked syscall_handler. The on-the-wire syscall numbers are
// documented next to dispatch

mod abi;
pub mod errno;
pub mod usercopy;
pub mod adapters;
pub mod selftest;
use crate::syscall::adapters::{admin, file, fs, gpu, io, mayland, memory, net, notify, process, thread, tty};

use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask, Star};
use x86_64::registers::rflags::RFlags;
use x86_64::VirtAddr;

use errno::{err, ENOSYS};
use crate::arch::x86_64::gdt;

/// The highest syscall number this kernel answers, plus one
const SYSCALL_TABLE_SIZE: u32 = 119;

// public entry point

pub fn init() {
    unsafe {
        Efer::update(|f| *f |= EferFlags::SYSTEM_CALL_EXTENSIONS | EferFlags::NO_EXECUTE_ENABLE);
    }
    Star::write(
        gdt::user_code_selector(),
        gdt::user_data_selector(),
        gdt::kernel_code_selector(),
        gdt::kernel_data_selector(),
    ).unwrap();
    LStar::write(VirtAddr::new(syscall_handler as *const () as u64));
    // Cleared on entry to every syscall:
    //   IF: the handler runs with interrupts off until it chooses otherwise
    //   AC: SMAP's override bit. Userspace can set it with popfq, and if it
    //       survived into ring 0 SMAP would be disabled for the syscall
    //   DF: string ops must start forward; a user-set DF would reverse any rep
    //       movs the kernel emits
    SFMask::write(
        RFlags::INTERRUPT_FLAG
            | RFlags::ALIGNMENT_CHECK
            | RFlags::DIRECTION_FLAG,
    );
    crate::serial_println!(
        "[syscall] MikuOS syscall table ready ({} entries)",
        SYSCALL_TABLE_SIZE
    );
}

// naked SYSCALL/SYSRET bridge

#[unsafe(naked)]
unsafe extern "C" fn syscall_handler() {
    core::arch::naked_asm!(
        "swapgs",
        "mov gs:[0x18], rsp",
        "mov rsp, gs:[0x10]",
        // `sigreturn` leaves by `iretq` rather than `sysretq`, which would
        // overwrite the RCX and R11 a handler is handing back. Nothing of
        // this entry is kept: the frame replaces the whole context
        "cmp rax, 69",
        "je {sigreturn}",
        "push rcx",
        "push r11",
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "push r10",
        "push r9",
        "push r8",
        // And the three argument registers, which a `syscall` keeps on Linux
        // (every register but RAX, RCX and R11). `ld-miku`, the vDSO and
        // `lib/miku` are written to that rule, and a signal delivered at the
        // end of a syscall records them. Below the eleven the rest of the
        // kernel addresses from the top, so none of those moves
        "push rdi",
        "push rsi",
        "push rdx",
        "mov r8,  r10",
        "mov rcx, rdx",
        "mov rdx, rsi",
        "mov rsi, rdi",
        "mov rdi, rax",
        // Fourteen pushes from a 16-aligned top leave rsp 16-aligned for the
        // call, as SysV asks: `fxsave` into a 16-aligned local of `sys_fork`
        // faults otherwise
        "call {handler}",
        // The handler can come back with interrupts on: a task that blocked in
        // a syscall is resumed by `iretq` with IF set. From
        // `mov rsp, gs:[0x18]` to `sysretq` this CPU is in ring 0 on the user
        // stack with the user GS, and an interrupt there (same privilege, so no
        // stack switch) pushes its frame onto the user stack: SMAP faults the
        // push, and the kernel dies at `sysretq`. `sysretq` loads RFLAGS from
        // r11, so the user's IF comes back regardless
        "cli",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop r8",
        "pop r9",
        "pop r10",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "pop r11",
        "pop rcx",
        "mov rsp, gs:[0x18]",
        "swapgs",
        "sysretq",
        handler = sym dispatch,
        sigreturn = sym sigreturn_entry,
    );
}

/// `sigreturn`(69), on the task's kernel stack with the user's RSP in
/// `gs:[0x18]` and RDI the frame. Builds a `TrapFrame` from the frame and
/// leaves through it
#[unsafe(naked)]
unsafe extern "C" fn sigreturn_entry() {
    core::arch::naked_asm!(
        "sub rsp, {size}",
        "mov rsi, rsp",
        "call {restore}",
        // Interrupts off from here to `iretq`, for the reason the `cli` in
        // `syscall_handler` gives
        "cli",
        "pop rax",
        "pop rbx",
        "pop rcx",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop rbp",
        "pop r8",
        "pop r9",
        "pop r10",
        "pop r11",
        "pop r12",
        "pop r13",
        "pop r14",
        "pop r15",
        "add rsp, 16",
        "swapgs",
        "iretq",
        size = const crate::arch::x86_64::trap::TRAP_FRAME_SIZE,
        restore = sym crate::process::signal::sigreturn_full,
    );
}

//                  syscall numbers
//
//  0  exit           16  sleep           32  symlink
//  1  write          17  uptime          33  readlink
//  2  read           18  stat            34  pipe
//  3  mmap           19  fstat           35  chdir
//  4  munmap         20  mkdir           36  statfs
//  5  mprotect       21  rmdir           37  fallocate
//  6  brk            22  unlink          38  getxattr
//  7  getpid         23  readdir         39  setxattr
//  8  getcwd         24  rename          40  utimensat
//  9  set_tls        25  link            41  fsync
// 10  get_tls        26  chmod           42  punch_hole
// 11  open           27  chown           43  fork
// 12  close          28  dup             44  wait4
// 13  seek           29  dup2            45  kill
// 14  fsize          30  truncate        46  exec
// 15  map_lib        31  write_file
//
// 47 umask  48 getuid  49 getgid  50 geteuid  51 getegid
// 52 setuid 53 setgid  54 seteuid 55 setegid
// 83 peercred 84 sendfd 85 recvfd
// 86 setresuid 87 setresgid 88 setgroups 89 getgroups
// 90 capget 91 capset 92 session
// 56 socket 57 connect 58 send    59 recv
// 72 may_pool  73 may_unpool  74 may_map  75 klog  76 poll  77 socketpair
// 78 getrandom 79 clock_settime 80 setsockopt
// 122 sleep_ns 123 poll_ns
// 81 listxattr 82 removexattr 99 tty

/// `ipc`: the state of the descriptor layer, and its selftest
///
/// Its own command rather than a corner of `net`: it covers descriptors,
/// readiness and waiting
pub fn cmd_ipc(args: &str) {
    let mut parts = args.trim().split_whitespace();
    match parts.next().unwrap_or("status") {
        "test" => {
            crate::cprintln!(57, 197, 187, "ipc: selftest");
            let report = selftest::run();
            for c in &report.cases {
                if c.skipped {
                    crate::cprintln!(220, 220, 100, "  skip  {}", c.name);
                } else if c.ok {
                    crate::cprintln!(100, 220, 150, "  ok    {}", c.name);
                } else {
                    crate::print_error!("  FAIL  {}", c.name);
                }
                crate::println!("          {}", c.detail);
            }
            crate::println!("{} passed, {} failed, {} skipped",
                report.passed(), report.failed(), report.skipped());
        }
        "status" | "" => {
            use crate::fs::vfs::types::{MAX_OPEN_FILES, RESERVED_STDIO_FDS};
            crate::println!("descriptors");
            crate::println!("  vfs      0..{} ({} reserved for stdio)",
                MAX_OPEN_FILES - 1, RESERVED_STDIO_FDS);
            crate::println!("  sockets  {}..{} ({} in use)",
                crate::net::socket::SOCK_FD_BASE,
                crate::net::socket::SOCK_FD_BASE + crate::net::socket::MAX_SOCKETS - 1,
                crate::net::socket::in_use());
            crate::println!("waiting");
            crate::println!("  poll     up to {} descriptors per call", 64);
        }
        other => crate::println!("ipc status|test (not '{}')", other),
    }
}

extern "C" fn dispatch(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let ret = dispatch_inner(nr, a1, a2, a3, a4);
    // Killed while in here (its group's `exit`, a fatal signal): it does not
    // get back to ring 3 to act on what the syscall answered
    crate::sched::stop_if_dead();
    // Signal delivery point: every syscall return passes through here.
    // May rewrite the sysret RIP/RSP to run a userspace signal handler
    crate::process::signal::deliver_pending(ret)
}

fn dispatch_inner(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    match nr {
        0  => process::sys_exit(a1),
        1  => io::sys_write(a1, a2, a3),
        2  => io::sys_read(a1, a2, a3),
        3  => memory::sys_mmap(a1, a2, a3, a4),
        4  => memory::sys_munmap(a1, a2),
        5  => memory::sys_mprotect(a1, a2, a3),
        6  => memory::sys_brk(a1),
        // The process, not the task: `kill(getpid(), ..)`, `setsid` and
        // `std::process::id()` name the process from any thread. `gettid` (108)
        // is the thread's own id
        7  => crate::sched::current_tgid(),
        8  => memory::sys_getcwd(a1, a2),
        9  => memory::sys_set_tls(a1),
        10 => memory::sys_get_tls(),
        11 => file::sys_open(a1, a2, a3, a4),
        12 => file::sys_close(a1),
        13 => file::sys_seek(a1, a2, a3),
        14 => file::sys_fsize(a1),
        15 => memory::sys_map_lib(a1, a2),
        16 => process::sys_sleep(a1),
        17 => process::sys_uptime(),
        18 => fs::sys_stat(a1, a2, a3),
        19 => fs::sys_fstat(a1, a2),
        20 => fs::sys_mkdir(a1, a2, a3),
        21 => fs::sys_rmdir(a1, a2),
        22 => fs::sys_unlink(a1, a2),
        23 => fs::sys_readdir(a1, a2, a3, a4),
        24 => fs::sys_rename(a1, a2, a3, a4),
        25 => fs::sys_link(a1, a2, a3, a4),
        26 => fs::sys_chmod(a1, a2, a3),
        27 => fs::sys_chown(a1, a2, a3, a4),
        28 => file::sys_dup(a1),
        29 => file::sys_dup2(a1, a2),
        30 => file::sys_truncate(a1, a2),
        31 => io::sys_write_file(a1, a2, a3),
        32 => fs::sys_symlink(a1, a2, a3, a4),
        33 => fs::sys_readlink(a1, a2, a3, a4),
        34 => file::sys_pipe(a1),
        35 => fs::sys_chdir(a1, a2),
        36 => fs::sys_statfs(a1, a2, a3),
        37 => file::sys_fallocate(a1, a2, a3),
        38 => fs::sys_getxattr(a1, a2, a3, a4),
        39 => fs::sys_setxattr(a1, a2, a3, a4),
        40 => fs::sys_utimensat(a1, a2, a3, a4),
        41 => file::sys_fsync(a1),
        42 => file::sys_punch_hole(a1, a2, a3),
        43 => process::sys_fork(),
        44 => process::sys_wait4(a1, a2, a3),
        45 => process::sys_kill(a1, a2),
        46 => process::sys_exec(a1, a2, a3, a4),
        47 => process::sys_umask(a1),
        48 => process::sys_getuid(),
        49 => process::sys_getgid(),
        50 => process::sys_geteuid(),
        51 => process::sys_getegid(),
        52 => process::sys_setuid(a1),
        53 => process::sys_setgid(a1),
        54 => process::sys_seteuid(a1),
        55 => process::sys_setegid(a1),
        56 => net::sys_socket(a1, a2, a3),
        57 => net::sys_connect(a1, a2, a3),
        58 => net::sys_send(a1, a2, a3, a4),
        59 => net::sys_recv(a1, a2, a3, a4),
        // 60  mmap_file(args_ptr): file-backed mmap; an args struct in user memory carries the six fields that do not fit the 4-arg ABI
        60 => memory::sys_mmap_file(a1),
        61 => memory::sys_msync(a1, a2),
        62 => net::sys_bind(a1, a2, a3),
        63 => net::sys_listen(a1, a2),
        64 => net::sys_accept(a1, a2, a3),
        65 => net::sys_sendto(a1, a2, a3, a4),
        66 => net::sys_recvfrom(a1, a2, a3, a4),
        // 67  execve(args_ptr): exec with caller-supplied environment; an args struct in user memory carries the six fields that do not fit the 4-arg ABI (see sys_execve)
        67 => process::sys_execve(a1),
        // 68/69: userspace signal dispatch (see process::signal)
        68 => crate::process::signal::sys_sigentry(a1),
        // 69, sigreturn, never gets here: `syscall_handler` sends it to
        // `sigreturn_entry` before anything is saved
        70 => process::sys_clock_gettime(a1, a2),
        71 => io::sys_ioctl(a1, a2, a3),
        // 72/73: Mayland shared-memory pools. A pool is the protocol's
        // buffer-passing primitive; see syscall/adapters/mayland.rs
        72 => mayland::sys_may_pool(a1, a2),
        73 => mayland::sys_may_unpool(a1),
        74 => mayland::sys_may_map(a1, a2),
        // 75  klog(ptr, len): a line for the kernel's log and not for the screen: the loader's relocation trace, a service's progress
        75 => io::sys_klog(a1, a2),
        // 76  poll(fds, nfds, timeout_ms): waiting on more than one thing at a time, so a program can be a server without a thread per client
        76 => io::sys_poll(a1, a2, a3),
        // 77  socketpair(fds_ptr): a connected pair of local sockets, a channel needing no name
        77 => net::sys_socketpair(a1),
        // 78  getrandom(buf, len): unpredictable bytes from the hardware. The userspace PRNG is seeded from uptime and pid, which an attacker can guess
        78 => process::sys_getrandom(a1, a2),
        // 79  clock_settime(clock_id, timespec): root only. The counterpart to 70, so the program correcting the clock can be a program
        79 => process::sys_clock_settime(a1, a2),
        // 80  setsockopt(fd, level, optname, value): IP_TTL, which traceroute needs, and the keepalive options
        80 => net::sys_setsockopt(a1, a2, a3, a4),

        // 81 listxattr(ino, buf, len), 82 removexattr(ino, name, name_len)
        81 => fs::sys_listxattr(a1, a2, a3),
        82 => fs::sys_removexattr(a1, a2, a3),

        // 83-85: what a privileged service in ring 3 needs. peercred answers
        // who is asking: the kernel wrote it down before the server saw the
        // connection. sendfd/recvfd carry a descriptor across, so an elevated
        // command inherits the user's terminal, pipe or redirection
        83 => net::sys_peercred(a1, a2),
        84 => net::sys_sendfd(a1, a2),
        85 => net::sys_recvfd(a1),

        // 86-91: the rest of the credential model. 52-55 move one id at a time,
        // not enough for a broker changing its whole identity without being a
        // half-built one
        86 => process::sys_setresuid(a1, a2, a3),
        87 => process::sys_setresgid(a1, a2, a3),
        88 => process::sys_setgroups(a1, a2),
        89 => process::sys_getgroups(a1, a2),
        90 => process::sys_capget(a1),
        91 => process::sys_capset(a1, a2, a3),
        // 92  session(op, arg, ptr, len): who is logged in, on which seat. A console session's identity is the shell thread's credential, which every VFS check reads. A graphical session's identity is on its leader process, and the seat is what the two have in common (see kernel/process/session.rs)
        92 => process::sys_session(a1, a2, a3, a4),
        // 93  power(action): stops the machine, or stops it and starts it again. Gated on CAP_SYS_BOOT, which a session holds and an ordinary program does not. A syscall because there is nothing to run: powering off is mikuD stopping its services and the kernel writing a register
        93 => process::sys_power(a1),
        // 94-97: the GPU, from ring 3. A pool handle names the memory, the
        // kernel resolves every address the engine touches, and a fence carries
        // the channel's epoch so one held across a teardown cannot read as
        // passed. See adapters/gpu.rs
        94 => gpu::sys_gpu_info(a1, a2),
        95 => gpu::sys_gpu_submit(a1),
        96 => gpu::sys_gpu_bind(a1, a2, a3),
        97 => gpu::sys_gpu_wait(a1, a2),
        98 => gpu::sys_gpu_program(a1),
        121 => gpu::sys_gpu_alloc(a1, a2, a3),
        // 99  tty(op, arg): makes a terminal, joins one, hangs one up, asks which one you are on. See adapters/tty.rs and dev/terminal-architecture.md
        99 => tty::sys_tty(a1, a2),

        // 100..102  job control: what a signal means, and who it reaches
        //
        // One mechanism: a shell hands a terminal to a group so Ctrl-C reaches
        // every process in a pipeline; a group is made with setpgid; and a
        // service shutting down cleanly on SIGTERM says so with a disposition.
        // See dev/terminal-architecture.md
        100 => crate::process::signal::sys_sigdisp(a1, a2),
        101 => process::sys_setpgid(a1, a2),
        102 => process::sys_getpgid(a1),

        // 103  the process table, so `ps` can be a program. See adapters/process.rs
        103 => process::sys_procinfo(a1, a2),

        // 104  the machine's own commands, for a caller that may administer it. See adapters/admin.rs
        104 => admin::sys_kcmd(a1, a2),

        // 105  which program a process is running, one pid at a time. `procinfo` carries the short name; this is the path
        105 => process::sys_proc_path(a1, a2, a3),

        // 106..110  threads, and the primitive they are built on.
        //
        // One mechanism: `std::thread` needs a task that is not a process, and
        // `std::sync::{Mutex, Condvar, RwLock}` need a way to sleep on a word,
        // and upstream's futex implementations compile unchanged. See
        // dev/std-architecture.md and adapters/thread.rs
        106 => thread::sys_thread_create(a1),
        107 => thread::sys_thread_exit(a1),
        108 => thread::sys_gettid(),
        109 => thread::sys_sched_yield(),
        110 => thread::sys_futex(a1, a2, a3, a4),

        // 111  getdents(fd, buf, max_entries, offset): a directory read with a cursor, unlike `readdir`(23). See adapters/fs.rs
        111 => fs::sys_getdents(a1, a2, a3, a4),

        // 112  lstat: `stat`(18) follows a symlink, and a recursive delete has to ask whether each entry is itself a link
        112 => fs::sys_lstat(a1, a2, a3),

        // 113  fcntl: FD_CLOEXEC after the fact, and `O_NONBLOCK` at runtime, as every reactor does. `sys_read` reads `O_NONBLOCK`
        113 => file::sys_fcntl(a1, a2, a3),
        // Reading a socket option, above all `SO_ERROR`, how a non-blocking
        // connect's outcome reaches the program
        114 => net::sys_getsockopt(a1, a2, a3),
        // sigaltstack(new *stack_t, old *stack_t): the task's alternate signal
        // stack, where a handler installed with SA_ONSTACK runs
        115 => crate::process::signal::sys_sigaltstack(a1, a2),
        // inotify, Linux's three calls: see dev/fs-architecture.md, "Watching"
        116 => notify::sys_inotify_init1(a1),
        117 => notify::sys_inotify_add_watch(a1, a2, a3, a4),
        118 => notify::sys_inotify_rm_watch(a1, a2),
        // realpath(path, path_word, buf, size): what a lookup resolved to,
        // named from the root; an empty path with a descriptor base names the
        // open file itself
        119 => fs::sys_realpath(a1, a2, a3, a4),
        120 => file::sys_pipe2(a1, a2),
        // 122  sleep_ns(ns): a sleep measured in nanoseconds (`sched::timer`); `sleep`(16) counts 4 ms ticks
        122 => process::sys_sleep_ns(a1),
        // 123  poll_ns(fds, nfds, timeout_ns): `poll`(76) with a timeout in nanoseconds
        123 => io::sys_poll_ns(a1, a2, a3),
        _  => {
            crate::serial_println!("[syscall] unknown nr={}", nr);
            err(ENOSYS)
        }
    }
}
