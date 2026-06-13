use crate::vfs::types::*;

pub struct DevFs;

impl DevFs {
    pub const fn new() -> Self {
        Self
    }
}

/// Major 8 = raw block devices (Linux's sd major), minor = dev * 16 + part
pub const BLOCK_MAJOR: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevType {
    Null,
    Zero,
    Random,
    Console,
    /// Raw block device node /dev/blkN (part 0) or /dev/blkNpM (part 1-15),
    /// byte-addressable on top of the block layer
    Block { dev: u8, part: u8 },
}

impl DevType {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "null" => Some(Self::Null),
            "zero" => Some(Self::Zero),
            "random" | "urandom" => Some(Self::Random),
            "console" => Some(Self::Console),
            _ => Self::parse_block_name(name),
        }
    }

    /// "blkN" or "blkNpM" with N in 0..=7, M in 1..=15
    fn parse_block_name(name: &str) -> Option<Self> {
        let rest = name.strip_prefix("blk")?;
        let (dev_s, part_s) = match rest.find('p') {
            Some(i) => (&rest[..i], Some(&rest[i + 1..])),
            None => (rest, None),
        };
        let dev: u8 = dev_s.parse().ok()?;
        if dev as usize >= crate::vfs::types::MAX_BLOCK_DEVICES {
            return None;
        }
        let part: u8 = match part_s {
            Some(s) => {
                let p = s.parse().ok()?;
                if p == 0 || p > 15 { return None; }
                p
            }
            None => 0,
        };
        Some(Self::Block { dev, part })
    }

    pub fn major(&self) -> u8 {
        match self {
            Self::Null | Self::Zero | Self::Random => 1,
            Self::Console => 5,
            Self::Block { .. } => BLOCK_MAJOR,
        }
    }

    pub fn minor(&self) -> u8 {
        match self {
            Self::Null => 3,
            Self::Zero => 5,
            Self::Random => 8,
            Self::Console => 1,
            Self::Block { dev, part } => dev * 16 + part,
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Null => "null device (discards all)",
            Self::Zero => "zero device (reads zeros)",
            Self::Random => "pseudo-random generator",
            Self::Console => "system console",
            Self::Block { .. } => "raw block device",
        }
    }
}

use core::sync::atomic::{AtomicU32, Ordering};

static RANDOM_STATE: AtomicU32 = AtomicU32::new(0xDEADBEEF);

fn next_random() -> u8 {
    let mut state = RANDOM_STATE.load(Ordering::Relaxed);
    state ^= state << 13;
    state ^= state >> 17;
    state ^= state << 5;
    RANDOM_STATE.store(state, Ordering::Relaxed);
    state as u8
}

pub fn dev_read(dev_type: DevType, buf: &mut [u8], offset: u64) -> VfsResult<usize> {
    match dev_type {
        DevType::Null => Ok(0),
        DevType::Zero => {
            let len = buf.len();
            for b in buf.iter_mut() {
                *b = 0;
            }
            Ok(len)
        }
        DevType::Random => {
            let len = buf.len();
            for b in buf.iter_mut() {
                *b = next_random();
            }
            Ok(len)
        }
        DevType::Console => Ok(0),
        DevType::Block { dev, part } => block_read(dev, part, buf, offset),
    }
}

/// Byte-granular read from a raw block node, clamped at the end of the
/// disk/partition (so reads return 0 at EOF like a regular file)
fn block_read(dev: u8, part: u8, buf: &mut [u8], offset: u64) -> VfsResult<usize> {
    let Some((base, sectors)) = crate::block::node_range(dev, part) else {
        return Err(VfsError::NotFound);
    };
    let size = sectors * 512;
    if offset >= size {
        return Ok(0);
    }
    let want = (buf.len() as u64).min(size - offset) as usize;

    let mut tmp = [0u8; 4096];
    let mut done = 0usize;
    while done < want {
        let pos = offset + done as u64;
        let sector = base + pos / 512;
        let in_off = (pos % 512) as usize;
        let chunk = (want - done).min(tmp.len() - in_off);
        let nsec = ((in_off + chunk + 511) / 512) as u32;
        crate::block::read(dev, sector, nsec, &mut tmp[..nsec as usize * 512])
            .map_err(|_| VfsError::IoError)?;
        buf[done..done + chunk].copy_from_slice(&tmp[in_off..in_off + chunk]);
        done += chunk;
    }
    Ok(want)
}

/// Byte-granular write to a raw block node; unaligned edges read-modify-
/// write their sectors. Writing past the end reports 'NoSpace'
fn block_write(dev: u8, part: u8, data: &[u8], offset: u64) -> VfsResult<usize> {
    let Some((base, sectors)) = crate::block::node_range(dev, part) else {
        return Err(VfsError::NotFound);
    };
    let size = sectors * 512;
    if offset >= size {
        return Err(VfsError::NoSpace);
    }
    let want = (data.len() as u64).min(size - offset) as usize;

    let mut tmp = [0u8; 4096];
    let mut done = 0usize;
    while done < want {
        let pos = offset + done as u64;
        let sector = base + pos / 512;
        let in_off = (pos % 512) as usize;
        let chunk = (want - done).min(tmp.len() - in_off);
        let nsec = ((in_off + chunk + 511) / 512) as u32;

        if in_off == 0 && chunk % 512 == 0 {
            crate::block::write(dev, sector, nsec, &data[done..done + chunk])
                .map_err(|_| VfsError::IoError)?;
        } else {
            crate::block::read(dev, sector, nsec, &mut tmp[..nsec as usize * 512])
                .map_err(|_| VfsError::IoError)?;
            tmp[in_off..in_off + chunk].copy_from_slice(&data[done..done + chunk]);
            crate::block::write(dev, sector, nsec, &tmp[..nsec as usize * 512])
                .map_err(|_| VfsError::IoError)?;
        }
        done += chunk;
    }
    Ok(want)
}

pub fn dev_write(dev_type: DevType, buf: &[u8], offset: u64) -> VfsResult<usize> {
    match dev_type {
        DevType::Null | DevType::Zero | DevType::Random => Ok(buf.len()),
        DevType::Block { dev, part } => block_write(dev, part, buf, offset),
        DevType::Console => {
            for &b in buf {
                if b >= 0x20 && b <= 0x7E {
                    crate::print!("{}", b as char);
                } else if b == b'\n' {
                    crate::println!();
                } else if b == b'\r' {
                } else if b == b'\t' {
                    crate::print!("    ");
                }
            }
            Ok(buf.len())
        }
    }
}

pub fn dev_type_from_node(major: u8, minor: u8) -> Option<DevType> {
    match (major, minor) {
        (1, 3) => Some(DevType::Null),
        (1, 5) => Some(DevType::Zero),
        (1, 8) => Some(DevType::Random),
        (5, 1) => Some(DevType::Console),
        (BLOCK_MAJOR, m) => Some(DevType::Block { dev: m / 16, part: m % 16 }),
        _ => None,
    }
}

pub const DEV_ENTRIES: &[(&str, DevType)] = &[
    ("null", DevType::Null),
    ("zero", DevType::Zero),
    ("random", DevType::Random),
    ("urandom", DevType::Random),
    ("console", DevType::Console),
];
