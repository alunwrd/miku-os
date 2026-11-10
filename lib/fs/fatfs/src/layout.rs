// FAT32 geometry and the sectors describing it. Arithmetic only.
//
// Microsoft's "FAT: General Overview of On-Disk Format" 1.03 for every number.

use crate::FatError;

pub const SECTOR: usize = 512;
/// Sectors before the first FAT: the boot sector and FSInfo, and their copies
pub const RESERVED: u32 = 32;
pub const FATS: u32 = 2;
pub const FSINFO_SECTOR: u32 = 1;
pub const BACKUP_SECTOR: u32 = 6;
/// A volume with fewer clusters is FAT16 to every reader, whatever its boot
/// sector says
pub const MIN_CLUSTERS: u32 = 65525;
pub const ROOT_CLUSTER: u32 = 2;
/// FAT[0]: the media byte in the low eight bits
pub const FAT_MEDIA: u32 = 0x0FFF_FFF8;
/// End of a chain
pub const EOC: u32 = 0x0FFF_FFFF;

pub const ATTR_VOLUME: u8 = 0x08;
pub const ATTR_DIR: u8 = 0x10;
pub const ATTR_ARCHIVE: u8 = 0x20;
pub const DIR_ENTRY: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub sectors: u32,
    pub sectors_per_cluster: u32,
    /// Sectors in one FAT
    pub fat_sectors: u32,
    pub clusters: u32,
}

impl Geometry {
    pub fn new(sectors: u64) -> Result<Self, FatError> {
        let sectors = u32::try_from(sectors).map_err(|_| FatError::TooBig)?;
        // The specification's table of cluster sizes for FAT32
        let sectors_per_cluster = match sectors {
            0..=66_600 => return Err(FatError::TooSmall),
            66_601..=532_480 => 1,
            532_481..=16_777_216 => 8,
            16_777_217..=33_554_432 => 16,
            33_554_433..=67_108_864 => 32,
            _ => 64,
        };
        // And its formula for the FAT's length, which may overshoot by a few
        // sectors and never undershoots
        let tmp1 = sectors - RESERVED;
        let tmp2 = (256 * sectors_per_cluster + FATS) / 2;
        let fat_sectors = tmp1.div_ceil(tmp2);
        let clusters = (sectors - RESERVED - FATS * fat_sectors) / sectors_per_cluster;
        if clusters < MIN_CLUSTERS {
            return Err(FatError::TooSmall);
        }
        Ok(Self { sectors, sectors_per_cluster, fat_sectors, clusters })
    }

    /// First sector of FAT `copy`, from the start of the volume
    pub fn fat_lba(&self, copy: u32) -> u32 {
        RESERVED + copy * self.fat_sectors
    }

    /// First sector of cluster 2
    pub fn data_lba(&self) -> u32 {
        RESERVED + FATS * self.fat_sectors
    }

    pub fn cluster_lba(&self, cluster: u32) -> u32 {
        self.data_lba() + (cluster - 2) * self.sectors_per_cluster
    }

    pub fn cluster_bytes(&self) -> usize {
        self.sectors_per_cluster as usize * SECTOR
    }

    /// The boot sector. `hidden` is the volume's first sector on the disk
    pub fn boot_sector(&self, hidden: u32, serial: u32, label: &[u8; 11]) -> [u8; SECTOR] {
        let mut s = [0u8; SECTOR];
        // jmp to 0x5A, where the boot code would be
        s[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        s[3..11].copy_from_slice(b"MIKUOS  ");
        s[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
        s[13] = self.sectors_per_cluster as u8;
        s[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
        s[16] = FATS as u8;
        // 17..19 root entries, 19..21 16-bit sector count, 22..24 16-bit FAT
        // size: zero on FAT32
        s[21] = 0xF8;
        s[24..26].copy_from_slice(&63u16.to_le_bytes());
        s[26..28].copy_from_slice(&255u16.to_le_bytes());
        s[28..32].copy_from_slice(&hidden.to_le_bytes());
        s[32..36].copy_from_slice(&self.sectors.to_le_bytes());
        s[36..40].copy_from_slice(&self.fat_sectors.to_le_bytes());
        // 40..42 flags (both FATs live), 42..44 version 0.0
        s[44..48].copy_from_slice(&ROOT_CLUSTER.to_le_bytes());
        s[48..50].copy_from_slice(&(FSINFO_SECTOR as u16).to_le_bytes());
        s[50..52].copy_from_slice(&(BACKUP_SECTOR as u16).to_le_bytes());
        s[64] = 0x80;
        s[66] = 0x29;
        s[67..71].copy_from_slice(&serial.to_le_bytes());
        s[71..82].copy_from_slice(label);
        s[82..90].copy_from_slice(b"FAT32   ");
        // Not bootable by a BIOS: a jump to itself
        s[0x5A..0x5C].copy_from_slice(&[0xEB, 0xFE]);
        s[510] = 0x55;
        s[511] = 0xAA;
        s
    }
}

/// The FSInfo sector: free clusters, and where to look for the next one
pub fn fsinfo(free: u32, next: u32) -> [u8; SECTOR] {
    let mut s = [0u8; SECTOR];
    s[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    s[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    s[488..492].copy_from_slice(&free.to_le_bytes());
    s[492..496].copy_from_slice(&next.to_le_bytes());
    s[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
    s
}

/// One directory entry. `stamp` is (date, time) in the FAT encoding
pub fn dir_entry(name: &[u8; 11], attr: u8, cluster: u32, size: u32, stamp: (u16, u16)) -> [u8; DIR_ENTRY] {
    let (date, time) = stamp;
    let mut e = [0u8; DIR_ENTRY];
    e[0..11].copy_from_slice(name);
    e[11] = attr;
    e[14..16].copy_from_slice(&time.to_le_bytes());
    e[16..18].copy_from_slice(&date.to_le_bytes());
    e[18..20].copy_from_slice(&date.to_le_bytes());
    e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
    e[22..24].copy_from_slice(&time.to_le_bytes());
    e[24..26].copy_from_slice(&date.to_le_bytes());
    e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    e
}

/// Seconds since 1970 as a FAT (date, time). The format starts at 1980 and
/// ends at 2107; outside that it is the nearest end
pub fn dos_stamp(unix: u32) -> (u16, u16) {
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    let (y, m, d) = miku_base::date::civil_from_days(days);
    if y < 1980 {
        return (1 << 5 | 1, 0);
    }
    if y > 2107 {
        return (127 << 9 | 12 << 5 | 31, 23 << 11 | 59 << 5 | 29);
    }
    let date = ((y - 1980) as u16) << 9 | (m as u16) << 5 | d as u16;
    let time = ((secs / 3600) as u16) << 11 | ((secs / 60 % 60) as u16) << 5 | (secs % 60 / 2) as u16;
    (date, time)
}
