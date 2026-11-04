use crate::structs::*;
use crate::{FsError, MikuFS};
use crate::host::ExtHost;

/// Most entries one `read_dir` returns. It fills a caller's slice and stops
/// when full, so a directory larger than the array is truncated.
/// `read_dir_each` sees every entry
pub const EXT2_MAX_DIR_ENTRIES: usize = 256;

// The directory hash
//
// An htree directory is indexed by the hash of each name, which must match
// Linux's bit for bit in both directions.
//
// The algorithms are from Linux's fs/ext4/hash.c, and tests check them against
// `debugfs dx_hash` vectors.

/// Which hash a directory's index was built with. Stored in the dx_root, so the
/// directory decides
pub const DX_HASH_LEGACY: u8 = 0;
pub const DX_HASH_HALF_MD4: u8 = 1;
pub const DX_HASH_TEA: u8 = 2;
pub const DX_HASH_LEGACY_UNSIGNED: u8 = 3;
pub const DX_HASH_HALF_MD4_UNSIGNED: u8 = 4;
pub const DX_HASH_TEA_UNSIGNED: u8 = 5;

/// Marks the end of a hash-ordered readdir. A colliding real hash is nudged
/// down by one
const EXT4_HTREE_EOF_32BIT: u32 = 0x7fff_ffff;

const TEA_DELTA: u32 = 0x9E37_79B9;

/// The MD4 round constants
const K1: u32 = 0;
const K2: u32 = 0x5A82_7999;
const K3: u32 = 0x6ED9_EBA1;

/// The initial state, when the volume carries no seed of its own
const MD4_INIT: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];

/// The major hash, which index slot, and the minor one, ordering names in one
/// slot
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirHash {
    pub hash: u32,
    pub minor: u32,
}

#[inline]
fn f(x: u32, y: u32, z: u32) -> u32 { z ^ (x & (y ^ z)) }
#[inline]
fn g(x: u32, y: u32, z: u32) -> u32 { (x & y).wrapping_add((x ^ y) & z) }
#[inline]
fn h(x: u32, y: u32, z: u32) -> u32 { x ^ y ^ z }

/// One MD4 round: `a += fn(b,c,d) + x; a = rol(a, s)`
macro_rules! round {
    ($fun:ident, $a:expr, $b:expr, $c:expr, $d:expr, $x:expr, $s:expr) => {
        $a = $a.wrapping_add($fun($b, $c, $d)).wrapping_add($x).rotate_left($s);
    };
}

fn half_md4_transform(buf: &mut [u32; 4], input: &[u32; 8]) {
    let (mut a, mut b, mut c, mut d) = (buf[0], buf[1], buf[2], buf[3]);

    round!(f, a, b, c, d, input[0].wrapping_add(K1), 3);
    round!(f, d, a, b, c, input[1].wrapping_add(K1), 7);
    round!(f, c, d, a, b, input[2].wrapping_add(K1), 11);
    round!(f, b, c, d, a, input[3].wrapping_add(K1), 19);
    round!(f, a, b, c, d, input[4].wrapping_add(K1), 3);
    round!(f, d, a, b, c, input[5].wrapping_add(K1), 7);
    round!(f, c, d, a, b, input[6].wrapping_add(K1), 11);
    round!(f, b, c, d, a, input[7].wrapping_add(K1), 19);

    round!(g, a, b, c, d, input[1].wrapping_add(K2), 3);
    round!(g, d, a, b, c, input[3].wrapping_add(K2), 5);
    round!(g, c, d, a, b, input[5].wrapping_add(K2), 9);
    round!(g, b, c, d, a, input[7].wrapping_add(K2), 13);
    round!(g, a, b, c, d, input[0].wrapping_add(K2), 3);
    round!(g, d, a, b, c, input[2].wrapping_add(K2), 5);
    round!(g, c, d, a, b, input[4].wrapping_add(K2), 9);
    round!(g, b, c, d, a, input[6].wrapping_add(K2), 13);

    round!(h, a, b, c, d, input[3].wrapping_add(K3), 3);
    round!(h, d, a, b, c, input[7].wrapping_add(K3), 9);
    round!(h, c, d, a, b, input[2].wrapping_add(K3), 11);
    round!(h, b, c, d, a, input[6].wrapping_add(K3), 15);
    round!(h, a, b, c, d, input[1].wrapping_add(K3), 3);
    round!(h, d, a, b, c, input[5].wrapping_add(K3), 9);
    round!(h, c, d, a, b, input[0].wrapping_add(K3), 11);
    round!(h, b, c, d, a, input[4].wrapping_add(K3), 15);

    buf[0] = buf[0].wrapping_add(a);
    buf[1] = buf[1].wrapping_add(b);
    buf[2] = buf[2].wrapping_add(c);
    buf[3] = buf[3].wrapping_add(d);
}

fn tea_transform(buf: &mut [u32; 4], input: &[u32; 4]) {
    let mut sum = 0u32;
    let (mut b0, mut b1) = (buf[0], buf[1]);
    let (a, b, c, d) = (input[0], input[1], input[2], input[3]);

    for _ in 0..16 {
        sum = sum.wrapping_add(TEA_DELTA);
        b0 = b0.wrapping_add(
            ((b1 << 4).wrapping_add(a)) ^ (b1.wrapping_add(sum)) ^ ((b1 >> 5).wrapping_add(b)),
        );
        b1 = b1.wrapping_add(
            ((b0 << 4).wrapping_add(c)) ^ (b0.wrapping_add(sum)) ^ ((b0 >> 5).wrapping_add(d)),
        );
    }

    buf[0] = buf[0].wrapping_add(b0);
    buf[1] = buf[1].wrapping_add(b1);
}

/// Packs up to `num` words of the name, padding with the name's length.
/// `signed` reproduces C's `signed char` promotion, where the two hash version
/// families differ for a byte above 0x7F. Both exist on real filesystems
fn str2hashbuf(name: &[u8], buf: &mut [u32; 8], num: usize, signed: bool) {
    let mut pad = name.len() as u32;
    pad |= pad << 8;
    pad |= pad << 16;

    let mut val = pad;
    let len = name.len().min(num * 4);
    let mut out = 0usize;
    let mut remaining = num;

    for i in 0..len {
        let byte = if signed {
            (name[i] as i8) as i32 as u32
        } else {
            name[i] as u32
        };
        val = byte.wrapping_add(val << 8);
        if i % 4 == 3 {
            buf[out] = val;
            out += 1;
            val = pad;
            remaining -= 1;
        }
    }
    if remaining > 0 {
        remaining -= 1;
        buf[out] = val;
        out += 1;
    }
    while remaining > 0 {
        remaining -= 1;
        buf[out] = pad;
        out += 1;
    }
}

fn dx_hack_hash(name: &[u8], signed: bool) -> u32 {
    let mut hash0 = 0x12a3_fe2du32;
    let mut hash1 = 0x37ab_e8f9u32;
    for &b in name {
        let c = if signed { (b as i8) as i32 as u32 } else { b as u32 };
        let hash = hash1.wrapping_add(hash0 ^ c.wrapping_mul(7152373));
        let hash = if hash & 0x8000_0000 != 0 { hash.wrapping_sub(0x7fff_ffff) } else { hash };
        hash1 = hash0;
        hash0 = hash;
    }
    hash0 << 1
}

/// Hashes a directory entry name as ext4 does. `seed` is the volume's
/// `s_hash_seed`; an all-zero seed means the standard MD4 initial state. Linux
/// stops copying the seed at the first zero word, and so does this
pub fn ext2_dx_hash_full(name: &[u8], hash_version: u8, seed: &[u32; 4]) -> DirHash {
    let mut buf = MD4_INIT;
    if seed.iter().any(|&w| w != 0) {
        for i in 0..4 {
            if seed[i] == 0 { break; }
            buf[i] = seed[i];
        }
    }

    let (hash, minor) = match hash_version {
        DX_HASH_LEGACY => (dx_hack_hash(name, true), 0),
        DX_HASH_LEGACY_UNSIGNED => (dx_hack_hash(name, false), 0),

        DX_HASH_HALF_MD4 | DX_HASH_HALF_MD4_UNSIGNED => {
            let signed = hash_version == DX_HASH_HALF_MD4;
            let mut p = name;
            loop {
                let mut input = [0u32; 8];
                str2hashbuf(p, &mut input, 8, signed);
                half_md4_transform(&mut buf, &input);
                if p.len() <= 32 { break; }
                p = &p[32..];
            }
            (buf[1], buf[2])
        }

        DX_HASH_TEA | DX_HASH_TEA_UNSIGNED => {
            let signed = hash_version == DX_HASH_TEA;
            let mut p = name;
            loop {
                let mut wide = [0u32; 8];
                str2hashbuf(p, &mut wide, 4, signed);
                let input = [wide[0], wide[1], wide[2], wide[3]];
                tea_transform(&mut buf, &input);
                if p.len() <= 16 { break; }
                p = &p[16..];
            }
            // The minor hash is buf[1]: TEA_transform writes only buf[0] and
            // buf[1]
            (buf[0], buf[1])
        }

        // An index with an unknown hash cannot be searched; the caller falls
        // back to a linear scan
        _ => (0, 0),
    };

    // The low bit is reserved as a collision marker, and the top value marks
    // end-of-directory
    let mut hash = hash & !1;
    if hash == EXT4_HTREE_EOF_32BIT << 1 {
        hash = (EXT4_HTREE_EOF_32BIT - 1) << 1;
    }
    DirHash { hash, minor }
}

/// Only the major hash, for picking an index slot
pub fn ext2_dx_hash(name: &[u8], hash_version: u8, seed: &[u32; 4]) -> u32 {
    ext2_dx_hash_full(name, hash_version, seed).hash
}

impl<H: ExtHost> MikuFS<H> {
    /// Passes every entry of a directory to `f`, one at a time, using a
    /// callback in place of tens of kilobytes of stack. Returning `false`
    /// stops. Returns the number of entries passed
    pub fn read_dir_each<F>(&mut self, inode: &Inode, mut f: F) -> Result<usize, FsError>
    where
        F: FnMut(&DirEntry) -> bool,
    {
        if !inode.is_directory() {
            return Err(FsError::NotDirectory);
        }

        let dir_size = inode.size() as usize;
        let bs = self.block_size as usize;
        let mut count = 0usize;
        let mut file_offset = 0usize;

        while file_offset < dir_size {
            let logical_block = (file_offset / bs) as u32;
            let phys_block = self.get_file_block(inode, logical_block)?;

            if phys_block == 0 {
                file_offset += bs;
                continue;
            }

            let mut block_buf = [0u8; 4096];
            let read_size = bs.min(4096);
            self.read_block_into(phys_block, &mut block_buf[..read_size])?;

            let mut pos = 0usize;
            while pos + 8 <= read_size {
                let abs_pos = file_offset + pos;
                if abs_pos >= dir_size {
                    break;
                }

                let raw_inode = u32::from_le_bytes([
                    block_buf[pos],
                    block_buf[pos + 1],
                    block_buf[pos + 2],
                    block_buf[pos + 3],
                ]);
                let rec_len = u16::from_le_bytes([block_buf[pos + 4], block_buf[pos + 5]]) as usize;
                let name_len = block_buf[pos + 6] as usize;
                let file_type = block_buf[pos + 7];

                if rec_len == 0 || rec_len > bs {
                    break;
                }

                if raw_inode != 0 && name_len > 0 && pos + 8 + name_len <= read_size {
                    let mut entry = DirEntry::empty();
                    entry.inode = raw_inode;
                    entry.file_type = file_type;
                    let copy_len = name_len.min(MAX_NAME);
                    entry.name_len = copy_len as u8;
                    entry.name[..copy_len].copy_from_slice(&block_buf[pos + 8..pos + 8 + copy_len]);
                    count += 1;
                    if !f(&entry) {
                        return Ok(count);
                    }
                }

                pos += rec_len;
            }

            file_offset += bs;
        }

        Ok(count)
    }

    /// The first `entries.len()` entries, with no sign of whether there were
    /// more. To see the whole directory, use `read_dir_each`
    pub fn read_dir(&mut self, inode: &Inode, entries: &mut [DirEntry]) -> Result<usize, FsError> {
        let limit = entries.len();
        if limit == 0 {
            return Ok(0);
        }
        let mut at = 0usize;
        self.read_dir_each(inode, |e| {
            entries[at] = *e;
            at += 1;
            at < limit
        })?;
        Ok(at)
    }

    /// Finds a name in a directory. The inode number comes along because an
    /// indexed directory's blocks carry checksums keyed by it, and an unchecked
    /// index could follow a garbage block number
    pub fn lookup(
        &mut self,
        dir_ino: u32,
        dir_inode: &Inode,
        name: &str,
    ) -> Result<u32, FsError> {
        let indexed = self.dir_is_indexed(dir_inode);
        if indexed {
            match self.htree_lookup(dir_ino, dir_inode, name) {
                Ok(Some(ino)) => return Ok(ino),
                // The index walked cleanly and the name is not in the one leaf
                // that could hold it
                Ok(None) => return Err(FsError::NotFound),
                Err(_) => {
                    // The index could not be walked; reads every block instead
                    self.dx_fallbacks += 1;
                    crate::extlog!(
                        "[extfs] directory {} index unreadable - scanning instead",
                        dir_ino
                    );
                }
            }
        }

        // The whole directory: a scan stopping early would report "not found"
        let name_bytes = name.as_bytes();
        let mut found = 0u32;
        self.read_dir_each(dir_inode, |entry| {
            let elen = entry.name_len as usize;
            if elen == name_bytes.len() && &entry.name[..elen] == name_bytes {
                found = entry.inode;
                return false;
            }
            true
        })?;
        if found != 0 {
            return Ok(found);
        }

        Err(FsError::NotFound)
    }

    pub fn resolve_path(&mut self, path: &str) -> Result<u32, FsError> {
        let mut current_ino = EXT2_ROOT_INO;

        if path.is_empty() || path == "/" {
            return Ok(current_ino);
        }

        for component in path.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                let inode = self.read_inode(current_ino)?;
                current_ino = self.lookup(current_ino, &inode, "..")?;
                continue;
            }

            let inode = self.read_inode(current_ino)?;
            if !inode.is_directory() {
                return Err(FsError::NotDirectory);
            }

            current_ino = self.lookup(current_ino, &inode, component)?;
        }

        Ok(current_ino)
    }
}

/// Fits one entry into a directory block by taking slack from an existing
/// record; `false` when none has enough. Used by plain inserts, the leaf a
/// descent reaches, and each half of a split leaf.
///
/// `usable` is where real entries stop: on `metadata_csum` the last twelve
/// bytes are the checksum record
pub(crate) fn insert_into_dir_block(
    buf: &mut [u8],
    usable: usize,
    name: &[u8],
    ino: u32,
    file_type: u8,
) -> bool {
    let needed = (8 + name.len() + 3) / 4 * 4;
    let mut pos = 0usize;
    while pos + 8 <= usable {
        let rec_ino = u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
        let rec_len = u16::from_le_bytes([buf[pos + 4], buf[pos + 5]]) as usize;
        let rec_name_len = buf[pos + 6] as usize;
        if rec_len < 8 || rec_len > usable - pos {
            return false;
        }
        // A record with inode zero is unused, so all of it is available
        let held = if rec_ino == 0 {
            0
        } else {
            (8 + rec_name_len + 3) / 4 * 4
        };
        if rec_len < held {
            pos += rec_len;
            continue;
        }
        if rec_len - held >= needed {
            let at = pos + held;
            let remaining = rec_len - held;
            if held != 0 {
                buf[pos + 4..pos + 6].copy_from_slice(&(held as u16).to_le_bytes());
            }
            buf[at..at + 4].copy_from_slice(&ino.to_le_bytes());
            buf[at + 4..at + 6].copy_from_slice(&(remaining as u16).to_le_bytes());
            buf[at + 6] = name.len() as u8;
            buf[at + 7] = file_type;
            buf[at + 8..at + 8 + name.len()].copy_from_slice(name);
            return true;
        }
        pos += rec_len;
    }
    false
}
