// Stream I/O syscalls: read, write, write_file

use super::errno::{err, vfs_err, EFAULT, EINVAL};
use super::user_mem::{current_cr3, user_ptr_mapped, user_ptr_writable};

const MAX_IO_LEN: u64 = 1 << 20; // 1 MiB cap on a single syscall buffer

// 1  write(fd, buf, len) -> bytes_written
pub fn sys_write(fd: u64, ptr: u64, len: u64) -> u64 {
    if len == 0 { return 0; }
    if len > MAX_IO_LEN { return err(EINVAL); }

    let cr3 = current_cr3();
    // read-side: just needs to be readable from userspace
    if !user_ptr_mapped(cr3, ptr, len) { return err(EFAULT); }

    // stdout/stderr go to console
    if fd == 1 || fd == 2 {
        let s = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        match core::str::from_utf8(s) {
            Ok(t)  => crate::print!("{}", t),
            Err(_) => for &b in s { crate::print!("{}", b as char); },
        }
        return len;
    }

    // Socket fds route to the socket send path (write == send, no flags)
    if crate::net::socket::is_socket_fd(fd) {
        return super::net::sys_send(fd, ptr, len, 0);
    }

    let buf = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    crate::vfs::core::with_vfs(|vfs| match vfs.write(fd as usize, buf) {
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

    if fd == 0 {
        return crate::user_stdin::read(buf, len);
    }

    // Socket fds route to the socket recv path (read == recv, no flags)
    if crate::net::socket::is_socket_fd(fd) {
        return super::net::sys_recv(fd, buf, len, 0);
    }

    let slice = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len as usize) };
    crate::vfs::core::with_vfs(|vfs| match vfs.read(fd as usize, slice) {
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
    crate::vfs::core::with_vfs(|vfs| match vfs.write(fd as usize, buf) {
        Ok(n)  => n as u64,
        Err(e) => vfs_err(e),
    })
}
