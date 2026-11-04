extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

// Amortizes the per-call ext2 lock + path/inode lookup. Typical ELF binaries
// run several hundred KB, so 64 KiB cuts round trips ~16x vs single-page reads.
const READ_CHUNK: usize = 64 * 1024;

/// Ceiling on a whole-file read. These functions allocate the entire file in
/// the kernel heap (`mm::heap`), and the size comes from an on-disk inode. The
/// ELF loader's own 64 MiB cap applies only after the read has allocated, so
/// without this a large file is an unbounded kernel-heap allocation reachable
/// from ring 3
pub const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;

fn too_big(path: &str, total: usize) -> bool {
    if total > MAX_FILE_BYTES {
        crate::serial_println!(
            "[vfs_read] '{}' is {} bytes, over the {} byte limit", path, total, MAX_FILE_BYTES,
        );
        return true;
    }
    false
}

pub fn read_file(path: &str) -> Option<Vec<u8>> {
    let path_clean = path.trim_end_matches('\0');

    if let Some(data) = read_from_vfs(path_clean) {
        return Some(data);
    }

    read_from_ext2(path_clean)
}

pub fn read_file_or_solib(path: &str) -> Option<Vec<u8>> {
    let path_clean = path.trim_end_matches('\0');

    if path_clean.ends_with(".so") || path_clean.contains(".so.") {
        if let Some(data) = crate::process::solib::resolve_path(path_clean) {
            return Some((*data).clone());
        }
        let soname = path_clean.rsplit('/').next().unwrap_or(path_clean);
        if let Some(data) = crate::process::solib::resolve(soname) {
            return Some((*data).clone());
        }
    }

    read_file(path_clean)
}

pub fn read_from_vfs(path: &str) -> Option<Vec<u8>> {
    crate::fs::vfs::core::with_vfs(|vfs| -> Option<Vec<u8>> {
        use crate::fs::vfs::types::{OpenFlags, FileMode};
        let fl = OpenFlags(OpenFlags::READ);
        let fd = vfs.open(0, path, fl, FileMode::default_file()).ok()?;
        let size = vfs.fstat(fd).ok()?.size as usize;
        if size == 0 || too_big(path, size) {
            let _ = vfs.close(fd);
            return None;
        }
        let mut buf = vec![0u8; size];
        let n = vfs.read(fd, &mut buf).ok()?;
        buf.truncate(n);
        let _ = vfs.close(fd);
        if n > 0 { Some(buf) } else { None }
    })
}

pub fn read_from_ext2(path: &str) -> Option<Vec<u8>> {
    use crate::fs::ext::mount::with_ext2_pub;
    use crate::fs::ext::error::FsError;

    let result = with_ext2_pub(|fs| -> Result<Vec<u8>, FsError> {
        let ino = fs.resolve_path(path)?;
        let inode = fs.read_inode(ino)?;
        if !inode.is_regular() {
            return Err(FsError::NotRegularFile);
        }
        let total = inode.size() as usize;
        if total == 0 {
            return Ok(Vec::new());
        }
        if too_big(path, total) {
            return Err(FsError::IoError);
        }
        let mut buf = vec![0u8; total];
        let mut offset = 0usize;
        while offset < total {
            let chunk = READ_CHUNK.min(total - offset);
            let n = fs.read_file(&inode, offset as u64, &mut buf[offset..offset + chunk])?;
            if n == 0 {
                break;
            }
            offset += n;
        }
        buf.truncate(offset);
        Ok(buf)
    });

    match result {
        Some(Ok(d)) if !d.is_empty() => Some(d),
        _ => None,
    }
}

#[derive(Debug)]
pub enum ReadError {
    FsNotMounted,
    FileNotFound,
    NotRegularFile,
    IoError,
}

pub fn read_file_strict(path: &str) -> Result<Vec<u8>, ReadError> {
    if let Some(data) = read_from_vfs(path) {
        return Ok(data);
    }

    use crate::fs::ext::mount::with_ext2_pub;
    use crate::fs::ext::error::FsError;

    let result = with_ext2_pub(|fs| -> Result<Vec<u8>, FsError> {
        let ino = fs.resolve_path(path)?;
        let inode = fs.read_inode(ino)?;
        if !inode.is_regular() {
            return Err(FsError::NotRegularFile);
        }
        let total = inode.size() as usize;
        crate::serial_println!("[vfs_read] '{}' ino={} size={} blocks={}", path, ino, total, inode.blocks());
        if total == 0 {
            return Ok(Vec::new());
        }
        if too_big(path, total) {
            return Err(FsError::IoError);
        }
        let mut buf = vec![0u8; total];
        let mut offset = 0usize;
        while offset < total {
            let chunk = READ_CHUNK.min(total - offset);
            let n = fs.read_file(&inode, offset as u64, &mut buf[offset..offset + chunk])?;
            if n == 0 {
                crate::serial_println!("[vfs_read] warning: read_file returned 0 at offset {}/{}", offset, total);
                break;
            }
            offset += n;
        }
        if offset < total {
            crate::serial_println!("[vfs_read] warning: only read {}/{} bytes, truncating", offset, total);
            buf.truncate(offset);
        }
        Ok(buf)
    });

    match result {
        Some(Ok(data)) => {
            crate::serial_println!("[vfs_read] read {} bytes from '{}'", data.len(), path);
            Ok(data)
        }
        Some(Err(FsError::NotFound)) => Err(ReadError::FileNotFound),
        Some(Err(FsError::NotRegularFile)) => Err(ReadError::NotRegularFile),
        Some(Err(_)) => Err(ReadError::IoError),
        None => Err(ReadError::FsNotMounted),
    }
}
