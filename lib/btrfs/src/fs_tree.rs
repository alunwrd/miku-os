// Inodes, names, and directories
//
//   ROOT_ITEM    where a subvolume's tree is, in the root tree
//   INODE_ITEM   a file's metadata, keyed by inode number
//   DIR_ITEM     a name, keyed by `(dir, 84, crc32c-of-the-name)`
//   DIR_INDEX    the same entry keyed by position, walked by `readdir` for a stable order
//
// A lookup does not scan: the key's offset is `btrfs_name_hash(name)`. Two
// colliding names share one item, so an item is a sequence of records, and the
// loop below compares names after finding the hash.
//
// `btrfs_name_hash` is `crc32c((u32)~1, name, len)`, where Linux's `crc32c()`
// is the raw accumulation with no inversion at either end, unlike block
// checksums, the standard inverted CRC-32C. The test compares against keys
// `mkfs.btrfs` wrote.

use crate::error::BtrfsError;
use crate::structs::*;
use alloc::vec::Vec;

// key types

pub const INODE_ITEM_KEY: u8 = 1;
pub const INODE_REF_KEY: u8 = 12;
pub const INODE_EXTREF_KEY: u8 = 13;
pub const XATTR_ITEM_KEY: u8 = 24;
pub const DIR_ITEM_KEY: u8 = 84;
pub const DIR_INDEX_KEY: u8 = 96;
pub const EXTENT_DATA_KEY: u8 = 108;
pub const EXTENT_CSUM_KEY: u8 = 128;
pub const ROOT_ITEM_KEY: u8 = 132;
pub const ROOT_REF_KEY: u8 = 156;

// Tree ids

pub const ROOT_TREE_OBJECTID: u64 = 1;
pub const CHUNK_TREE_OBJECTID: u64 = 3;
pub const FS_TREE_OBJECTID: u64 = 5;
pub const CSUM_TREE_OBJECTID: u64 = 7;
/// The first inode number a file can have. Everything below is reserved
pub const FIRST_FREE_OBJECTID: u64 = 256;

// Entry types

pub const FT_UNKNOWN: u8 = 0;
pub const FT_REG_FILE: u8 = 1;
pub const FT_DIR: u8 = 2;
pub const FT_CHRDEV: u8 = 3;
pub const FT_BLKDEV: u8 = 4;
pub const FT_FIFO: u8 = 5;
pub const FT_SOCK: u8 = 6;
pub const FT_SYMLINK: u8 = 7;
pub const FT_XATTR: u8 = 8;
pub const FT_MAX: u8 = 9;
/// Set alongside the type for encrypted names. Masked off before the type is
/// read, as `btrfs_dir_flags_to_ftype` does
pub const FT_ENCRYPTED: u8 = 0x80;

pub const NAME_LEN_MAX: usize = 255;

/// Offset a `DIR_ITEM` or `XATTR_ITEM` key carries for this name:
/// `crc32c((u32)~1, name)` in the kernel, the raw accumulation, not the
/// standard CRC-32C of block checksums
pub fn name_hash(name: &[u8]) -> u64 {
    fshost::crc32c::crc32c_raw(!1u32, name) as u64
}

// The items

/// Field offsets in `struct btrfs_inode_item`
pub mod inode {
    pub const GENERATION: usize = 0;
    pub const TRANSID: usize = 8;
    pub const SIZE: usize = 16;
    pub const NBYTES: usize = 24;
    pub const NLINK: usize = 40;
    pub const UID: usize = 44;
    pub const GID: usize = 48;
    pub const MODE: usize = 52;
    pub const RDEV: usize = 56;
    pub const FLAGS: usize = 64;
    pub const ATIME: usize = 112;
    pub const CTIME: usize = 124;
    pub const MTIME: usize = 136;
    pub const OTIME: usize = 148;
    pub const SIZE_OF: usize = 160;
}

/// Field offsets in `struct btrfs_dir_item`. The name follows the header, and
/// data (for an xattr) the name
pub mod dir_item {
    pub const LOCATION: usize = 0;
    pub const TRANSID: usize = 17;
    pub const DATA_LEN: usize = 25;
    pub const NAME_LEN: usize = 27;
    pub const TYPE: usize = 29;
    pub const SIZE_OF: usize = 30;
}

/// Field offsets in `struct btrfs_root_item`, which begins with a whole inode
/// item: hence the 160
pub mod root_item {
    pub const GENERATION: usize = 160;
    pub const ROOT_DIRID: usize = 168;
    pub const BYTENR: usize = 176;
    pub const BYTES_USED: usize = 192;
    pub const FLAGS: usize = 208;
    pub const REFS: usize = 216;
    pub const DROP_LEVEL: usize = 237;
    pub const LEVEL: usize = 238;
    /// The item's size before uuid and time fields. An older volume's stops
    /// here, so nothing past it is read without checking the size
    pub const LEGACY_SIZE: usize = 239;
}

/// What an inode says about itself
#[derive(Clone, Copy, Debug)]
pub struct Inode {
    pub size: u64,
    pub nbytes: u64,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    /// The full mode word, type bits included
    pub mode: u32,
    pub rdev: u64,
    pub flags: u64,
    pub atime: (u64, u32),
    pub ctime: (u64, u32),
    pub mtime: (u64, u32),
}

impl Inode {
    pub fn parse(data: &[u8]) -> Result<Self, BtrfsError> {
        if data.len() < inode::SIZE_OF {
            return Err(BtrfsError::Corrupt);
        }
        let time = |at: usize| -> Result<(u64, u32), BtrfsError> {
            Ok((u64_at(data, at)?, u32_at(data, at + 8)?))
        };
        Ok(Inode {
            size: u64_at(data, inode::SIZE)?,
            nbytes: u64_at(data, inode::NBYTES)?,
            nlink: u32_at(data, inode::NLINK)?,
            uid: u32_at(data, inode::UID)?,
            gid: u32_at(data, inode::GID)?,
            mode: u32_at(data, inode::MODE)?,
            rdev: u64_at(data, inode::RDEV)?,
            flags: u64_at(data, inode::FLAGS)?,
            atime: time(inode::ATIME)?,
            ctime: time(inode::CTIME)?,
            mtime: time(inode::MTIME)?,
        })
    }

    pub fn is_dir(&self) -> bool {
        self.mode & 0xF000 == 0x4000
    }
    pub fn is_regular(&self) -> bool {
        self.mode & 0xF000 == 0x8000
    }
    pub fn is_symlink(&self) -> bool {
        self.mode & 0xF000 == 0xA000
    }
    pub fn permissions(&self) -> u16 {
        (self.mode & 0o7777) as u16
    }
}

/// A subvolume's tree, as the root tree records it
#[derive(Clone, Copy, Debug)]
pub struct Root {
    pub bytenr: u64,
    pub level: u8,
    /// The inode number of the subvolume's own top directory
    pub root_dirid: u64,
    pub generation: u64,
}

impl Root {
    pub fn parse(data: &[u8]) -> Result<Self, BtrfsError> {
        // The legacy size is the floor
        if data.len() < root_item::LEGACY_SIZE {
            return Err(BtrfsError::Corrupt);
        }
        let level = u8_at(data, root_item::LEVEL)?;
        if level >= MAX_LEVEL {
            return Err(BtrfsError::Corrupt);
        }
        Ok(Root {
            bytenr: u64_at(data, root_item::BYTENR)?,
            level,
            root_dirid: u64_at(data, root_item::ROOT_DIRID)?,
            generation: u64_at(data, root_item::GENERATION)?,
        })
    }
}

/// One name in a directory
#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: Vec<u8>,
    /// What the name points at: an inode in this tree or a subvolume's root
    pub location: Key,
    /// `FT_*`, with the encrypted flag already masked off
    pub ftype: u8,
    /// Only an xattr has data
    pub data: Vec<u8>,
}

impl DirEntry {
    pub fn points_at_inode(&self) -> bool {
        self.location.kind == INODE_ITEM_KEY || self.location.kind == 0
    }
    pub fn points_at_subvolume(&self) -> bool {
        self.location.kind == ROOT_ITEM_KEY
    }
}

/// Decodes every record in one `DIR_ITEM`, `DIR_INDEX`, or `XATTR_ITEM`;
/// colliding names land in one item.
///
/// Checks are `check_dir_item` from `tree-checker.c`. For a `DIR_ITEM` or
/// `XATTR_ITEM`, the key's offset must be the hash of the name following
pub fn parse_dir_records(key: &Key, item: &[u8]) -> Result<Vec<DirEntry>, BtrfsError> {
    let mut out = Vec::new();
    let mut cur = 0usize;

    while cur < item.len() {
        if cur + dir_item::SIZE_OF > item.len() {
            return Err(BtrfsError::Corrupt);
        }
        let location = Key::read(item, cur + dir_item::LOCATION)?;
        let data_len = u16_at(item, cur + dir_item::DATA_LEN)? as usize;
        let name_len = u16_at(item, cur + dir_item::NAME_LEN)? as usize;
        let raw_type = u8_at(item, cur + dir_item::TYPE)?;
        let ftype = raw_type & !FT_ENCRYPTED;

        // A name points at an inode in this tree or a subvolume, nothing else.
        // Type 0 is what an old volume wrote for an inode
        if !(location.kind == ROOT_ITEM_KEY
            || location.kind == INODE_ITEM_KEY
            || location.kind == 0)
        {
            return Err(BtrfsError::Corrupt);
        }
        if ftype == FT_UNKNOWN || ftype >= FT_MAX {
            return Err(BtrfsError::Corrupt);
        }
        // The xattr type and xattr key imply each other
        let is_xattr_key = key.kind == XATTR_ITEM_KEY;
        if is_xattr_key != (ftype == FT_XATTR) {
            return Err(BtrfsError::Corrupt);
        }
        if name_len > NAME_LEN_MAX {
            return Err(BtrfsError::Corrupt);
        }
        // Only an attribute has data; otherwise the next record would be
        // misplaced
        if data_len != 0 && ftype != FT_XATTR {
            return Err(BtrfsError::Corrupt);
        }

        let total = dir_item::SIZE_OF + name_len + data_len;
        if cur + total > item.len() {
            return Err(BtrfsError::Corrupt);
        }

        let name_at = cur + dir_item::SIZE_OF;
        let name = &item[name_at..name_at + name_len];

        // The key's offset is this name's hash, for the two hash-keyed kinds
        if key.kind == DIR_ITEM_KEY || key.kind == XATTR_ITEM_KEY {
            if name_hash(name) != key.offset {
                return Err(BtrfsError::Corrupt);
            }
        }

        let mut name_vec = Vec::new();
        name_vec
            .try_reserve_exact(name_len)
            .map_err(|_| BtrfsError::OutOfMemory)?;
        name_vec.extend_from_slice(name);

        let mut data_vec = Vec::new();
        if data_len > 0 {
            data_vec
                .try_reserve_exact(data_len)
                .map_err(|_| BtrfsError::OutOfMemory)?;
            data_vec.extend_from_slice(&item[name_at + name_len..name_at + name_len + data_len]);
        }

        out.try_reserve(1).map_err(|_| BtrfsError::OutOfMemory)?;
        out.push(DirEntry { name: name_vec, location, ftype, data: data_vec });
        cur += total;
    }
    Ok(out)
}
