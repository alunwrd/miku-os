extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

const READ_CHUNK: usize = 4096;

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
        if let Some(data) = crate::solib::resolve_path(path_clean) {
            return Some(data);
        }
        let soname = path_clean.rsplit('/').next().unwrap_or(path_clean);
        if let Some(data) = crate::solib::resolve(soname) {
            return Some(data);
        }
    }

    read_file(path_clean)
}

pub fn read_from_vfs(path: &str) -> Option<Vec<u8>> {
    crate::vfs::core::with_vfs(|vfs| -> Option<Vec<u8>> {
        use crate::vfs::types::{OpenFlags, FileMode};
        let fl = OpenFlags(OpenFlags::READ);
        let fd = vfs.open(0, path, fl, FileMode::default_file()).ok()?;
        let size = vfs.fstat(fd).ok()?.size as usize;
        if size == 0 {
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
    use crate::commands::ext2_cmds::with_ext2_pub;
    use crate::miku_extfs::error::FsError;

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

    use crate::commands::ext2_cmds::with_ext2_pub;
    use crate::miku_extfs::error::FsError;

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
