const fn generate_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0x82F63B78;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static TABLE: [u32; 256] = generate_table();

pub fn crc32c(initial: u32, data: &[u8]) -> u32 {
    let mut crc = !initial;
    for &b in data {
        let idx = ((crc ^ b as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ TABLE[idx];
    }
    !crc
}

pub fn crc32c_le(seed: u32, data: &[u8]) -> u32 {
    crc32c(seed, data)
}

pub fn ext4_superblock_csum(uuid: &[u8], sb_data: &[u8]) -> u32 {
    let crc = crc32c(0xFFFFFFFF, uuid);
    crc32c(crc, sb_data)
}

pub fn ext4_group_desc_csum(uuid: &[u8], group: u32, gd_data: &[u8]) -> u16 {
    let crc = crc32c(0xFFFFFFFF, uuid);
    let gb = group.to_le_bytes();
    let crc = crc32c(crc, &gb);
    let result = crc32c(crc, gd_data);
    (result & 0xFFFF) as u16
}

pub fn ext4_inode_csum(uuid: &[u8], inode_num: u32, gen: u32, inode_data: &[u8]) -> u32 {
    let crc = crc32c(0xFFFFFFFF, uuid);
    let ino_bytes = inode_num.to_le_bytes();
    let crc = crc32c(crc, &ino_bytes);
    let gen_bytes = gen.to_le_bytes();
    let crc = crc32c(crc, &gen_bytes);
    crc32c(crc, inode_data)
}

pub fn ext4_extent_csum(uuid: &[u8], inode_num: u32, extent_data: &[u8]) -> u32 {
    let crc = crc32c(0xFFFFFFFF, uuid);
    let ino_bytes = inode_num.to_le_bytes();
    let crc = crc32c(crc, &ino_bytes);
    crc32c(crc, extent_data)
}

pub fn ext4_dirent_csum(uuid: &[u8], inode_num: u32, dir_data: &[u8]) -> u32 {
    let crc = crc32c(0xFFFFFFFF, uuid);
    let ino_bytes = inode_num.to_le_bytes();
    let crc = crc32c(crc, &ino_bytes);
    crc32c(crc, dir_data)
}

pub fn ext4_bitmap_csum(uuid: &[u8], bitmap_data: &[u8]) -> u32 {
    let crc = crc32c(0xFFFFFFFF, uuid);
    crc32c(crc, bitmap_data)
}
