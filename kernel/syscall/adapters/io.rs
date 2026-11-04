// Stream I/O syscalls: read, write, write_file, poll

use crate::syscall::errno::{EAGAIN, err, vfs_err, EFAULT, EINTR, EINVAL, EIO, ENOTTY, ENXIO, EPERM, EPIPE};
use crate::syscall::usercopy::{current_cr3, current_pid, user_ptr_mapped, user_ptr_writable};

// The most one read or write moves. A larger request moves this much and says
// so, a short count every caller of read and write loops on. Linux caps the
// same way, at 0x7ffff000
const MAX_IO_LEN: u64 = 1 << 20;

/// What a descriptor is
///
/// A descriptor number means one of these kinds, decided by one rule: sockets
/// by their fd range, everything else by what the VFS table holds, with 0, 1
/// and 2 falling back to the terminal only when nothing is redirected onto
/// them.
///
/// `read`, `write` and `poll` share it: a `poll` disagreeing with `read`
/// reports a descriptor ready and the read goes somewhere else and blocks
#[derive(Clone, Copy, PartialEq)]
pub enum FdKind {
    /// fd 0 with nothing redirected onto it: the keyboard
    Stdin,
    /// fd 1 or 2 with nothing redirected onto it: the console
    Console,
    /// A pipe, and the vnode that holds its buffer
    Pipe(usize),
    /// A socket: network, local, or the compositor
    Socket,
    /// /dev/net0: raw ethernet frames. Apart from `Vfs` because an ordinary
    /// file is always ready and this is not: a program acting as the network
    /// stack waits on the card and its clients in the same `poll`, and a device
    /// always claiming readable turns that wait into a spin
    NetRaw,
    /// A terminal named by a descriptor: `/dev/tty`, the caller's own, or
    /// `/dev/console`, tty0 whoever is asking
    ///
    /// Apart from `Vfs`: a read on it waits, which must not happen inside
    /// `with_vfs` under the global filesystem lock, and an `ioctl` on it is a
    /// terminal ioctl, which is how a program handed a pipe for stdin still
    /// sets its terminal up.
    ///
    /// `TtyId(0)` means the caller has no controlling terminal, the one thing
    /// `/dev/tty` cannot resolve to
    Tty(crate::io::tty::TtyId),
    /// The master end of a pseudo-terminal
    ///
    /// Reading takes what the program on the other side wrote; writing is that
    /// program's input, by the path a keystroke takes. The `TtyId` is looked up
    /// from the node's minor on every call, so a descriptor outliving its
    /// terminal is caught by the generation
    PtyMaster(crate::io::tty::TtyId),
    /// An inotify instance, and the anonymous vnode it is keyed by. Apart from
    /// `Vfs` because reading it waits for an event, outside the VFS lock
    Inotify(usize),
    /// An ordinary open VFS descriptor
    Vfs,
    /// Nothing is open here
    Closed,
}

/// Decides what `fd` is, from the descriptor table alone (`dev/vfs-locking.md`,
/// "4a"): a descriptor's route is worked out under the VFS lock the first time
/// it is asked for, and kept on the descriptor. Every `read` and `write` asks
/// this first
pub fn classify(fd: u64) -> FdKind {
    use crate::fs::vfs::fd::Route;
    // Socket fds live above the VFS range and never appear in its table, so
    // this needs no VFS lock
    if crate::net::socket::is_socket_fd(fd) {
        return FdKind::Socket;
    }

    let (vid, route) = match crate::fs::vfs::core::route(fd as usize) {
        // An entry in the table wins over the terminal: `cmd > file` and
        // `cmd < file` install a real descriptor at 0/1/2
        Some(seen) => seen,
        None if fd == 0            => return FdKind::Stdin,
        None if fd == 1 || fd == 2 => return FdKind::Console,
        None                       => return FdKind::Closed,
    };
    match route {
        Route::Pipe => FdKind::Pipe(vid),
        Route::Inotify => FdKind::Inotify(vid),
        Route::NetRaw => FdKind::NetRaw,
        // Resolved on every call, not at open: a process moving to another
        // terminal moves its `/dev/tty` with it
        Route::Tty => FdKind::Tty(
            crate::io::input::user_stdin::ctty().unwrap_or(crate::io::tty::TtyId(0)),
        ),
        Route::Console => FdKind::Tty(crate::io::tty::SYSTEM),
        // The slave end of a pty: a terminal descriptor like `/dev/tty`'s,
        // naming that terminal. Gone is id 0, which every terminal path answers
        // as "no terminal"
        Route::PtySlave(slot) => FdKind::Tty(
            crate::io::tty::master_tty(slot as usize).unwrap_or(crate::io::tty::TtyId(0)),
        ),
        // The terminal has gone but the descriptor has not: still not a VFS
        // file. Reads answer EOF rather than whatever devfs would say
        Route::PtyMaster(slot) => FdKind::PtyMaster(
            crate::io::tty::master_tty(slot as usize).unwrap_or(crate::io::tty::TtyId(0)),
        ),
        Route::Vfs { .. } | Route::Unknown => FdKind::Vfs,
    }
}

// 1  write(fd, buf, len) -> bytes_written
pub fn sys_write(fd: u64, ptr: u64, len: u64) -> u64 {
    if len == 0 { return 0; }
    let len = len.min(MAX_IO_LEN);

    let cr3 = current_cr3();
    // read-side: just needs to be readable from userspace
    let _ua = match user_ptr_mapped(cr3, ptr, len) { Some(g) => g, None => return err(EFAULT) };

    // Asked once, before the write: afterwards a woken reader could have
    // consumed the bytes and closed the pipe
    let kind = classify(fd);

    // An inotify descriptor is read-only by nature; Linux says EINVAL
    if matches!(kind, FdKind::Inotify(_)) {
        return err(EINVAL);
    }

    if kind == FdKind::Console {
        let s = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        match core::str::from_utf8(s) {
            Ok(t)  => crate::print!("{}", t),
            Err(_) => for &b in s { crate::print!("{}", b as char); },
        }
        return len;
    }

    // A descriptor naming a terminal writes to that terminal and no other.
    // `print!` routes by the writer's own terminal, so `/dev/console` does not
    // go through it
    if let FdKind::Tty(id) = kind {
        if !crate::io::tty::exists(id) { return err(ENXIO); }
        let s = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        let ok = match core::str::from_utf8(s) {
            Ok(t) => crate::io::tty::write_text(id, t),
            // Not valid UTF-8, so not text: Latin-1 per byte, as the console
            // writer does
            Err(_) => {
                let mut ok = true;
                let mut tmp = [0u8; 4];
                for &b in s {
                    ok &= crate::io::tty::write_text(id, (b as char).encode_utf8(&mut tmp));
                }
                ok
            }
        };
        return if ok { len } else { err(EIO) };
    }

    // Writing a master is the slave's input, by the path a keystroke takes: the
    // line discipline, which echoes it, assembles a line and turns a Ctrl-C
    // into a signal
    if let FdKind::PtyMaster(id) = kind {
        if !crate::io::tty::exists(id) { return err(EIO); }
        let s = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        crate::io::tty::feed_bytes(id, s);
        return len;
    }

    // Socket fds route to the socket send path (write == send, no flags)
    if kind == FdKind::Socket {
        return super::net::sys_send(fd, ptr, len, 0);
    }

    let buf = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    // A pipe is the one destination that can be full, and the one that can
    // have nobody at the other end
    if let FdKind::Pipe(vid) = kind {
        return pipe_write(fd, vid, buf);
    }
    let r = crate::fs::vfs::core::with_vfs(|vfs| match vfs.write(fd as usize, buf) {
        Ok(n)  => n as u64,
        Err(e) => vfs_err(e),
    });
    // Over the dirty limit, this writer writes some out before it goes on
    crate::fs::vfs::core::writeback::balance();
    r
}

/// Sleep on a pipe: its own queue, and the one a hangup with no key wakes
/// (an exit, `release_fds_on_exit`). The request is made before the pipe is
/// looked at again (`ready`), so a write or a close in between is not lost
fn pipe_wait(vid: usize, ready: impl FnOnce() -> bool) {
    use crate::sched::wait::{keyed, PIPE};
    let deadline = crate::sched::tick_deadline(PIPE_BACKSTOP);
    let q = keyed(crate::fs::vfs::pipe::wait_key(vid));
    crate::sched::wait::wait_any_unless(&[q, &PIPE], deadline, ready);
}

/// `write` to a pipe: everything, waiting for room as it goes, or as much as
/// there was room for on a descriptor asked not to wait.
///
/// The SIGPIPE half makes `yes | head` end
fn pipe_write(fd: u64, vid: usize, data: &[u8]) -> u64 {
    use crate::fs::vfs::pipe::{self, PipeWrite};
    let mut done = 0usize;
    loop {
        match pipe::try_write(vid, &data[done..]) {
            PipeWrite::Wrote(n) => {
                done += n;
                // The readers, and their pollers - nobody else
                pipe::wake(vid);
                if done == data.len() {
                    return done as u64;
                }
            }
            PipeWrite::Broken => {
                if done > 0 {
                    return done as u64;
                }
                // The reader has gone. The signal first, then the errno for a
                // process that arranged to survive it, as a shell does
                let me = current_pid();
                crate::process::signal::send_signal(me, crate::process::signal::SIGPIPE);
                // If that was fatal (the default), this task is DEAD and must
                // not be resumed in userspace. `sys_exit` hands the processor
                // away for the same reason
                if !crate::sched::process_alive(me) {
                    crate::sched::yield_now();
                }
                return err(EPIPE);
            }
            PipeWrite::Full => {
                if super::file::fd_is_nonblocking(fd) {
                    return if done > 0 { done as u64 } else { err(EAGAIN) };
                }
                // A signal ends the wait with what was written, as on Linux
                if crate::process::signal::has_deliverable(current_pid()) {
                    return if done > 0 { done as u64 } else { err(EINTR) };
                }
                let rest = &data[done..];
                pipe_wait(vid, || {
                    let (_, writable, gone) = pipe::poll(vid);
                    // Room enough for an atomic write, or any room at all
                    let room = writable && (rest.len() > pipe::ATOMIC_WRITE || pipe::room(vid) >= rest.len());
                    room || gone
                });
            }
        }
    }
}

/// Ticks a pipe reader sleeps for when nobody wakes it (4 ms each)
const PIPE_BACKSTOP: u64 = 25;

// 75  klog(ptr, len) -> bytes written
//
// The kernel's log, and never the screen, nor the serial line unless `trace`: a
// line down a 115200-baud wire is milliseconds with interrupts off, and aosh
// says a dozen when the overview opens (`dmesg` reads them). Stdout is the
// console, so log and output are kept apart
pub fn sys_klog(ptr: u64, len: u64) -> u64 {
    if len == 0 { return 0; }
    if len > MAX_IO_LEN { return err(EINVAL); }
    let cr3 = current_cr3();
    let _ua = match user_ptr_mapped(cr3, ptr, len) { Some(g) => g, None => return err(EFAULT) };
    let s = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    // In the kernel log as the program's own line: module `user`, its pid.
    // The program ends its line, so the record's own newline is cut off it
    use crate::kcore::klog::{flags, quiet_as, Level, Site};
    let say = |a: core::fmt::Arguments| quiet_as(Level::Info, Site::USER, flags::USER, a);
    match core::str::from_utf8(s) {
        Ok(t) => say(format_args!("{}", t.strip_suffix('\n').unwrap_or(t))),
        Err(_) => say(format_args!("{}", alloc::string::String::from_utf8_lossy(s).trim_end_matches('\n'))),
    }
    len
}

// 2  read(fd, buf, len) -> bytes_read
pub fn sys_read(fd: u64, buf: u64, len: u64) -> u64 {
    if len == 0 { return 0; }
    let len = len.min(MAX_IO_LEN);
    let cr3 = current_cr3();
    // write-side: store-through the user buffer requires PTE_WRITABLE
    let _ua = match user_ptr_writable(cr3, buf, len) { Some(g) => g, None => return err(EFAULT) };

    match classify(fd) {
        // Only reach for the keyboard when fd 0 still *is* the terminal
        FdKind::Stdin => crate::io::input::user_stdin::read(buf, len),

        // Pipes are streams: consume from the shared cursor and wait for a
        // writer instead of reporting EOF. The wait is here, not inside
        // with_vfs: sleeping under the global VFS lock would stop the machine
        FdKind::Pipe(vid) => {
            use crate::fs::vfs::core::PipeRead;
            let slice = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len as usize) };
            loop {
                match crate::fs::vfs::core::pipe_try_read(vid, slice) {
                    PipeRead::Data(n) => {
                        // A writer parked because the pipe was full has room
                        crate::fs::vfs::pipe::wake(vid);
                        return n as u64;
                    }
                    PipeRead::Eof => return 0,
                    // Off the run queue until a writer writes or the last one
                    // closes.
                    //
                    // `O_NONBLOCK`, which `fcntl`(113) changes at runtime, as
                    // every reactor sets it
                    PipeRead::WouldBlock => {
                        if super::file::fd_is_nonblocking(fd) { return err(EAGAIN); }
                        pipe_wait(vid, || {
                            let (data, _, gone) = crate::fs::vfs::pipe::poll(vid);
                            data || gone
                        })
                    }
                }
            }
        }

        // Socket fds route to the socket recv path (read == recv, no flags)
        FdKind::Socket => super::net::sys_recv(fd, buf, len, 0),

        FdKind::Inotify(vid) => {
            let slice = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len as usize) };
            super::notify::read(fd, vid, slice)
        }

        // The same wait `fd 0` does, on a terminal that was named. Reading is
        // the foreground job's privilege (`read_from` asks), so two programs
        // sharing a terminal cannot take alternate halves of a line by opening
        // `/dev/tty` each
        FdKind::Tty(id) => {
            if !crate::io::tty::exists(id) { return err(ENXIO); }
            crate::io::input::user_stdin::read_from(id, buf, len)
        }

        // What the program on the other side has printed. Waits for something,
        // like every other stream here, and outside any VFS lock, which is why
        // it is routed here and not left to `devfs::dev_read`
        FdKind::PtyMaster(id) => {
            if !crate::io::tty::exists(id) {
                // The slave side is gone: end of input, not an error
                return 0;
            }
            let slice = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len as usize) };
            use crate::sched::wait::{join, key, keyed, POLL};
            let queues = [keyed(key::pty_output(id)), &POLL];
            let mut joined = None;
            loop {
                let n = crate::io::tty::master_read(id, slice);
                if n > 0 { return n as u64; }
                if !crate::io::tty::exists(id) { return 0; }
                // As for a pipe above: a master asked not to block says so
                // instead of sleeping, for a terminal multiplexer reading
                // several masters in one loop
                if super::file::fd_is_nonblocking(fd) { return err(EAGAIN); }
                // Joined, then looked at once more, then asleep: a print in
                // between is not lost to the backstop (`wait::join`)
                match joined.take() {
                    None => joined = Some(join(&queues)),
                    Some(j) => { j.sleep(crate::sched::tick_deadline(PIPE_BACKSTOP)); }
                }
            }
        }

        // Reading the console, or a descriptor nothing is open on, is whatever
        // the VFS makes of it: an error. A file on a disk reads through the
        // page cache, and a miss reads its pages without holding the VFS lock
        // (`fs::vfs::core::read_fd`)
        FdKind::Console | FdKind::Vfs | FdKind::NetRaw | FdKind::Closed => {
            let slice = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len as usize) };
            match crate::fs::vfs::core::read_fd(fd as usize, slice) {
                Ok(n)  => n as u64,
                Err(e) => vfs_err(e),
            }
        }
    }
}

// 31  write_file(fd, buf, len) -> bytes_written  (VFS only, no console)
pub fn sys_write_file(fd: u64, ptr: u64, len: u64) -> u64 {
    if len == 0 { return 0; }
    let len = len.min(MAX_IO_LEN);

    let cr3 = current_cr3();
    let _ua = match user_ptr_mapped(cr3, ptr, len) { Some(g) => g, None => return err(EFAULT) };

    let buf = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    // The same wake as `sys_write`: a reader may be asleep at the other end.
    //
    // Not routed like `sys_write` otherwise: this call is the VFS one, and fd 1
    // here means the open file at 1, never the console
    if let FdKind::Pipe(vid) = classify(fd) {
        return pipe_write(fd, vid, buf);
    }
    let r = crate::fs::vfs::core::with_vfs(|vfs| match vfs.write(fd as usize, buf) {
        Ok(n)  => n as u64,
        Err(e) => vfs_err(e),
    });
    // Over the dirty limit, this writer writes some out before it goes on
    crate::fs::vfs::core::writeback::balance();
    r
}

// 71  ioctl(fd, request, argp) -> 0
//
// Terminal control: turning off echo, taking the keyboard raw, asking how big
// the screen is.
//
// Only the tty requests are implemented; the numbers follow Linux so a libc can
// pass termios straight through.
pub const TCGETS:     u64 = 0x5401;
pub const TCSETS:     u64 = 0x5402;
pub const TIOCGWINSZ: u64 = 0x5413;
/// Makes this terminal the caller's controlling one: `login_tty` on a slave it
/// has just opened, by the function `tty(ATTACH)` uses
pub const TIOCSCTTY:  u64 = 0x540E;
pub const TIOCSWINSZ: u64 = 0x5414;
/// Which terminal a master descriptor is the master of
///
/// Linux's number, with this machine's "pty number": there the integer in
/// `/dev/pts/N`, here the `TtyId`. A program that has opened `/dev/ptmx` needs
/// it to say which terminal it is drawing
pub const TIOCGPTN:   u64 = 0x8004_5430;

/// Wire layout shared with userspace. Not Linux's 60-byte termios: there are no
/// baud rates or modem lines to configure
#[repr(C)]
struct MikuTermios {
    icanon: u8,
    echo:   u8,
    isig:   u8,
    /// How few characters a raw-mode read may return with, and how long it
    /// waits in tenths of a second: see `user_stdin::read_from`
    vmin:   u8,
    vtime:  u8,
    /// Stop a background job that writes here. POSIX's `TOSTOP`
    tostop: u8,
    /// Which character does what. Indices are `ldisc`'s `V*` constants, and a
    /// zero disables that one: a full-screen editor wanting every byte turns
    /// the interrupt character off rather than `isig`, keeping quit and suspend
    c_cc:   [u8; crate::io::tty::ldisc::NCCS],
    /// To a round twelve bytes
    _pad:   [u8; 1],
}

#[repr(C)]
struct WinSize {
    rows: u16,
    cols: u16,
}

/// The terminal an `ioctl` is about, if the descriptor names one
///
/// Decided from the descriptor, not from `fd <= 2`: `msh < script` puts a file
/// on fd 0, and `ioctl(0, TCSETS)` must not reconfigure the real terminal,
/// while a descriptor on `/dev/tty` or a `dup` of fd 0 onto fd 5 is a terminal
fn ioctl_target(fd: u64) -> Option<(crate::io::tty::TtyId, bool)> {
    match classify(fd) {
        // 0, 1 and 2 with nothing redirected onto them: the caller's own
        // terminal
        FdKind::Stdin | FdKind::Console => {
            Some((crate::io::input::user_stdin::ctty().unwrap_or(crate::io::tty::SYSTEM), false))
        }
        FdKind::Tty(id) if crate::io::tty::exists(id) => Some((id, false)),
        // The descriptor is the right: holding the master is what being the
        // master means
        FdKind::PtyMaster(id) if crate::io::tty::exists(id) => Some((id, true)),
        _ => None,
    }
}

pub fn sys_ioctl(fd: u64, request: u64, argp: u64) -> u64 {
    let Some((id, is_master)) = ioctl_target(fd) else { return err(ENOTTY) };
    let cr3 = current_cr3();

    match request {
        TCGETS => {
            let _ua = match user_ptr_writable(cr3, argp, core::mem::size_of::<MikuTermios>() as u64) {
                Some(g) => g,
                None => return err(EFAULT),
            };
            let t = crate::io::tty::termios(id);
            unsafe {
                (argp as *mut MikuTermios).write_unaligned(MikuTermios {
                    icanon: t.icanon as u8,
                    echo:   t.echo as u8,
                    isig:   t.isig as u8,
                    vmin:   t.vmin,
                    vtime:  t.vtime,
                    tostop: t.tostop as u8,
                    c_cc:   t.c_cc,
                    _pad:   [0; 1],
                });
            }
            0
        }
        TCSETS => {
            // Changing a terminal's settings from the background is SIGTTOU,
            // unconditionally, not gated on `TOSTOP` as a background write is.
            // POSIX requires it: turning the echo off under somebody typing, or
            // leaving canonical mode mid-line, cannot be interleaved and read
            // later
            if !crate::io::tty::in_foreground(id) && crate::io::tty::stop_for_tcsetattr(id) {
                return err(EINTR);
            }
            let _ua = match user_ptr_mapped(cr3, argp, core::mem::size_of::<MikuTermios>() as u64) {
                Some(g) => g,
                None => return err(EFAULT),
            };
            let raw = unsafe { (argp as *const MikuTermios).read_unaligned() };
            crate::io::tty::set_termios(
                id,
                crate::io::tty::ldisc::Termios {
                    icanon: raw.icanon != 0,
                    echo:   raw.echo != 0,
                    isig:   raw.isig != 0,
                    c_cc:   raw.c_cc,
                    vmin:   raw.vmin,
                    vtime:  raw.vtime,
                    tostop: raw.tostop != 0,
                },
            );
            0
        }
        TIOCGWINSZ => {
            let _ua = match user_ptr_writable(cr3, argp, core::mem::size_of::<WinSize>() as u64) {
                Some(g) => g,
                None => return err(EFAULT),
            };
            // The terminal this descriptor names, for fd 0/1/2 the caller's
            // own. Every terminal has a size of its own
            let (cols, rows) = crate::io::tty::size(id);
            unsafe {
                (argp as *mut WinSize).write_unaligned(WinSize {
                    rows: rows as u16,
                    cols: cols as u16,
                });
            }
            0
        }
        // Only a master may ask: this says which terminal this descriptor is
        // the master of
        TIOCSCTTY => super::tty::attach(crate::sched::current_pid(), id),
        TIOCGPTN => {
            if !is_master { return err(ENOTTY); }
            let _ua = match user_ptr_writable(cr3, argp, 4) {
                Some(g) => g,
                None => return err(EFAULT),
            };
            unsafe { (argp as *mut u32).write_unaligned(id.raw()); }
            0
        }
        // The other half of the pair.
        //
        // A terminal emulator here tells the kernel its new shape through
        // `may_console.SET_SIZE`, but a program owning a pty and not a Mayland
        // client has no other way, and `TIOCSWINSZ` is the name programs know.
        // `set_size` signals the job either way
        TIOCSWINSZ => {
            let _ua = match user_ptr_mapped(cr3, argp, core::mem::size_of::<WinSize>() as u64) {
                Some(g) => g,
                None => return err(EFAULT),
            };
            let ws = unsafe { (argp as *const WinSize).read_unaligned() };
            // Only the master may reshape a terminal: otherwise any process
            // sharing the terminal could make somebody's output unreadable.
            //
            // "The master" is the descriptor this call came in on, not a pid,
            // so a forked child holding the master fd qualifies. The console is
            // the machine's and is reshaped by the compositor, not through here
            if !is_master {
                return err(EPERM);
            }
            if crate::io::tty::set_size(id, ws.cols as usize, ws.rows as usize) {
                0
            } else {
                err(EINVAL)
            }
        }
        _ => err(EINVAL),
    }
}

// poll
//
// Waiting on several descriptors at once, so a program can be a server or watch
// its socket and its keyboard without spinning.
//
// `poll` and not `select`: socket descriptors start at 4096 (SOCK_FD_BASE), VFS
// ones are far below, so an `fd_set` would be mostly zeroes on every call.
// `poll` takes the descriptors the caller cares about.

/// Event bits, following Linux so a libc can hand its own `struct pollfd`
/// straight through
pub const POLLIN:   i16 = 0x001;
pub const POLLOUT:  i16 = 0x004;
pub const POLLERR:  i16 = 0x008;
pub const POLLHUP:  i16 = 0x010;
pub const POLLNVAL: i16 = 0x020;

// How many descriptors one call may watch. The arrays below are on the kernel
// stack, where 256 cost about two kilobytes
const MAX_POLL_FDS: usize = 256;

/// Bytes per `struct pollfd`: `{ i32 fd; i16 events; i16 revents; }`
const POLLFD_SIZE: u64 = 8;

/// How long a poller sleeps when nobody wakes it
///
/// Shorter than the pipe backstop: a poller is usually a program's whole event
/// loop, so a missed wake looks like a hang. The queues of its own descriptors
/// wake it (`wait::keyed`), and this is what a missed wake costs
const POLL_BACKSTOP_NS: u64 = 50_000_000;

/// The queue a wake for `fd` lands on, for the kinds that have one of their
/// own; `None` for the ones only `wait::POLL` hears of
fn wait_key_for(fd: i32) -> Option<u64> {
    use crate::sched::wait::key;
    if fd < 0 { return None; }
    let fd = fd as u64;
    match classify(fd) {
        FdKind::Pipe(vid) => Some(crate::fs::vfs::pipe::wait_key(vid)),
        FdKind::Socket => crate::net::socket::wait_key(fd),
        FdKind::Stdin => Some(key::tty_input(crate::io::input::user_stdin::mine())),
        FdKind::Tty(id) => Some(key::tty_input(id)),
        FdKind::PtyMaster(id) => Some(key::pty_output(id)),
        // One flag for every instance raises it (`notify::wake_if_raised`),
        // so `POLL` is what it wakes
        FdKind::Inotify(_) | FdKind::NetRaw | FdKind::Console | FdKind::Vfs | FdKind::Closed => None,
    }
}

/// What `fd` can do right now, as poll event bits
///
/// Routing goes through `classify`, as for `read` and `write`
pub(crate) fn revents_for(fd: i32, events: i16) -> i16 {
    // Linux ignores a negative entry, which is how a caller parks a slot in a
    // fixed-size set without rebuilding it
    if fd < 0 { return 0; }
    let fd = fd as u64;

    let (readable, writable, hup) = match classify(fd) {
        FdKind::Stdin => (crate::io::input::user_stdin::readable(), false, false),

        // The console can be written to and not read from
        FdKind::Console => (false, true, false),

        FdKind::Pipe(vid) => {
            let (data, room, hup) = crate::fs::vfs::pipe::poll(vid);
            // End of file is progress: the read returns 0 at once, so a drained
            // pipe with no writers is readable, and a write to a pipe nobody
            // reads fails at once, so it is writable
            (data || hup, room || hup, hup)
        }

        // A socket answers only to the process holding it, as at every other
        // socket entry point. Otherwise any process could learn whether any
        // slot is readable: an oracle over every connection on the machine,
        // such as a password on its way through `/run/mikudo.sock`
        FdKind::Socket => {
            if !crate::net::socket::owned_by(fd, crate::sched::current_tgid()) {
                return POLLNVAL;
            }
            let r = crate::net::socket::poll_fd(fd);
            if r.invalid { return POLLNVAL; }
            (r.readable, r.writable, r.hup)
        }

        // Readable only with a frame waiting. Always writable: a full ring is a
        // failed `write`, not a reason to sleep
        FdKind::NetRaw => (crate::net::raw::pending(), true, false),

        // What fd 0 answers, for the terminal this descriptor names. A terminal
        // that has gone is a hangup, not an error: a poller learns its window
        // closed as on a pipe
        FdKind::Tty(id) => {
            if !crate::io::tty::exists(id) { return POLLHUP; }
            (crate::io::input::user_stdin::readable_on(id), true, false)
        }

        // Readable when the program on the other side has printed; hung up when
        // that program's terminal has gone. As for a pipe: end of input is
        // progress, so a hung-up master is readable
        FdKind::PtyMaster(id) => {
            let gone = !crate::io::tty::exists(id);
            (crate::io::tty::master_pending(id) > 0 || gone, true, gone)
        }

        // Readable with an event queued; never writable, since a write is
        // EINVAL
        FdKind::Inotify(vid) => (crate::fs::vfs::notify::pending(vid), false, false),

        // An ordinary file never makes anyone wait
        FdKind::Vfs => (true, true, false),

        FdKind::Closed => return POLLNVAL,
    };

    let mut re = 0i16;
    if readable && (events & POLLIN)  != 0 { re |= POLLIN; }
    if writable && (events & POLLOUT) != 0 { re |= POLLOUT; }
    // Hangup is reported whether or not asked for, as on Linux: a program
    // subscribed only to POLLIN still has to learn the peer has gone
    if hup { re |= POLLHUP; }
    re
}

// 76  poll(fds, nfds, timeout_ms) -> ready count | 0 on timeout | -errno
pub fn sys_poll(fds_ptr: u64, nfds: u64, timeout_ms: u64) -> u64 {
    // The bit pattern of an i64; negative means no deadline
    let timeout = timeout_ms as i64;
    let deadline = (timeout >= 0).then(|| {
        crate::kcore::time::now_ns().saturating_add((timeout as u64).saturating_mul(1_000_000))
    });
    poll_until(fds_ptr, nfds, deadline)
}

// 123  poll_ns(fds, nfds, timeout_ns) -> ready count | 0 on timeout | -errno
//
// `poll` with its timeout in nanoseconds: `ppoll`'s precision without its
// signal mask
pub fn sys_poll_ns(fds_ptr: u64, nfds: u64, timeout_ns: u64) -> u64 {
    let timeout = timeout_ns as i64;
    let deadline = (timeout >= 0).then(|| crate::kcore::time::now_ns().saturating_add(timeout as u64));
    poll_until(fds_ptr, nfds, deadline)
}

/// `poll` until `deadline` on `kcore::time::now_ns`'s clock, or for ever
fn poll_until(fds_ptr: u64, nfds: u64, deadline: Option<u64>) -> u64 {
    let n = nfds as usize;
    if n > MAX_POLL_FDS { return err(EINVAL); }

    // nfds == 0 is a sleep with no descriptors, a legitimate thing to ask. An
    // infinite one would never return
    if n == 0 {
        match deadline {
            Some(d) => crate::sched::sleep_until(d),
            None => return err(EINVAL),
        }
        return 0;
    }

    let bytes = nfds * POLLFD_SIZE;
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, fds_ptr, bytes) { Some(g) => g, None => return err(EFAULT) };

    let mut fds    = [0i32; MAX_POLL_FDS];
    let mut events = [0i16; MAX_POLL_FDS];
    // Copied in whole before anything is examined: the set must not change
    // between the scan and the write-back
    for i in 0..n {
        let base = fds_ptr + i as u64 * POLLFD_SIZE;
        unsafe {
            fds[i]    = core::ptr::read_unaligned(base as *const i32);
            events[i] = core::ptr::read_unaligned((base + 4) as *const i16);
        }
    }
    drop(_ua);
    // A socket and /dev/net0 become readable only when somebody drained the
    // driver. Classified once, here: the answer cannot change while these
    // descriptors are held
    let needs_pump = fds[..n].iter().any(|&f| {
        f >= 0 && matches!(classify(f as u64), FdKind::Socket | FdKind::NetRaw)
    });
    // What to sleep on: the queue of every descriptor that has one, and the
    // queue of everything that has none
    let mut queues: [&'static crate::sched::wait::WaitQueue; MAX_POLL_FDS + 1] =
        [&crate::sched::wait::POLL; MAX_POLL_FDS + 1];
    let mut m = 1;
    for &f in &fds[..n] {
        if let Some(k) = wait_key_for(f) {
            queues[m] = crate::sched::wait::keyed(k);
            m += 1;
        }
    }

    let mut revents = [0i16; MAX_POLL_FDS];
    // On the queues before the look that decides the sleep (`wait::join`)
    let mut joined: Option<crate::sched::wait::Joined> = None;

    loop {
        // Readiness on a socket depends on somebody draining the RX demux. netd
        // does it every tick, and every blocking socket op does; a sleeping
        // poller does neither, so it does it itself
        if needs_pump { crate::net::rx::pump(); }

        let mut ready = 0u64;
        for i in 0..n {
            revents[i] = revents_for(fds[i], events[i]);
            if revents[i] != 0 { ready += 1; }
        }

        let now = crate::kcore::time::now_ns();
        let expired = deadline.is_some_and(|d| now >= d);

        if ready > 0 || expired {
            // Re-validates before the store: another thread could have unmapped
            // the set during the scan
            let _ua = match user_ptr_writable(cr3, fds_ptr, bytes) { Some(g) => g, None => return err(EFAULT) };
            for i in 0..n {
                let base = fds_ptr + i as u64 * POLLFD_SIZE;
                unsafe { core::ptr::write_unaligned((base + 6) as *mut i16, revents[i]); }
            }
            return ready;
        }

        match joined.take() {
            // On the queues, and one more look: a descriptor that moved
            // between the scan above and joining woke nobody
            None => joined = Some(crate::sched::wait::join(&queues[..m])),
            // Off the run queue until one of these descriptors moves, the
            // deadline comes, or the backstop does
            Some(j) => {
                let backstop = now.saturating_add(POLL_BACKSTOP_NS);
                j.sleep(deadline.map_or(backstop, |d| d.min(backstop)));
            }
        }
    }
}
