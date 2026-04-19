use crate::miku_extfs::structs::*;
use crate::miku_extfs::{FsError, MikuFS};

pub const EXT2_MAX_DIR_ENTRIES: usize = 256;

// ext2/3/4 directory hash (half_md4-based)
// hash_version: 0=legacy, 1=half_md4, 2=tea, 3=half_md4_unsigned, 4=tea_unsigned
pub fn ext2_dx_hash(name: &[u8], hash_version: u8, seed: &[u32; 4]) -> u32 {
    match hash_version {
        0 => dx_hash_legacy(name),
        1 | 3 => dx_hash_half_md4(name, seed),
        2 | 4 => dx_hash_tea(name, seed),
        _ => dx_hash_half_md4(name, seed),
    }
}

fn dx_hash_legacy(name: &[u8]) -> u32 {
    let mut hash = 0x12A3FE2Du32;
    for &b in name {
        hash = (hash << 5).wrapping_add(hash).wrapping_add(b as u32);
    }
    hash & 0x7FFFFFFF
}

fn dx_hash_half_md4(name: &[u8], seed: &[u32; 4]) -> u32 {
    let mut buf = [0u32; 8];
    buf[0] = seed[0];
    buf[1] = seed[1];
    buf[2] = seed[2];
    buf[3] = seed[3];

    // pack name bytes into u32 words
    let mut i = 0;
    let mut word_idx = 0;
    while i < name.len() && word_idx < 8 {
        let b0 = name[i] as u32;
        let b1 = if i + 1 < name.len() { name[i + 1] as u32 } else { 0 };
        let b2 = if i + 2 < name.len() { name[i + 2] as u32 } else { 0 };
        let b3 = if i + 3 < name.len() { name[i + 3] as u32 } else { 0 };
        buf[word_idx] ^= b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);
        i += 4;
        word_idx += 1;
    }

    // simplified half_md4 transform
    let mut a = buf[0];
    let mut b = buf[1];
    let mut c = buf[2];
    let mut d = buf[3];

    for round in 0..4 {
        let data_idx = (round * 2) % 8;
        a = a.wrapping_add(
            (b & c) | (!b & d),
        ).wrapping_add(buf[data_idx]);
        a = a.rotate_left(3);

        d = d.wrapping_add(
            (a & b) | (!a & c),
        ).wrapping_add(buf[data_idx + 1]);
        d = d.rotate_left(7);
    }

    (a ^ b) & 0x7FFFFFFF
}

fn dx_hash_tea(name: &[u8], seed: &[u32; 4]) -> u32 {
    let mut a = seed[0];
    let mut b = seed[1];
    let mut c = seed[2];
    let mut d = seed[3];

    // pack name
    let mut words = [0u32; 8];
    let mut i = 0;
    let mut wi = 0;
    while i < name.len() && wi < 8 {
        let b0 = name[i] as u32;
        let b1 = if i + 1 < name.len() { name[i + 1] as u32 } else { 0 };
        let b2 = if i + 2 < name.len() { name[i + 2] as u32 } else { 0 };
        let b3 = if i + 3 < name.len() { name[i + 3] as u32 } else { 0 };
        words[wi] = b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);
        i += 4;
        wi += 1;
    }

    // TEA rounds
    let delta = 0x9E3779B9u32;
    let mut sum = 0u32;
    for _ in 0..16 {
        sum = sum.wrapping_add(delta);
        a = a.wrapping_add(
            (b.wrapping_shl(4).wrapping_add(words[0]))
                ^ (b.wrapping_add(sum))
                ^ (b.wrapping_shr(5).wrapping_add(words[1])),
        );
        b = b.wrapping_add(
            (a.wrapping_shl(4).wrapping_add(words[2]))
                ^ (a.wrapping_add(sum))
                ^ (a.wrapping_shr(5).wrapping_add(words[3])),
        );
    }

    (a ^ c) & 0x7FFFFFFF
}

impl MikuFS {
    pub fn read_dir(&mut self, inode: &Inode, entries: &mut [DirEntry]) -> Result<usize, FsError> {
        if !inode.is_directory() {
            return Err(FsError::NotDirectory);
        }

        let dir_size = inode.size() as usize;
        let bs = self.block_size as usize;
        let mut count = 0usize;
        let mut file_offset = 0usize;

        while file_offset < dir_size && count < entries.len() {
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

            while pos + 8 <= read_size && count < entries.len() {
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
                    entries[count] = entry;
                    count += 1;
                }

                pos += rec_len;
            }

            file_offset += bs;
        }

        Ok(count)
    }

    pub fn lookup(&mut self, dir_inode: &Inode, name: &str) -> Result<u32, FsError> {
        // try htree lookup first for indexed directories
        if dir_inode.has_flag(EXT4_INDEX_FL) && self.superblock.has_dir_index() {
            if let Ok(ino) = self.htree_lookup(dir_inode, name) {
                return Ok(ino);
            }
            // fall through to linear scan if htree fails
        }

        let mut entries = [const { DirEntry::empty() }; 256];
        let count = self.read_dir(dir_inode, &mut entries)?;
        let name_bytes = name.as_bytes();

        for i in 0..count {
            let entry = &entries[i];
            let elen = entry.name_len as usize;
            if elen == name_bytes.len() && &entry.name[..elen] == name_bytes {
                return Ok(entry.inode);
            }
        }

        Err(FsError::NotFound)
    }

    // ext3/4 htree (dx_root) indexed directory lookup
    fn htree_lookup(&mut self, dir_inode: &Inode, name: &str) -> Result<u32, FsError> {
        let bs = self.block_size as usize;

        // read root block (block 0 of directory)
        let root_phys = self.get_file_block(dir_inode, 0)?;
        if root_phys == 0 {
            return Err(FsError::NotFound);
        }
        let mut root_buf = [0u8; 4096];
        self.read_block_into(root_phys, &mut root_buf[..bs])?;

        // dx_root header starts after the fake . and .. entries
        // offset 24: dx_root_info
        let hash_version = root_buf[28]; // dx_root.info.hash_version
        let _info_len = root_buf[29];    // dx_root.info.info_length
        let indirect_levels = root_buf[30]; // dx_root.info.indirect_levels
        let _unused = root_buf[31];

        // dx_entry[] starts at offset 32 with dx_countlimit overlaying dx_entry[0].hash
        let _limit = u16::from_le_bytes([root_buf[32], root_buf[33]]) as usize;
        let count = u16::from_le_bytes([root_buf[34], root_buf[35]]) as usize;

        if count < 1 || count > 512 {
            return Err(FsError::CorruptedFs);
        }

        // compute hash for the name
        let seed = [
            self.superblock.hash_seed(0),
            self.superblock.hash_seed(1),
            self.superblock.hash_seed(2),
            self.superblock.hash_seed(3),
        ];
        let hash = ext2_dx_hash(name.as_bytes(), hash_version, &seed);

        // dx_entry[0].block (first leaf) at offset 36; high 4 bits reserved
        let mut target_block = u32::from_le_bytes([
            root_buf[36], root_buf[37], root_buf[38], root_buf[39],
        ]) & 0x0fffffff;

        // dx_entry[i] for i>=1 at offset 32+i*8 (hash at +0, block at +4)
        for i in 1..count {
            let off = 32 + i * 8;
            if off + 8 > bs { break; }
            let entry_hash = u32::from_le_bytes([
                root_buf[off], root_buf[off + 1], root_buf[off + 2], root_buf[off + 3],
            ]);
            let entry_block = u32::from_le_bytes([
                root_buf[off + 4], root_buf[off + 5], root_buf[off + 6], root_buf[off + 7],
            ]) & 0x0fffffff;
            if hash >= entry_hash {
                target_block = entry_block;
            } else {
                break;
            }
        }

        // handle one level of indirect if present
        // dx_node layout: fake_dirent[8] then dx_entry[] starting at offset 8
        // dx_countlimit overlays dx_entry[0].hash at offset 8
        if indirect_levels > 0 {
            let idx_phys = self.get_file_block(dir_inode, target_block)?;
            if idx_phys == 0 {
                return Err(FsError::NotFound);
            }
            let mut idx_buf = [0u8; 4096];
            self.read_block_into(idx_phys, &mut idx_buf[..bs])?;
            let _il = u16::from_le_bytes([idx_buf[8], idx_buf[9]]) as usize;
            let ic = u16::from_le_bytes([idx_buf[10], idx_buf[11]]) as usize;
            // dx_entry[0].block at offset 12
            target_block = u32::from_le_bytes([
                idx_buf[12], idx_buf[13], idx_buf[14], idx_buf[15],
            ]) & 0x0fffffff;
            for i in 1..ic.min(512) {
                let off = 8 + i * 8;
                if off + 8 > bs { break; }
                let eh = u32::from_le_bytes([idx_buf[off], idx_buf[off+1], idx_buf[off+2], idx_buf[off+3]]);
                let eb = u32::from_le_bytes([idx_buf[off+4], idx_buf[off+5], idx_buf[off+6], idx_buf[off+7]]) & 0x0fffffff;
                if hash >= eh {
                    target_block = eb;
                } else {
                    break;
                }
            }
        }

        // now read the leaf directory block and scan it
        let leaf_phys = self.get_file_block(dir_inode, target_block)?;
        if leaf_phys == 0 {
            return Err(FsError::NotFound);
        }
        let mut leaf_buf = [0u8; 4096];
        self.read_block_into(leaf_phys, &mut leaf_buf[..bs])?;

        let name_bytes = name.as_bytes();
        let mut pos = 0;
        while pos + 8 <= bs {
            let rec_ino = u32::from_le_bytes([
                leaf_buf[pos], leaf_buf[pos+1], leaf_buf[pos+2], leaf_buf[pos+3],
            ]);
            let rec_len = u16::from_le_bytes([leaf_buf[pos+4], leaf_buf[pos+5]]) as usize;
            let nlen = leaf_buf[pos+6] as usize;
            if rec_len == 0 || rec_len > bs { break; }
            if rec_ino != 0 && nlen == name_bytes.len()
                && pos + 8 + nlen <= bs
                && &leaf_buf[pos+8..pos+8+nlen] == name_bytes
            {
                return Ok(rec_ino);
            }
            pos += rec_len;
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
                current_ino = self.lookup(&inode, "..")?;
                continue;
            }

            let inode = self.read_inode(current_ino)?;
            if !inode.is_directory() {
                return Err(FsError::NotDirectory);
            }

            current_ino = self.lookup(&inode, component)?;
        }

        Ok(current_ino)
    }
}
