// Stream I/O syscalls: read, write, write_file

use crate::syscall::errno::{err, vfs_err, EFAULT, EINVAL, ENOTTY};
use crate::syscall::usercopy::{current_cr3, user_ptr_mapped, user_ptr_writable};

const MAX_IO_LEN: u64 = 1 << 20; // 1 MiB cap on a single syscall buffer

// 1  write(fd, buf, len) -> bytes_written
pub fn sys_write(fd: u64, ptr: u64, len: u64) -> u64 {
    if len == 0 { return 0; }
    if len > MAX_IO_LEN { return err(EINVAL); }

    let cr3 = current_cr3();
    // read-side: just needs to be readable from userspace
    if !user_ptr_mapped(cr3, ptr, len) { return err(EFAULT); }

    // stdout/stderr go to the console - unless the descriptor has been
    // redirected. dup2() installs a real entry at fd 1/2, but this path used
    // to short-circuit to the console before ever consulting the table, so
    // `cmd > file` silently printed to the screen and wrote nothing
    if fd == 1 || fd == 2 {
        let redirected =
            crate::fs::vfs::core::with_vfs(|vfs| vfs.fds().get(fd as usize).is_ok());
        if !redirected {
            let s = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
            match core::str::from_utf8(s) {
                Ok(t)  => crate::print!("{}", t),
                Err(_) => for &b in s { crate::print!("{}", b as char); },
            }
            return len;
        }
    }

    // Socket fds route to the socket send path (write == send, no flags)
    if crate::net::socket::is_socket_fd(fd) {
        return super::net::sys_send(fd, ptr, len, 0);
    }

    let buf = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    crate::fs::vfs::core::with_vfs(|vfs| match vfs.write(fd as usize, buf) {
        Ok(n)  => n as u64,
        Err(e) => vfs_err(e),
    })
}

// 2  read(fd, buf, len) -> bytes_read
pub fn sys_read(fd: u64, buf: u64, len: u64) -> u64 {
    if len == 0 { return 0; }
    if len > MAX_IO_LEN { return err(EINVAL); }
    let cr3 = current_cr3();
    // write-side: store-through the user buffer requires PTE_WRITABLE
    if !user_ptr_writable(cr3, buf, len) { return err(EFAULT); }

    // Same for stdin: only reach for the keyboard when fd 0 still is the
    // terminal. After `cmd < file` it is an ordinary open file
    if fd == 0 {
        let redirected =
            crate::fs::vfs::core::with_vfs(|vfs| vfs.fds().get(0usize).is_ok());
        if !redirected {
            return crate::io::input::user_stdin::read(buf, len);
        }
    }


    // Pipes are streams: consume from the shared cursor and wait for a
    // writer instead of reporting EOF. Waiting happens here, not inside
    // with_vfs - sleeping under the global VFS lock would stop the machine
    if let Some(vid) = crate::fs::vfs::core::pipe_vnode_of_fd(fd as usize) {
        use crate::fs::vfs::core::PipeRead;
        let slice = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len as usize) };
        loop {
            match crate::fs::vfs::core::pipe_try_read(vid, slice) {
                PipeRead::Data(n) => return n as u64,
                PipeRead::Eof => return 0,
                PipeRead::WouldBlock => crate::sched::yield_now(),
            }
        }
    }

    // Socket fds route to the socket recv path (read == recv, no flags)
    if crate::net::socket::is_socket_fd(fd) {
        return super::net::sys_recv(fd, buf, len, 0);
    }

    let slice = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len as usize) };
    crate::fs::vfs::core::with_vfs(|vfs| match vfs.read(fd as usize, slice) {
        Ok(n)  => n as u64,
        Err(e) => vfs_err(e),
    })
}

// 31  write_file(fd, buf, len) -> bytes_written  (VFS only, no console)
pub fn sys_write_file(fd: u64, ptr: u64, len: u64) -> u64 {
    if len == 0 { return 0; }
    if len > MAX_IO_LEN { return err(EINVAL); }

    let cr3 = current_cr3();
    if !user_ptr_mapped(cr3, ptr, len) { return err(EFAULT); }

    let buf = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    crate::fs::vfs::core::with_vfs(|vfs| match vfs.write(fd as usize, buf) {
        Ok(n)  => n as u64,
        Err(e) => vfs_err(e),
    })
}

// 71  ioctl(fd, request, argp) -> 0
//
// Terminal control. Without it nothing in ring 3 could turn off echo, take
// the keyboard raw, or ask how big the screen is - the line discipline was
// fixed at "canonical, echoing, Ctrl-C kills" for every program.
//
// Only the tty requests are implemented; the numbers follow Linux so a libc
// can pass termios straight through.
pub const TCGETS:     u64 = 0x5401;
pub const TCSETS:     u64 = 0x5402;
pub const TIOCGWINSZ: u64 = 0x5413;

/// Wire layout shared with userspace. Deliberately not Linux's 60-byte
/// termios: MikuOS has no baud rates or modem lines to configure, and
/// pretending otherwise would mean maintaining fields nothing reads
#[repr(C)]
struct MikuTermios {
    icanon: u8,
    echo:   u8,
    isig:   u8,
    /// Padding to keep the struct a round 4 bytes. Previously this was a
    /// `vmin` field that ioctl faithfully reported and the read path never
    /// consulted - an ABI promising behaviour that did not exist
    _pad:   u8,
}

#[repr(C)]
struct WinSize {
    rows: u16,
    cols: u16,
}

pub fn sys_ioctl(fd: u64, request: u64, argp: u64) -> u64 {
    // Only the three standard descriptors are terminals here
    if fd > 2 {
        return err(ENOTTY);
    }
    let cr3 = current_cr3();

    match request {
        TCGETS => {
            if !user_ptr_writable(cr3, argp, core::mem::size_of::<MikuTermios>() as u64) {
                return err(EFAULT);
            }
            let t = crate::io::input::user_stdin::termios();
            unsafe {
                (argp as *mut MikuTermios).write_unaligned(MikuTermios {
                    icanon: t.icanon as u8,
                    echo:   t.echo as u8,
                    isig:   t.isig as u8,
                    _pad:   0,
                });
            }
            0
        }
        TCSETS => {
            if !user_ptr_mapped(cr3, argp, core::mem::size_of::<MikuTermios>() as u64) {
                return err(EFAULT);
            }
            let raw = unsafe { (argp as *const MikuTermios).read_unaligned() };
            crate::io::input::user_stdin::set_termios(
                crate::io::input::user_stdin::Termios {
                    icanon: raw.icanon != 0,
                    echo:   raw.echo != 0,
                    isig:   raw.isig != 0,
                },
            );
            0
        }
        TIOCGWINSZ => {
            if !user_ptr_writable(cr3, argp, core::mem::size_of::<WinSize>() as u64) {
                return err(EFAULT);
            }
            let (cols, rows) = crate::io::console::text_size();
            unsafe {
                (argp as *mut WinSize).write_unaligned(WinSize {
                    rows: rows as u16,
                    cols: cols as u16,
                });
            }
            0
        }
        _ => err(EINVAL),
    }
}
