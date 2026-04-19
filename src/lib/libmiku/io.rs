use crate::sys::*;

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_write(fd: u64, buf: *const u8, len: usize) -> i64 {
    unsafe { sc3(SYS_WRITE, fd, buf as u64, len as u64) }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_read(fd: u64, buf: *mut u8, len: usize) -> i64 {
    unsafe { sc3(SYS_READ, fd, buf as u64, len as u64) }
}

pub fn write_all(fd: u64, data: &[u8]) -> i64 {
    if data.is_empty() { return 0; }
    let mut done = 0usize;
    while done < data.len() {
        let n = miku_write(fd, unsafe { data.as_ptr().add(done) }, data.len() - done);
        if n <= 0 { return n; }
        done += n as usize;
    }
    done as i64
}

pub fn read_all(fd: u64, buf: &mut [u8]) -> i64 {
    if buf.is_empty() { return 0; }
    let mut done = 0usize;
    while done < buf.len() {
        let n = miku_read(fd, unsafe { buf.as_mut_ptr().add(done) }, buf.len() - done);
        if n < 0 { return n; }
        if n == 0 { break; }
        done += n as usize;
    }
    done as i64
}
