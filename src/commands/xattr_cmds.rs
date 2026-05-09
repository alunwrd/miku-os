use crate::commands::ext2_cmds::with_ext2_pub;
use crate::miku_extfs::structs::*;
use crate::miku_extfs::FsError;
use crate::{print_error, print_success, println};

pub fn cmd_getxattr(path: &str, name: &str) {
    if path.is_empty() || name.is_empty() {
        println!("Usage: getxattr <path> <name>");
        return;
    }
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        let mut buf = [0u8; 256];
        let len = fs.get_xattr(ino, crate::miku_extfs::xattr::XATTR_INDEX_USER, name, &mut buf)?;
        if let Ok(s) = core::str::from_utf8(&buf[..len]) {
            println!("  {}=\"{}\"", name, s);
        } else {
            println!("  {} = [{} bytes]", name, len);
        }
        Ok(())
    });
    match result {
        Some(Ok(())) => {}
        Some(Err(FsError::NotFound)) => print_error!("  xattr '{}' not found", name),
        Some(Err(e)) => print_error!("  getxattr: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_setxattr(path: &str, name: &str, value: &str) {
    if path.is_empty() || name.is_empty() {
        println!("Usage: setxattr <path> <name> <value>");
        return;
    }
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        fs.set_xattr(ino, crate::miku_extfs::xattr::XATTR_INDEX_USER, name, value.as_bytes())
    });
    match result {
        Some(Ok(())) => print_success!("  xattr '{}' set", name),
        Some(Err(e)) => print_error!("  setxattr: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_listxattr(path: &str) {
    if path.is_empty() { println!("Usage: listxattr <path>"); return; }
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        let list = fs.read_xattrs(ino)?;
        if list.count == 0 {
            println!("  (no extended attributes)");
        }
        for i in 0..list.count {
            let e = &list.entries[i];
            let ns = match e.name_index {
                1 => "user",
                2 => "system.posix_acl_access",
                3 => "system.posix_acl_default",
                4 => "trusted",
                6 => "security",
                7 => "system",
                _ => "unknown",
            };
            if let Ok(val) = core::str::from_utf8(&e.value[..e.value_len as usize]) {
                println!("  {}.{}=\"{}\"", ns, e.name_str(), val);
            } else {
                println!("  {}.{} = [{} bytes]", ns, e.name_str(), e.value_len);
            }
        }
        Ok(())
    });
    match result {
        Some(Ok(())) => {}
        Some(Err(e)) => print_error!("  listxattr: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_chattr(flags_str: &str, path: &str) {
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        let inode = fs.read_inode(ino)?;
        let mut flags = inode.flags();
        let adding = flags_str.starts_with('+');
        let removing = flags_str.starts_with('-');
        if !adding && !removing {
            return Err(FsError::InvalidArg);
        }
        let chars = &flags_str[1..];
        for c in chars.bytes() {
            let flag = match c {
                b'i' => EXT4_IMMUTABLE_FL,
                b'a' => EXT4_APPEND_FL,
                b'd' => EXT4_NODUMP_FL,
                b'A' => EXT4_NOATIME_FL,
                _ => continue,
            };
            if adding { flags |= flag; } else { flags &= !flag; }
        }
        let mut inode = fs.read_inode(ino)?;
        inode.set_flags(flags);
        let now = fs.get_timestamp();
        inode.set_ctime(now);
        fs.write_inode(ino, &inode)
    });
    match result {
        Some(Ok(())) => print_success!("  flags updated"),
        Some(Err(e)) => print_error!("  chattr: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_lsattr(path: &str) {
    if path.is_empty() { println!("Usage: lsattr <path>"); return; }
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        let inode = fs.read_inode(ino)?;
        let f = inode.flags();
        let mut attrs = [b'-'; 8];
        if f & EXT4_IMMUTABLE_FL   != 0 { attrs[0] = b'i'; }
        if f & EXT4_APPEND_FL      != 0 { attrs[1] = b'a'; }
        if f & EXT4_NODUMP_FL      != 0 { attrs[2] = b'd'; }
        if f & EXT4_NOATIME_FL     != 0 { attrs[3] = b'A'; }
        if f & EXT4_EXTENTS_FL     != 0 { attrs[4] = b'e'; }
        if f & EXT4_INDEX_FL       != 0 { attrs[5] = b'I'; }
        if f & EXT4_HUGE_FILE_FL   != 0 { attrs[6] = b'h'; }
        if f & EXT4_INLINE_DATA_FL != 0 { attrs[7] = b'N'; }
        let s = core::str::from_utf8(&attrs).unwrap_or("-----");
        println!("  {} {}", s, path);
        Ok(())
    });
    match result {
        Some(Ok(())) => {}
        Some(Err(e)) => print_error!("  lsattr: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}
