use crate::ata::{AtaDrive, AtaError};
use spin::Mutex;

pub const GPT_SIGNATURE: u64     = 0x5452415020494645;
pub const GPT_REVISION: u32      = 0x00010000;
pub const GPT_HEADER_SIZE: u32   = 92;
pub const GPT_ENTRY_SIZE: usize  = 128;
pub const GPT_MAX_ENTRIES: usize = 128;
pub const GPT_ENTRIES_LBA: u32   = 2;
pub const GPT_HEADER_LBA: u32    = 1;
pub const GPT_FIRST_USABLE: u64  = 34;

pub const GUID_LINUX_FS: [u8; 16] = [
    0xAF,0x3D,0xC6,0x0F, 0x83,0x84, 0x72,0x47,
    0x8E,0x79, 0x3D,0x69,0xD8,0x47,0x7D,0xE4,
];

pub const GUID_LINUX_SWAP: [u8; 16] = [
    0x6D,0xFD,0x57,0x06, 0xAB,0xA4, 0xC4,0x43,
    0x84,0xE5, 0x09,0x33,0xC8,0x4B,0x4F,0x4F,
];

pub const GUID_EMPTY: [u8; 16] = [0u8; 16];

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct GptHeader {
    pub signature:                   u64,
    pub revision:                    u32,
    pub header_size:                 u32,
    pub header_crc32:                u32,
    pub reserved:                    u32,
    pub my_lba:                      u64,
    pub alternate_lba:               u64,
    pub first_usable_lba:            u64,
    pub last_usable_lba:             u64,
    pub disk_guid:                   [u8; 16],
    pub partition_entry_lba:         u64,
    pub num_partition_entries:       u32,
    pub size_of_partition_entry:     u32,
    pub partition_entry_array_crc32: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct GptEntry {
    pub type_guid:   [u8; 16],
    pub unique_guid: [u8; 16],
    pub start_lba:   u64,
    pub end_lba:     u64,
    pub attributes:  u64,
    pub name:        [u16; 36],
}

impl GptEntry {
    pub const fn empty() -> Self {
        Self {
            type_guid:   GUID_EMPTY,
            unique_guid: GUID_EMPTY,
            start_lba:   0,
            end_lba:     0,
            attributes:  0,
            name:        [0u16; 36],
        }
    }

    pub fn is_used(&self) -> bool {
        self.type_guid != GUID_EMPTY
    }

    pub fn is_swap(&self) -> bool {
        self.type_guid == GUID_LINUX_SWAP
    }

    pub fn size_sectors(&self) -> u64 {
        if self.end_lba >= self.start_lba { self.end_lba - self.start_lba + 1 } else { 0 }
    }

    pub fn size_mb(&self) -> u64 {
        self.size_sectors() * 512 / (1024 * 1024)
    }

    pub fn set_name(&mut self, s: &str) {
        self.name = [0u16; 36];
        for (i, c) in s.chars().take(35).enumerate() {
            self.name[i] = c as u16;
        }
    }

    pub fn name_str(&self, buf: &mut [u8; 36]) -> usize {
        let mut len = 0;
        for i in 0..36 {
            let c = self.name[i];
            if c == 0 { break; }
            buf[len] = if c < 128 { c as u8 } else { b'?' };
            len += 1;
        }
        len
    }

    pub fn type_name(&self) -> &'static str {
        if self.type_guid == GUID_LINUX_FS   { "Linux FS" }
        else if self.type_guid == GUID_LINUX_SWAP { "Linux Swap" }
        else                                      { "Unknown" }
    }
}

pub struct GptTable {
    pub header:        GptHeader,
    pub entries:       [GptEntry; GPT_MAX_ENTRIES],
    pub total_sectors: u32,
}

impl GptTable {
    pub const fn empty() -> Self {
        unsafe { core::mem::zeroed() }
    }

    pub fn first_free_slot(&self) -> Option<usize> {
        for i in 0..GPT_MAX_ENTRIES {
            if !self.entries[i].is_used() { return Some(i); }
        }
        None
    }

    pub fn next_free_lba(&self) -> u64 {
        let mut max_end = GPT_FIRST_USABLE;
        for e in &self.entries {
            if e.is_used() && e.end_lba >= max_end {
                max_end = e.end_lba + 1;
            }
        }
        max_end
    }

    pub fn last_usable_lba(&self) -> u64 {
        self.total_sectors as u64 - 34
    }
}

static GPT_CACHE: Mutex<GptTable> = Mutex::new(GptTable::empty());

const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut c = i;
        let mut j = 0;
        while j < 8 {
            if c & 1 != 0 { c = 0xEDB8_8320 ^ (c >> 1); } else { c >>= 1; }
            j += 1;
        }
        table[i as usize] = c;
        i += 1;
    }
    table
}

static CRC32_TABLE: [u32; 256] = make_crc32_table();

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = CRC32_TABLE[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

fn guid_pseudo(seed: u32) -> [u8; 16] {
    let mut g = [0u8; 16];
    let a = seed.wrapping_mul(0x9E37_79B9).wrapping_add(0x6C62_272E);
    let b = a.wrapping_mul(0x85EB_CA77).wrapping_add(0xC2B2_AE3D);
    g[0..4].copy_from_slice(&a.to_le_bytes());
    g[4..8].copy_from_slice(&b.to_le_bytes());
    g[8..12].copy_from_slice(&a.wrapping_add(b).to_le_bytes());
    g[12..16].copy_from_slice(&b.wrapping_sub(a).to_le_bytes());
    g[6] = (g[6] & 0x0F) | 0x40;
    g[8] = (g[8] & 0x3F) | 0x80;
    g
}

fn write_protective_mbr(drive: &mut AtaDrive, total_sectors: u32) -> Result<(), AtaError> {
    let mut mbr = [0u8; 512];
    mbr[446] = 0x00;
    mbr[447] = 0x00;
    mbr[448] = 0x02;
    mbr[449] = 0x00;
    mbr[450] = 0xEE;
    mbr[451] = 0xFF;
    mbr[452] = 0xFF;
    mbr[453] = 0xFF;
    mbr[454..458].copy_from_slice(&1u32.to_le_bytes());
    let size = total_sectors.saturating_sub(1);
    mbr[458..462].copy_from_slice(&size.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    drive.write_sector(0, &mbr)
}

fn gpt_header_to_buf(h: &GptHeader) -> [u8; 512] {
    let mut buf = [0u8; 512];
    buf[0..8].copy_from_slice(&h.signature.to_le_bytes());
    buf[8..12].copy_from_slice(&h.revision.to_le_bytes());
    buf[12..16].copy_from_slice(&h.header_size.to_le_bytes());
    buf[16..20].copy_from_slice(&h.header_crc32.to_le_bytes());
    buf[24..32].copy_from_slice(&h.my_lba.to_le_bytes());
    buf[32..40].copy_from_slice(&h.alternate_lba.to_le_bytes());
    buf[40..48].copy_from_slice(&h.first_usable_lba.to_le_bytes());
    buf[48..56].copy_from_slice(&h.last_usable_lba.to_le_bytes());
    buf[56..72].copy_from_slice(&h.disk_guid);
    buf[72..80].copy_from_slice(&h.partition_entry_lba.to_le_bytes());
    buf[80..84].copy_from_slice(&h.num_partition_entries.to_le_bytes());
    buf[84..88].copy_from_slice(&h.size_of_partition_entry.to_le_bytes());
    buf[88..92].copy_from_slice(&h.partition_entry_array_crc32.to_le_bytes());
    buf
}

fn entry_to_bytes(e: &GptEntry) -> [u8; GPT_ENTRY_SIZE] {
    let mut buf = [0u8; GPT_ENTRY_SIZE];
    buf[0..16].copy_from_slice(&e.type_guid);
    buf[16..32].copy_from_slice(&e.unique_guid);
    buf[32..40].copy_from_slice(&e.start_lba.to_le_bytes());
    buf[40..48].copy_from_slice(&e.end_lba.to_le_bytes());
    buf[48..56].copy_from_slice(&e.attributes.to_le_bytes());
    for i in 0..36 {
        let le = e.name[i].to_le_bytes();
        buf[56 + i * 2]     = le[0];
        buf[56 + i * 2 + 1] = le[1];
    }
    buf
}

fn bytes_to_entry(buf: &[u8]) -> GptEntry {
    let mut e = GptEntry::empty();
    e.type_guid.copy_from_slice(&buf[0..16]);
    e.unique_guid.copy_from_slice(&buf[16..32]);
    e.start_lba  = u64::from_le_bytes(buf[32..40].try_into().unwrap_or([0;8]));
    e.end_lba    = u64::from_le_bytes(buf[40..48].try_into().unwrap_or([0;8]));
    e.attributes = u64::from_le_bytes(buf[48..56].try_into().unwrap_or([0;8]));
    for i in 0..36 {
        e.name[i] = u16::from_le_bytes([buf[56 + i * 2], buf[56 + i * 2 + 1]]);
    }
    e
}

fn entries_crc(entries: &[GptEntry; GPT_MAX_ENTRIES]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for e in entries.iter() {
        let eb = entry_to_bytes(e);
        for &b in eb.iter() {
            crc = CRC32_TABLE[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
        }
    }
    !crc
}

fn header_crc(h: &GptHeader) -> u32 {
    let mut tmp = *h;
    tmp.header_crc32 = 0;
    let buf = gpt_header_to_buf(&tmp);
    crc32(&buf[0..GPT_HEADER_SIZE as usize])
}

fn write_entries(drive: &mut AtaDrive, entries: &[GptEntry; GPT_MAX_ENTRIES], base_lba: u32)
    -> Result<(), AtaError>
{
    let per_sec = 512 / GPT_ENTRY_SIZE;
    let mut buf = [0u8; 512];
    for chunk in 0..32u32 {
        for j in 0..per_sec {
            let idx = chunk as usize * per_sec + j;
            let eb = entry_to_bytes(&entries[idx]);
            buf[j * GPT_ENTRY_SIZE..(j + 1) * GPT_ENTRY_SIZE].copy_from_slice(&eb);
        }
        drive.write_sector(base_lba + chunk, &buf)?;
    }
    Ok(())
}

fn write_full_table(drive: &mut AtaDrive, tbl: &mut GptTable) -> Result<(), AtaError> {
    tbl.header.partition_entry_array_crc32 = entries_crc(&tbl.entries);
    tbl.header.header_crc32 = 0;
    tbl.header.header_crc32 = header_crc(&tbl.header);

    let hbuf = gpt_header_to_buf(&tbl.header);
    drive.write_sector(GPT_HEADER_LBA, &hbuf)?;
    write_entries(drive, &tbl.entries, GPT_ENTRIES_LBA)?;

    let mut backup = tbl.header;
    backup.my_lba            = tbl.total_sectors as u64 - 1;
    backup.alternate_lba     = GPT_HEADER_LBA as u64;
    backup.partition_entry_lba = tbl.total_sectors as u64 - 33;
    backup.header_crc32 = 0;
    backup.header_crc32 = header_crc(&backup);

    let bbuf = gpt_header_to_buf(&backup);
    drive.write_sector(tbl.total_sectors - 1, &bbuf)?;
    write_entries(drive, &tbl.entries, tbl.total_sectors - 33)?;

    Ok(())
}

pub fn gpt_init(mut drive: AtaDrive, total_sectors: u32) -> Result<(), AtaError> {
    write_protective_mbr(&mut drive, total_sectors)?;

    let disk_guid = guid_pseudo(total_sectors ^ 0xDEAD_BEEF);
    let last_usable = total_sectors as u64 - 34;
    let entries = [GptEntry::empty(); GPT_MAX_ENTRIES];

    let mut header = GptHeader {
        signature:                    GPT_SIGNATURE,
        revision:                     GPT_REVISION,
        header_size:                  GPT_HEADER_SIZE,
        header_crc32:                 0,
        reserved:                     0,
        my_lba:                       GPT_HEADER_LBA as u64,
        alternate_lba:                total_sectors as u64 - 1,
        first_usable_lba:             GPT_FIRST_USABLE,
        last_usable_lba:              last_usable,
        disk_guid,
        partition_entry_lba:          GPT_ENTRIES_LBA as u64,
        num_partition_entries:        GPT_MAX_ENTRIES as u32,
        size_of_partition_entry:      GPT_ENTRY_SIZE as u32,
        partition_entry_array_crc32:  entries_crc(&entries),
    };
    header.header_crc32 = header_crc(&header);

    let mut tbl = GptTable { header, entries, total_sectors };
    write_full_table(&mut drive, &mut tbl)?;

    let mut cache = GPT_CACHE.lock();
    cache.header  = tbl.header;
    cache.entries = tbl.entries;
    cache.total_sectors = total_sectors;

    crate::serial_println!("[gpt] init: {} sectors, usable LBAs {}-{}", total_sectors, GPT_FIRST_USABLE, last_usable);
    Ok(())
}

pub fn gpt_read(drive: &mut AtaDrive) -> Result<GptTable, GptReadError> {
    let mut buf = [0u8; 512];
    drive.read_sector(GPT_HEADER_LBA, &mut buf).map_err(GptReadError::Io)?;

    let sig = u64::from_le_bytes(buf[0..8].try_into().unwrap_or([0;8]));
    if sig != GPT_SIGNATURE { return Err(GptReadError::NotGpt); }

    let num_entries = u32::from_le_bytes(buf[80..84].try_into().unwrap_or([0;4])) as usize;
    let entry_size  = u32::from_le_bytes(buf[84..88].try_into().unwrap_or([0;4])) as usize;
    if entry_size != GPT_ENTRY_SIZE || num_entries > GPT_MAX_ENTRIES {
        return Err(GptReadError::InvalidFormat);
    }

    let header = GptHeader {
        signature:                   sig,
        revision:                    u32::from_le_bytes(buf[8..12].try_into().unwrap_or([0;4])),
        header_size:                 u32::from_le_bytes(buf[12..16].try_into().unwrap_or([0;4])),
        header_crc32:                u32::from_le_bytes(buf[16..20].try_into().unwrap_or([0;4])),
        reserved:                    0,
        my_lba:                      u64::from_le_bytes(buf[24..32].try_into().unwrap_or([0;8])),
        alternate_lba:               u64::from_le_bytes(buf[32..40].try_into().unwrap_or([0;8])),
        first_usable_lba:            u64::from_le_bytes(buf[40..48].try_into().unwrap_or([0;8])),
        last_usable_lba:             u64::from_le_bytes(buf[48..56].try_into().unwrap_or([0;8])),
        disk_guid:                   buf[56..72].try_into().unwrap_or([0;16]),
        partition_entry_lba:         u64::from_le_bytes(buf[72..80].try_into().unwrap_or([0;8])),
        num_partition_entries:       num_entries as u32,
        size_of_partition_entry:     entry_size as u32,
        partition_entry_array_crc32: u32::from_le_bytes(buf[88..92].try_into().unwrap_or([0;4])),
    };

    let mut entries = [GptEntry::empty(); GPT_MAX_ENTRIES];
    let per_sec = 512 / GPT_ENTRY_SIZE;
    for chunk in 0..32u32 {
        drive.read_sector(GPT_ENTRIES_LBA + chunk, &mut buf).map_err(GptReadError::Io)?;
        for j in 0..per_sec {
            let idx = chunk as usize * per_sec + j;
            if idx >= GPT_MAX_ENTRIES { break; }
            entries[idx] = bytes_to_entry(&buf[j * GPT_ENTRY_SIZE..(j + 1) * GPT_ENTRY_SIZE]);
        }
    }

    // Compute total sectors with overflow + LBA28 range check. If the
    // disk genuinely exceeds u32 sectors (>2 TB) we must refuse rather
    // than silently truncate; downstream sector arithmetic would index
    // the wrong physical region
    let total_u64 = match header.alternate_lba.checked_add(1) {
        Some(v) => v,
        None    => return Err(GptReadError::InvalidFormat),
    };
    if total_u64 > u32::MAX as u64 {
        return Err(GptReadError::DiskTooLarge);
    }
    let total_sectors = total_u64 as u32;
    Ok(GptTable { header, entries, total_sectors })
}

pub fn gpt_add_partition(
    mut drive:    AtaDrive,
    type_guid:    [u8; 16],
    size_sectors: u64,
    name:         &str,
    seed:         u32,
) -> Result<usize, GptWriteError> {
    let mut tbl = gpt_read(&mut drive).map_err(|_| GptWriteError::ReadFailed)?;

    let slot  = tbl.first_free_slot().ok_or(GptWriteError::NoFreeSlot)?;
    let start = tbl.next_free_lba();
    if size_sectors == 0 { return Err(GptWriteError::NotEnoughSpace); }
    // checked_add - size_sectors is caller-supplied and could overflow
    // when combined with start (which itself derives from disk metadata)
    let end = match start.checked_add(size_sectors).and_then(|v| v.checked_sub(1)) {
        Some(e) => e,
        None    => return Err(GptWriteError::NotEnoughSpace),
    };
    if end > tbl.last_usable_lba() { return Err(GptWriteError::NotEnoughSpace); }

    let mut entry    = GptEntry::empty();
    entry.type_guid  = type_guid;
    entry.unique_guid = guid_pseudo(seed ^ start as u32);
    entry.start_lba  = start;
    entry.end_lba    = end;
    entry.set_name(name);
    tbl.entries[slot] = entry;

    write_full_table(&mut drive, &mut tbl).map_err(GptWriteError::Io)?;

    {
        let mut cache = GPT_CACHE.lock();
        cache.header  = tbl.header;
        cache.entries = tbl.entries;
        cache.total_sectors = tbl.total_sectors;
    }

    crate::serial_println!("[gpt] added partition {} '{}' lba {}-{}", slot, name, start, end);
    Ok(slot)
}

pub fn gpt_del_partition(mut drive: AtaDrive, index: usize) -> Result<(), GptWriteError> {
    if index >= GPT_MAX_ENTRIES { return Err(GptWriteError::InvalidIndex); }

    let mut tbl = gpt_read(&mut drive).map_err(|_| GptWriteError::ReadFailed)?;
    if !tbl.entries[index].is_used() { return Err(GptWriteError::InvalidIndex); }

    tbl.entries[index] = GptEntry::empty();
    write_full_table(&mut drive, &mut tbl).map_err(GptWriteError::Io)?;

    {
        let mut cache = GPT_CACHE.lock();
        cache.header  = tbl.header;
        cache.entries = tbl.entries;
        cache.total_sectors = tbl.total_sectors;
    }

    crate::serial_println!("[gpt] deleted partition {}", index);
    Ok(())
}

pub fn gpt_probe_sectors(drive: &mut AtaDrive) -> u32 {
    let mut buf = [0u8; 512];
    if drive.read_sector(0, &mut buf).is_err() { return 0; }
    let mut lo: u32 = 2048;
    let mut hi: u32 = u32::MAX / 2;
    while hi > lo && drive.read_sector(hi - 1, &mut buf).is_err() { hi /= 2; }
    while lo + 1 < hi {
        let mid = lo + (hi - lo) / 2;
        if drive.read_sector(mid, &mut buf).is_ok() { lo = mid; } else { hi = mid; }
    }
    lo + 1
}

#[derive(Debug)]
pub enum GptReadError {
    Io(AtaError),
    NotGpt,
    InvalidFormat,
    /// Disk exceeds the LBA28 addressing window this driver supports.
    /// Fail loudly so a >2 TB disk isn't silently presented as 2 TB
    DiskTooLarge,
}

#[derive(Debug)]
pub enum GptWriteError {
    Io(AtaError),
    ReadFailed,
    NoFreeSlot,
    NotEnoughSpace,
    InvalidIndex,
}
