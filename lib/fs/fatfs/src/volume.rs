// A FAT32 volume being filled.
//
// Clusters are handed out in order and never freed, so every file and
// directory is one run and the FAT kept here is only its used front. A
// directory is one cluster. `finish` writes the FAT and FSInfo; before it the
// volume holds the data and no chains.

use alloc::vec;
use alloc::vec::Vec;

use fshost::BlockHost;

use crate::layout::{
    self, Geometry, ATTR_ARCHIVE, ATTR_DIR, ATTR_VOLUME, BACKUP_SECTOR, DIR_ENTRY, EOC, FATS,
    FAT_MEDIA, FSINFO_SECTOR, ROOT_CLUSTER, SECTOR,
};
use crate::name::short_name;
use crate::FatError;

/// Sectors a single write carries
const CHUNK: usize = 128;

/// A directory of the volume it came from
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dir(usize);

struct DirState {
    cluster: u32,
    names: Vec<[u8; 11]>,
    /// Entries written, the label and the dot entries among them
    used: usize,
}

pub struct Volume<H: BlockHost> {
    host: H,
    /// First sector of the volume on the device
    start: u64,
    geo: Geometry,
    /// FAT entries of every cluster handed out, by cluster number
    fat: Vec<u32>,
    dirs: Vec<DirState>,
    stamp: (u16, u16),
}

/// Lays an empty FAT32 volume over `sectors` sectors at `start`. `label` is
/// eleven bytes padded with spaces, all spaces for none
pub fn format<H: BlockHost>(
    host: H,
    start: u64,
    sectors: u64,
    label: &[u8; 11],
    serial: u32,
) -> Result<Volume<H>, FatError> {
    let geo = Geometry::new(sectors)?;
    let stamp = layout::dos_stamp(host.wall_clock().unwrap_or(0));
    let mut vol = Volume {
        host,
        start,
        geo,
        fat: vec![FAT_MEDIA, EOC, EOC],
        dirs: vec![DirState { cluster: ROOT_CLUSTER, names: Vec::new(), used: 0 }],
        stamp,
    };

    // The reserved sectors, both FATs and the root directory's cluster
    vol.zero(0, geo.data_lba() + geo.sectors_per_cluster)?;
    let boot = geo.boot_sector(start as u32, serial, label);
    let info = layout::fsinfo(geo.clusters - 1, ROOT_CLUSTER + 1);
    for base in [0, BACKUP_SECTOR] {
        vol.write(base, &boot)?;
        vol.write(base + FSINFO_SECTOR, &info)?;
    }
    if label != b"           " {
        let entry = layout::dir_entry(label, ATTR_VOLUME, 0, 0, stamp);
        vol.add_entry(Dir(0), &entry)?;
    }
    Ok(vol)
}

impl<H: BlockHost> Volume<H> {
    pub fn root(&self) -> Dir {
        Dir(0)
    }

    pub fn geometry(&self) -> Geometry {
        self.geo
    }

    pub fn mkdir(&mut self, parent: Dir, name: &str) -> Result<Dir, FatError> {
        let short = self.new_name(parent, name)?;
        self.room_in(parent)?;
        let cluster = self.alloc(1)?;
        let parent_cluster = match self.dirs[parent.0].cluster {
            // The root is written as 0 in a `..` entry
            ROOT_CLUSTER => 0,
            c => c,
        };

        let mut body = vec![0u8; self.geo.cluster_bytes()];
        body[..DIR_ENTRY].copy_from_slice(&layout::dir_entry(b".          ", ATTR_DIR, cluster, 0, self.stamp));
        body[DIR_ENTRY..2 * DIR_ENTRY]
            .copy_from_slice(&layout::dir_entry(b"..         ", ATTR_DIR, parent_cluster, 0, self.stamp));
        self.write(self.geo.cluster_lba(cluster), &body)?;

        self.add_entry(parent, &layout::dir_entry(&short, ATTR_DIR, cluster, 0, self.stamp))?;
        self.dirs[parent.0].names.push(short);
        self.dirs.push(DirState { cluster, names: Vec::new(), used: 2 });
        Ok(Dir(self.dirs.len() - 1))
    }

    pub fn write_file(&mut self, parent: Dir, name: &str, data: &[u8]) -> Result<(), FatError> {
        let short = self.new_name(parent, name)?;
        self.room_in(parent)?;
        let size = u32::try_from(data.len()).map_err(|_| FatError::TooBig)?;
        let clusters = data.len().div_ceil(self.geo.cluster_bytes()) as u32;
        // An empty file owns no cluster
        let first = if clusters == 0 { 0 } else { self.alloc(clusters)? };

        if clusters != 0 {
            let lba = self.geo.cluster_lba(first);
            let whole = data.len() / SECTOR * SECTOR;
            self.write(lba, &data[..whole])?;
            // The rest of the last cluster, zero past the end of the file
            let run = clusters as usize * self.geo.cluster_bytes();
            let mut tail = vec![0u8; run - whole];
            tail[..data.len() - whole].copy_from_slice(&data[whole..]);
            self.write(lba + (whole / SECTOR) as u32, &tail)?;
        }

        self.add_entry(parent, &layout::dir_entry(&short, ATTR_ARCHIVE, first, size, self.stamp))?;
        self.dirs[parent.0].names.push(short);
        Ok(())
    }

    /// Writes the FAT, both copies, and FSInfo, and drains the device. The
    /// volume is not one before this
    pub fn finish(mut self) -> Result<H, FatError> {
        let mut bytes = Vec::with_capacity(self.fat.len() * 4 + SECTOR);
        for e in &self.fat {
            bytes.extend_from_slice(&e.to_le_bytes());
        }
        bytes.resize(bytes.len().div_ceil(SECTOR) * SECTOR, 0);
        for copy in 0..FATS {
            self.write(self.geo.fat_lba(copy), &bytes)?;
        }

        let used = self.fat.len() as u32 - 2;
        let free = self.geo.clusters - used;
        // 0xFFFFFFFF: no hint
        let next = if free == 0 { u32::MAX } else { self.fat.len() as u32 };
        let info = layout::fsinfo(free, next);
        for base in [0, BACKUP_SECTOR] {
            self.write(base + FSINFO_SECTOR, &info)?;
        }
        self.host.flush()?;
        Ok(self.host)
    }

    /// `name` in its stored form, if `dir` does not have it yet
    fn new_name(&self, dir: Dir, name: &str) -> Result<[u8; 11], FatError> {
        let short = short_name(name)?;
        if self.dirs[dir.0].names.contains(&short) {
            return Err(FatError::Exists);
        }
        Ok(short)
    }

    fn room_in(&self, dir: Dir) -> Result<(), FatError> {
        if self.dirs[dir.0].used >= self.geo.cluster_bytes() / DIR_ENTRY {
            return Err(FatError::DirFull);
        }
        Ok(())
    }

    /// A run of `count` clusters, chained. Returns the first
    fn alloc(&mut self, count: u32) -> Result<u32, FatError> {
        let first = self.fat.len() as u32;
        if count > self.geo.clusters - (first - 2) {
            return Err(FatError::NoSpace);
        }
        for i in 1..count {
            self.fat.push(first + i);
        }
        self.fat.push(EOC);
        Ok(first)
    }

    fn add_entry(&mut self, dir: Dir, entry: &[u8; DIR_ENTRY]) -> Result<(), FatError> {
        self.room_in(dir)?;
        let state = &mut self.dirs[dir.0];
        let at = state.used * DIR_ENTRY;
        state.used += 1;
        let lba = self.geo.cluster_lba(state.cluster) + (at / SECTOR) as u32;

        let mut sector = [0u8; SECTOR];
        self.host.read_sectors(self.start + lba as u64, 1, &mut sector)?;
        sector[at % SECTOR..at % SECTOR + DIR_ENTRY].copy_from_slice(entry);
        self.write(lba, &sector)
    }

    /// Whole sectors at `lba` from the start of the volume
    fn write(&mut self, lba: u32, data: &[u8]) -> Result<(), FatError> {
        debug_assert!(data.len() % SECTOR == 0);
        let mut lba = self.start + lba as u64;
        for chunk in data.chunks(CHUNK * SECTOR) {
            let count = (chunk.len() / SECTOR) as u32;
            self.host.write_sectors(lba, count, chunk)?;
            lba += count as u64;
        }
        Ok(())
    }

    fn zero(&mut self, lba: u32, count: u32) -> Result<(), FatError> {
        let zeros = vec![0u8; CHUNK * SECTOR];
        let mut done = 0u32;
        while done < count {
            let n = (count - done).min(CHUNK as u32);
            self.write(lba + done, &zeros[..n as usize * SECTOR])?;
            done += n;
        }
        Ok(())
    }
}
