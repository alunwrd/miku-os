// The reader against a volume from `mkfs.btrfs`, checked against
// `btrfs inspect-internal dump-super`
//
// Tools are vendored in `refsrc/btrfs-progs`, built there since the system has
// no package; an installed one is found through PATH first. Absent both, the
// tests skip.

use btrfs::structs::*;
use btrfs::{check_superblock, MountReject, Superblock};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where `mkfs.btrfs` and `btrfs` are, if anywhere. The vendored build first,
/// whose source is in `refsrc/`
fn tool(name: &str) -> Option<PathBuf> {
    let vendored = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../refsrc/btrfs-progs")
        .join(name);
    if vendored.exists() {
        return Some(vendored);
    }
    let out = Command::new(name).arg("--version").output().ok()?;
    if out.status.success() {
        Some(PathBuf::from(name))
    } else {
        None
    }
}

fn skip(why: &str) -> bool {
    eprintln!("btrfs-progs not available ({why}) - skipping");
    true
}

/// 512 MiB, past btrfs's minimum and small enough to make several
const IMG_BYTES: u64 = 512 * 1024 * 1024;

struct Image {
    path: PathBuf,
}

impl Image {
    /// A fresh volume. `populate` fills a staging directory `mkfs -r` copies
    /// in, with no root or mount
    fn new(tag: &str, populate: Option<&Path>) -> Option<Self> {
        let mkfs = tool("mkfs.btrfs")?;
        let mut path = std::env::temp_dir();
        path.push(format!("btrfs-test-{tag}-{}.img", std::process::id()));

        let f = std::fs::File::create(&path).expect("cannot create image");
        f.set_len(IMG_BYTES).expect("cannot size image");
        drop(f);

        let mut cmd = Command::new(&mkfs);
        cmd.arg("-q");
        if let Some(dir) = populate {
            cmd.arg("-r").arg(dir);
        }
        cmd.arg(&path);
        let out = cmd.output().expect("mkfs.btrfs did not run");
        if !out.status.success() {
            panic!(
                "mkfs.btrfs failed: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Some(Image { path })
    }

    fn read_super_at(&self, offset: u64) -> Superblock {
        let mut f = std::fs::File::open(&self.path).expect("cannot open image");
        f.seek(SeekFrom::Start(offset)).expect("cannot seek");
        let mut sb = Superblock::zeroed();
        f.read_exact(&mut sb.data).expect("cannot read superblock");
        sb
    }

    fn read_super(&self) -> Superblock {
        self.read_super_at(SUPER_INFO_OFFSET)
    }

    /// What btrfs-progs says is in there, as `field value` lines
    fn dump_super(&self) -> String {
        let btrfs = tool("btrfs").expect("btrfs tool vanished");
        let out = Command::new(btrfs)
            .args(["inspect-internal", "dump-super"])
            .arg(&self.path)
            .output()
            .expect("btrfs did not run");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn field(&self, dump: &str, name: &str) -> String {
        for line in dump.lines() {
            let mut parts = line.split_whitespace();
            if parts.next() == Some(name) {
                return parts.next().unwrap_or("").to_string();
            }
        }
        panic!("dump-super has no field {name}:\n{dump}");
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        if std::env::var_os("BTRFS_KEEP_IMAGES").is_some() {
            eprintln!("kept image: {}", self.path.display());
            return;
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Each number read from the superblock against btrfs-progs's print of it
#[test]
fn every_field_agrees_with_btrfs_progs() {
    let Some(img) = Image::new("fields", None) else {
        assert!(skip("no mkfs.btrfs"));
        return;
    };
    let sb = img.read_super();
    let dump = img.dump_super();

    let u64f = |name: &str| -> u64 {
        let v = img.field(&dump, name);
        v.parse::<u64>().unwrap_or_else(|_| panic!("{name} = {v:?}"))
    };
    let u32f = |name: &str| -> u32 {
        let v = img.field(&dump, name);
        v.parse::<u32>().unwrap_or_else(|_| panic!("{name} = {v:?}"))
    };

    assert_eq!(sb.magic(), BTRFS_MAGIC, "magic at offset 64");
    assert_eq!(sb.bytenr(), u64f("bytenr"));
    assert_eq!(sb.generation(), u64f("generation"));
    assert_eq!(sb.root(), u64f("root"));
    assert_eq!(sb.chunk_root(), u64f("chunk_root"));
    assert_eq!(sb.log_root(), u64f("log_root"));
    assert_eq!(sb.total_bytes(), u64f("total_bytes"));
    assert_eq!(sb.bytes_used(), u64f("bytes_used"));
    assert_eq!(sb.num_devices(), u64f("num_devices"));
    assert_eq!(sb.sectorsize(), u32f("sectorsize"));
    assert_eq!(sb.nodesize(), u32f("nodesize"));
    assert_eq!(sb.stripesize(), u32f("stripesize"));
    assert_eq!(sb.sys_chunk_array_size(), u32f("sys_array_size"));

    // The levels and the feature words are printed in hex
    let hex = |name: &str| -> u64 {
        let v = img.field(&dump, name);
        u64::from_str_radix(v.trim_start_matches("0x"), 16)
            .unwrap_or_else(|_| panic!("{name} = {v:?}"))
    };
    assert_eq!(sb.incompat_flags(), hex("incompat_flags"));
    assert_eq!(sb.compat_ro_flags(), hex("compat_ro_flags"));
    assert_eq!(sb.root_level() as u64, u64f("root_level"));
    assert_eq!(sb.chunk_root_level() as u64, u64f("chunk_root_level"));
}

/// The checksum must be recomputed as btrfs computes it
#[test]
fn the_superblock_checksum_is_the_one_btrfs_wrote() {
    let Some(img) = Image::new("csum", None) else {
        assert!(skip("no mkfs.btrfs"));
        return;
    };
    let sb = img.read_super();
    assert_eq!(sb.csum_type(), CSUM_TYPE_CRC32, "mkfs default is crc32c");
    assert!(
        sb.csum_matches(),
        "computed {:08x}, stored {:08x}",
        sb.computed_csum(),
        u32_at(&sb.data, sb::CSUM).unwrap()
    );
}

/// A new `mkfs.btrfs` volume must be one this driver opens
#[test]
fn a_freshly_made_volume_is_accepted() {
    let Some(img) = Image::new("accept", None) else {
        assert!(skip("no mkfs.btrfs"));
        return;
    };
    let sb = img.read_super();
    let geo = check_superblock(&sb, Some(SUPER_INFO_OFFSET))
        .unwrap_or_else(|e| panic!("a default mkfs volume was refused: {e:?}"));

    assert_eq!(geo.sectorsize, 4096);
    assert_eq!(geo.nodesize, 16384);
    assert_eq!(geo.num_devices, 1);
    assert!(geo.chunk_root != 0, "the chunk root has to be somewhere");

    // This machine's measured defaults, asserted so a change fails here
    assert!(
        geo.incompat & FEATURE_INCOMPAT_NO_HOLES != 0,
        "no-holes is a default and changes how a gap is represented"
    );
    assert!(
        geo.incompat & FEATURE_INCOMPAT_SKINNY_METADATA != 0,
        "skinny-metadata is a default and shortens an extent item's key"
    );
    assert!(
        geo.compat_ro & FEATURE_COMPAT_RO_BLOCK_GROUP_TREE != 0,
        "block-group-tree is a default; a driver refusing it refuses new volumes"
    );
}

/// Backups are the same superblock at other offsets, for when the disk's front
/// is damaged
#[test]
fn the_backup_superblock_is_readable_too() {
    let Some(img) = Image::new("mirror", None) else {
        assert!(skip("no mkfs.btrfs"));
        return;
    };
    // 512 MiB holds the primary and the 64 MiB mirror, and no more
    let mirror = SUPER_MIRRORS[1];
    assert!(mirror < IMG_BYTES, "this image has to contain mirror 1");

    let sb = img.read_super_at(mirror);
    let geo = check_superblock(&sb, Some(mirror))
        .unwrap_or_else(|e| panic!("the backup superblock was refused: {e:?}"));

    let primary = check_superblock(&img.read_super(), Some(SUPER_INFO_OFFSET)).unwrap();
    assert_eq!(geo.root, primary.root, "both copies name the same tree root");
    assert_eq!(geo.chunk_root, primary.chunk_root);
    assert_eq!(sb.bytenr(), mirror, "each copy records where it lives");
}

/// A copy read from the wrong place is caught, as each records its address
#[test]
fn a_copy_read_from_the_wrong_place_is_refused() {
    let Some(img) = Image::new("place", None) else {
        assert!(skip("no mkfs.btrfs"));
        return;
    };
    let sb = img.read_super();
    match check_superblock(&sb, Some(SUPER_MIRRORS[1])) {
        Err(MountReject::WrongLocation { says, read_from }) => {
            assert_eq!(says, SUPER_INFO_OFFSET);
            assert_eq!(read_from, SUPER_MIRRORS[1]);
        }
        other => panic!("a misplaced superblock must be refused, got {other:?}"),
    }
}

/// The bootstrap chunk array maps the chunk tree's blocks, so nothing is
/// readable if it is wrong
#[test]
fn the_bootstrap_chunk_array_walks() {
    let Some(img) = Image::new("sysarray", None) else {
        assert!(skip("no mkfs.btrfs"));
        return;
    };
    let sb = img.read_super();
    btrfs::mount::check_sys_chunk_array(&sb).expect("a real sys_chunk_array must walk");

    // It describes at least one SYSTEM chunk
    let arr = sb.sys_chunk_array();
    let key = Key::read(arr, 0).unwrap();
    assert_eq!(key.kind, CHUNK_ITEM_KEY, "the first entry is a chunk item");
    let ty = u64_at(arr, DISK_KEY_SIZE + chunk::TYPE).unwrap();
    assert!(ty & BLOCK_GROUP_SYSTEM != 0, "and it is a system chunk");
    let num_stripes = u16_at(arr, DISK_KEY_SIZE + chunk::NUM_STRIPES).unwrap();
    assert!(num_stripes >= 1);
    // mkfs makes SYSTEM chunks DUP on a single device
    assert_eq!(
        ty & BLOCK_GROUP_PROFILE_MASK,
        BLOCK_GROUP_DUP,
        "mkfs defaults SYSTEM to DUP on one device"
    );
    assert_eq!(num_stripes, 2, "DUP is two copies");
}

/// Proves the staging path later tests use, and the resulting volume is sound
#[test]
fn a_populated_volume_is_sound() {
    let mut stage = std::env::temp_dir();
    stage.push(format!("btrfs-stage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(stage.join("dir")).unwrap();
    std::fs::File::create(stage.join("hello.txt"))
        .unwrap()
        .write_all(b"hello from the host")
        .unwrap();

    let Some(img) = Image::new("populated", Some(&stage)) else {
        let _ = std::fs::remove_dir_all(&stage);
        assert!(skip("no mkfs.btrfs"));
        return;
    };

    let sb = img.read_super();
    let geo = check_superblock(&sb, Some(SUPER_INFO_OFFSET)).expect("populated volume refused");
    assert!(geo.total_bytes >= IMG_BYTES / 2);

    // btrfs-progs agrees it is sound
    let btrfs = tool("btrfs").unwrap();
    let out = Command::new(btrfs)
        .arg("check")
        .arg(&img.path)
        .output()
        .expect("btrfs check did not run");
    let _ = std::fs::remove_dir_all(&stage);
    assert!(
        out.status.success(),
        "btrfs check refused a volume we only read:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
