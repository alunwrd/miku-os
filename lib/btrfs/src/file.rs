// A file's bytes
//
// An inode's contents are `EXTENT_DATA` items keyed by file position:
//
//   INLINE     bytes in the item, after a 21-byte header. Only at file offset 0, for a file fitting in a leaf
//   REG        bytes on disk. `disk_bytenr` names the extent, `offset` how far in this record starts,
//              and `num_bytes` how much it covers. A `disk_bytenr` of zero is a hole
//   PREALLOC   reserved, never written, read as zeros; the disk holds stale data
//
// With `NO_HOLES`, a mkfs default, a gap has no record, so the loop below
// tracks position and does not assume contiguous items.
//
// Checks are `check_extent_data_item` from `tree-checker.c`, chiefly: an
// uncompressed inline extent's `ram_bytes` equals the item size minus the
// header, and a regular extent's item is exactly the struct.

use crate::error::BtrfsError;
use crate::fs_tree::EXTENT_CSUM_KEY;
use crate::structs::*;

/// Field offsets in `struct btrfs_file_extent_item`
pub mod fe {
    pub const GENERATION: usize = 0;
    pub const RAM_BYTES: usize = 8;
    pub const COMPRESSION: usize = 16;
    pub const ENCRYPTION: usize = 17;
    pub const OTHER_ENCODING: usize = 18;
    pub const TYPE: usize = 20;
    pub const DISK_BYTENR: usize = 21;
    pub const DISK_NUM_BYTES: usize = 29;
    pub const OFFSET: usize = 37;
    pub const NUM_BYTES: usize = 45;
    /// The whole struct, the exact size of a regular or preallocated extent's
    /// item
    pub const SIZE_OF: usize = 53;
    /// Where an inline extent's bytes begin: the offset of `disk_bytenr`, as it
    /// overlays the rest of the struct
    pub const INLINE_DATA_START: usize = 21;
}

pub const FILE_EXTENT_INLINE: u8 = 0;
pub const FILE_EXTENT_REG: u8 = 1;
pub const FILE_EXTENT_PREALLOC: u8 = 2;
pub const FILE_EXTENT_TYPES: u8 = 3;

/// The checksum tree's objectid: -10, outside the range a file can have
pub const EXTENT_CSUM_OBJECTID: u64 = u64::MAX - 9;

/// crc32c is four bytes, and this driver accepts no other algorithm
pub const CSUM_BYTES: usize = 4;

/// One `EXTENT_DATA` record, decoded
#[derive(Clone, Copy, Debug)]
pub enum Extent {
    /// Bytes in the item. The range is the data's offset within the item
    Inline { ram_bytes: u64, data_at: usize, data_len: usize },
    /// Nothing is there. Reads as zeros
    Hole { num_bytes: u64 },
    /// On disk at `logical`, for `num_bytes`
    Regular { logical: u64, num_bytes: u64, disk_bytenr: u64, disk_num_bytes: u64 },
    /// Reserved and never written: reads as zeros
    Prealloc { num_bytes: u64 },
}

impl Extent {
    /// How much of the file this record covers
    pub fn covers(&self) -> u64 {
        match self {
            Extent::Inline { ram_bytes, .. } => *ram_bytes,
            Extent::Hole { num_bytes } => *num_bytes,
            Extent::Regular { num_bytes, .. } => *num_bytes,
            Extent::Prealloc { num_bytes } => *num_bytes,
        }
    }
}

/// Decodes an `EXTENT_DATA` item. `key.offset` is where this record starts in
/// the file; an inline extent is legal only at zero
pub fn parse_extent(key: &Key, item: &[u8], sectorsize: u32) -> Result<Extent, BtrfsError> {
    if key.offset % sectorsize as u64 != 0 {
        return Err(BtrfsError::Corrupt);
    }
    // Enough for the header, so the type byte is valid
    if item.len() < fe::INLINE_DATA_START {
        return Err(BtrfsError::Corrupt);
    }

    let ty = u8_at(item, fe::TYPE)?;
    if ty >= FILE_EXTENT_TYPES {
        return Err(BtrfsError::Corrupt);
    }

    // Compression is refused at mount by the feature bit, and here, since an
    // extent claims it independently
    if u8_at(item, fe::COMPRESSION)? != 0 {
        return Err(BtrfsError::Unsupported);
    }
    if u8_at(item, fe::ENCRYPTION)? != 0 {
        return Err(BtrfsError::Unsupported);
    }
    if u16_at(item, fe::OTHER_ENCODING)? != 0 {
        return Err(BtrfsError::Unsupported);
    }

    let ram_bytes = u64_at(item, fe::RAM_BYTES)?;

    if ty == FILE_EXTENT_INLINE {
        // An inline extent is the whole file, so at the start
        if key.offset != 0 {
            return Err(BtrfsError::Corrupt);
        }
        // The item size says how many bytes there are, and must match
        // `ram_bytes`
        let expect = fe::INLINE_DATA_START
            .checked_add(ram_bytes as usize)
            .ok_or(BtrfsError::Corrupt)?;
        if item.len() != expect {
            return Err(BtrfsError::Corrupt);
        }
        return Ok(Extent::Inline {
            ram_bytes,
            data_at: fe::INLINE_DATA_START,
            data_len: ram_bytes as usize,
        });
    }

    // Everything else is exactly the fixed struct
    if item.len() != fe::SIZE_OF {
        return Err(BtrfsError::Corrupt);
    }

    let disk_bytenr = u64_at(item, fe::DISK_BYTENR)?;
    let disk_num_bytes = u64_at(item, fe::DISK_NUM_BYTES)?;
    let offset = u64_at(item, fe::OFFSET)?;
    let num_bytes = u64_at(item, fe::NUM_BYTES)?;

    for v in [ram_bytes, disk_bytenr, disk_num_bytes, offset, num_bytes] {
        if v % sectorsize as u64 != 0 {
            return Err(BtrfsError::Corrupt);
        }
    }
    // A record whose end wraps would cover the whole file
    if key.offset.checked_add(num_bytes).is_none() {
        return Err(BtrfsError::Corrupt);
    }

    if ty == FILE_EXTENT_PREALLOC {
        return Ok(Extent::Prealloc { num_bytes });
    }

    // A regular extent with no extent behind it is a hole when `NO_HOLES` is
    // off
    if disk_bytenr == 0 {
        return Ok(Extent::Hole { num_bytes });
    }

    // This record may start part-way into its extent, letting two snapshots
    // share the middle of one
    let logical = disk_bytenr
        .checked_add(offset)
        .ok_or(BtrfsError::Corrupt)?;
    // ...and it must stay inside it
    if offset
        .checked_add(num_bytes)
        .ok_or(BtrfsError::Corrupt)?
        > disk_num_bytes
    {
        return Err(BtrfsError::Corrupt);
    }

    Ok(Extent::Regular { logical, num_bytes, disk_bytenr, disk_num_bytes })
}

/// The key under which the checksum of the sector at `logical` is filed
pub fn csum_key(logical: u64) -> Key {
    Key {
        objectid: EXTENT_CSUM_OBJECTID,
        kind: EXTENT_CSUM_KEY,
        offset: logical,
    }
}
