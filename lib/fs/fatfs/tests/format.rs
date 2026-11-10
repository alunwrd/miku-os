// What `format`, `mkdir`, `write_file` and `finish` leave on the disk, read
// back from the raw image

use fatfs::layout::{Geometry, EOC, MIN_CLUSTERS, RESERVED, SECTOR};
use fatfs::{format, FatError, MemDisk};

/// 100 MiB, the installer's partition
const ESP: u64 = 204_800;
const LABEL: &[u8; 11] = b"MIKUOS ESP ";

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn fat_entry(img: &[u8], geo: &Geometry, copy: u32, cluster: u32) -> u32 {
    u32_at(img, geo.fat_lba(copy) as usize * SECTOR + cluster as usize * 4)
}

fn cluster<'a>(img: &'a [u8], geo: &Geometry, n: u32) -> &'a [u8] {
    let at = geo.cluster_lba(n) as usize * SECTOR;
    &img[at..at + geo.cluster_bytes()]
}

/// (first cluster, size, attributes) of the entry called `name` in a directory
fn lookup(dir: &[u8], name: &[u8; 11]) -> Option<(u32, u32, u8)> {
    dir.chunks(32).find(|e| &e[..11] == name).map(|e| {
        let cluster = (u16_at(e, 20) as u32) << 16 | u16_at(e, 26) as u32;
        (cluster, u32_at(e, 28), e[11])
    })
}

#[test]
fn geometry_follows_the_specification() {
    let g = Geometry::new(ESP).unwrap();
    assert_eq!(g.sectors_per_cluster, 1);
    assert!(g.clusters >= MIN_CLUSTERS);
    // A FAT entry for every cluster and the two before the first
    assert!(g.fat_sectors as u64 * 128 >= g.clusters as u64 + 2);
    assert!(g.data_lba() as u64 + g.clusters as u64 * g.sectors_per_cluster as u64 <= ESP);

    assert_eq!(Geometry::new(600_000).unwrap().sectors_per_cluster, 8);
    assert_eq!(Geometry::new(20_000_000).unwrap().sectors_per_cluster, 16);
    assert_eq!(Geometry::new(66_600), Err(FatError::TooSmall));
    assert!(Geometry::new(66_601).unwrap().clusters >= MIN_CLUSTERS);
    assert_eq!(Geometry::new(1 << 33), Err(FatError::TooBig));
}

#[test]
fn every_cluster_size_keeps_fat32_cluster_counts() {
    for sectors in [66_601u64, 100_000, 532_480, 532_481, 1 << 22, 16_777_216, 16_777_217, 1 << 26, 1 << 31] {
        let g = Geometry::new(sectors).unwrap();
        assert!(g.clusters >= MIN_CLUSTERS, "{sectors} sectors: {} clusters", g.clusters);
        assert!(g.fat_sectors as u64 * 128 >= g.clusters as u64 + 2, "{sectors} sectors: FAT too short");
    }
}

#[test]
fn boot_sector_and_its_copy() {
    let mut disk = MemDisk::new(ESP as usize);
    let vol = format(&mut disk, 0, ESP, LABEL, 0x1234_5678).unwrap();
    let geo = vol.geometry();
    vol.finish().unwrap();
    let img = disk.image_flushed();

    assert_eq!(u16_at(img, 11), 512);
    assert_eq!(img[13] as u32, geo.sectors_per_cluster);
    assert_eq!(u16_at(img, 14) as u32, RESERVED);
    assert_eq!(img[16], 2);
    assert_eq!(u16_at(img, 17), 0, "FAT32 has no fixed root directory");
    assert_eq!(u16_at(img, 22), 0, "FAT32 keeps its FAT size in the 32-bit field");
    assert_eq!(u32_at(img, 32) as u64, ESP);
    assert_eq!(u32_at(img, 36), geo.fat_sectors);
    assert_eq!(u32_at(img, 44), 2);
    assert_eq!(u32_at(img, 67), 0x1234_5678);
    assert_eq!(&img[71..82], LABEL);
    assert_eq!(&img[82..90], b"FAT32   ");
    assert_eq!(&img[510..512], &[0x55, 0xAA]);
    assert_eq!(&img[..SECTOR], &img[6 * SECTOR..7 * SECTOR]);
    assert_eq!(&img[SECTOR..2 * SECTOR], &img[7 * SECTOR..8 * SECTOR]);

    // FSInfo: the root's cluster is the only one taken
    assert_eq!(u32_at(img, SECTOR), 0x4161_5252);
    assert_eq!(u32_at(img, SECTOR + 484), 0x6141_7272);
    assert_eq!(u32_at(img, SECTOR + 488), geo.clusters - 1);
    assert_eq!(u32_at(img, SECTOR + 492), 3);
    assert_eq!(u32_at(img, SECTOR + 508), 0xAA55_0000);

    assert_eq!(fat_entry(img, &geo, 0, 0), 0x0FFF_FFF8);
    assert_eq!(fat_entry(img, &geo, 0, 2), EOC);
    assert_eq!(fat_entry(img, &geo, 0, 3), 0);
    // The label is the root's first entry
    assert_eq!(lookup(cluster(img, &geo, 2), LABEL), Some((0, 0, 0x08)));
}

#[test]
fn a_file_two_directories_down() {
    let data: Vec<u8> = (0..300_000u32).map(|i| (i * 7 + i / 512) as u8).collect();
    let mut disk = MemDisk::new(ESP as usize);
    let mut vol = format(&mut disk, 0, ESP, LABEL, 1).unwrap();
    let geo = vol.geometry();
    let efi = vol.mkdir(vol.root(), "EFI").unwrap();
    let boot = vol.mkdir(efi, "boot").unwrap();
    vol.write_file(boot, "BOOTX64.EFI", &data).unwrap();
    vol.write_file(boot, "EMPTY", &[]).unwrap();
    vol.finish().unwrap();
    let img = disk.image_flushed();

    let (efi_c, _, attr) = lookup(cluster(img, &geo, 2), b"EFI        ").unwrap();
    assert_eq!(attr, 0x10);
    let efi_dir = cluster(img, &geo, efi_c);
    assert_eq!(lookup(efi_dir, b".          "), Some((efi_c, 0, 0x10)));
    assert_eq!(lookup(efi_dir, b"..         "), Some((0, 0, 0x10)), "the root is 0 in a dot-dot entry");

    let (boot_c, _, _) = lookup(efi_dir, b"BOOT       ").unwrap();
    let boot_dir = cluster(img, &geo, boot_c);
    assert_eq!(lookup(boot_dir, b"..         "), Some((efi_c, 0, 0x10)));
    assert_eq!(lookup(boot_dir, b"EMPTY      "), Some((0, 0, 0x20)));

    let (first, size, attr) = lookup(boot_dir, b"BOOTX64 EFI").unwrap();
    assert_eq!((size, attr), (300_000, 0x20));
    // The chain, and the bytes it leads to
    let mut read = Vec::new();
    let mut c = first;
    loop {
        read.extend_from_slice(cluster(img, &geo, c));
        let next = fat_entry(img, &geo, 0, c);
        assert_eq!(next, fat_entry(img, &geo, 1, c), "the two FATs differ at cluster {c}");
        if next == EOC {
            break;
        }
        c = next;
    }
    assert_eq!(&read[..data.len()], &data[..]);
    assert!(read[data.len()..].iter().all(|&b| b == 0), "the last cluster is not zero past the end");

    let used = 3 + 300_000u32.div_ceil(512);
    assert_eq!(u32_at(img, SECTOR + 488), geo.clusters - used);
    assert_eq!(u32_at(img, SECTOR + 492), 2 + used);
}

#[test]
fn a_volume_inside_a_partition() {
    const START: u64 = 6144;
    let mut disk = MemDisk::new((START + ESP + 100) as usize);
    let mut vol = format(&mut disk, START, ESP, LABEL, 1).unwrap();
    let dir = vol.mkdir(vol.root(), "EFI").unwrap();
    vol.write_file(dir, "A.BIN", &[0xAB; 5000]).unwrap();
    vol.finish().unwrap();
    let img = disk.image_flushed();

    let at = START as usize * SECTOR;
    let end = (START + ESP) as usize * SECTOR;
    assert!(img[..at].iter().all(|&b| b == 0), "sectors before the partition were written");
    assert!(img[end..].iter().all(|&b| b == 0), "sectors after the partition were written");
    assert_eq!(u32_at(img, at + 28) as u64, START, "hidden sectors");
    assert_eq!(&img[at + 510..at + 512], &[0x55, 0xAA]);
}

#[test]
fn names() {
    let mut disk = MemDisk::new(ESP as usize);
    let mut vol = format(&mut disk, 0, ESP, LABEL, 1).unwrap();
    let root = vol.root();
    for bad in ["", ".", "..", "LONGERTHAN8", "A.LONG", "A.B.C", "A B", "A.", ".A", "Я", "A/B", "A*"] {
        assert_eq!(vol.write_file(root, bad, b"x"), Err(FatError::BadName), "{bad:?}");
    }
    vol.write_file(root, "a.txt", b"x").unwrap();
    assert_eq!(vol.write_file(root, "A.TXT", b"y"), Err(FatError::Exists));
    assert_eq!(vol.mkdir(root, "A.TXT").err(), Some(FatError::Exists));
}

#[test]
fn a_directory_is_one_cluster() {
    let mut disk = MemDisk::new(ESP as usize);
    let mut vol = format(&mut disk, 0, ESP, LABEL, 1).unwrap();
    let dir = vol.mkdir(vol.root(), "D").unwrap();
    // 16 entries in 512 bytes, two of them the dot entries
    for i in 0..14 {
        vol.write_file(dir, &format!("F{i}"), b"x").unwrap();
    }
    assert_eq!(vol.write_file(dir, "ONEMORE", b"x"), Err(FatError::DirFull));
    assert_eq!(vol.mkdir(dir, "ONEMORE").err(), Some(FatError::DirFull));
}

#[test]
fn a_file_larger_than_the_volume() {
    const SMALL: u64 = 66_601;
    let mut disk = MemDisk::new(SMALL as usize);
    let mut vol = format(&mut disk, 0, SMALL, LABEL, 1).unwrap();
    let clusters = vol.geometry().clusters as usize;
    let root = vol.root();
    assert_eq!(vol.write_file(root, "BIG", &vec![0u8; clusters * 512]), Err(FatError::NoSpace));
    // Every cluster but the root's
    vol.write_file(root, "FITS", &vec![1u8; (clusters - 1) * 512]).unwrap();
    assert_eq!(vol.write_file(root, "ONE", b"x"), Err(FatError::NoSpace));
    vol.finish().unwrap();
    let img = disk.image_flushed();
    assert_eq!(u32_at(img, SECTOR + 488), 0);
    assert_eq!(u32_at(img, SECTOR + 492), u32::MAX);
}

#[test]
fn a_volume_too_small_for_fat32() {
    let mut disk = MemDisk::new(40_000);
    assert_eq!(format(&mut disk, 0, 40_000, LABEL, 1).err(), Some(FatError::TooSmall));
}
