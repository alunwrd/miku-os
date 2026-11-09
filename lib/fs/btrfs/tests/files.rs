// Finding files staged by `mkfs.btrfs -r <dir>`, which copies a tree into a
// fresh volume with no mount or root
//
// `a_name_is_found_by_its_hash`: a lookup is not a scan, as the key's offset is
// `crc32c((u32)~1, name)`, so a wrong hash convention returns nothing.

mod common;

use btrfs::error::BtrfsError;
use btrfs::fs_tree::*;
use btrfs::{Key, Volume};
use common::Image;
use fshost::{BlockHost, HostError};
use std::io::Write;
use std::path::PathBuf;

struct FileHost {
    data: Vec<u8>,
}

impl BlockHost for FileHost {
    fn read_sectors(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), HostError> {
        let start = (lba as usize) * 512;
        let end = start + (count as usize) * 512;
        let src = self.data.get(start..end).ok_or(HostError::Io)?;
        let n = buf.len().min(src.len());
        buf[..n].copy_from_slice(&src[..n]);
        Ok(())
    }
    fn write_sectors(&mut self, _: u64, _: u32, _: &[u8]) -> Result<(), HostError> {
        Err(HostError::Unsupported)
    }
    fn write_sectors_sync(&mut self, _: u64, _: u32, _: &[u8]) -> Result<(), HostError> {
        Err(HostError::Unsupported)
    }
    fn write_sectors_barrier(&mut self, _: u64, _: u32, _: &[u8]) -> Result<(), HostError> {
        Err(HostError::Unsupported)
    }
    fn flush(&mut self) -> Result<(), HostError> {
        Ok(())
    }
    fn total_sectors(&self) -> u64 {
        (self.data.len() / 512) as u64
    }
}

/// What every case looks for: a long name, one with a dot, and enough entries
struct Staged {
    dir: PathBuf,
    files: Vec<(String, Vec<u8>)>,
}

fn stage(tag: &str) -> Staged {
    let mut dir = std::env::temp_dir();
    dir.push(format!("btrfs-stage-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("subdir")).unwrap();

    let mut files = Vec::new();
    let mut put = |name: &str, body: Vec<u8>| {
        std::fs::File::create(dir.join(name))
            .unwrap()
            .write_all(&body)
            .unwrap();
        files.push((name.to_string(), body));
    };

    put("hello.txt", b"hello from the host".to_vec());
    put("empty", Vec::new());
    put(&"n".repeat(255), b"a name at the format's limit".to_vec());
    put("dotted.name.with.many.parts", b"dots".to_vec());
    for i in 0..40 {
        put(&format!("f{i:03}"), format!("body of {i}").into_bytes());
    }

    std::os::unix::fs::symlink("hello.txt", dir.join("link")).unwrap();

    Staged { dir, files }
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Fixture {
    _img: Image,
    vol: Volume<FileHost>,
    root: Root,
    staged: Staged,
}

fn fixture(tag: &str) -> Option<Fixture> {
    let staged = stage(tag);
    let img = Image::new(tag, Some(&staged.dir))?;
    let host = FileHost { data: std::fs::read(&img.path).expect("cannot read image") };
    let mut vol = Volume::open(host, 0).expect("a fresh volume must open");
    vol.load_chunk_tree().expect("the chunk tree must load");
    let root = vol.fs_root().expect("the fs tree must be findable");
    Some(Fixture { _img: img, vol, root, staged })
}

macro_rules! fx {
    ($tag:expr) => {
        match fixture($tag) {
            Some(f) => f,
            None => {
                eprintln!("btrfs-progs not available - skipping");
                return;
            }
        }
    };
}

/// The root tree names the fs tree, and the fs tree's root block reads
#[test]
fn the_default_subvolume_is_found() {
    let mut f = fx!("subvol");
    assert!(f.root.bytenr != 0);
    assert_eq!(
        f.root.root_dirid, FIRST_FREE_OBJECTID,
        "a subvolume's top directory is inode 256"
    );

    let node = f.vol.read_node(f.root.bytenr).expect("the fs tree root must read");
    assert_eq!(node.bytenr, f.root.bytenr);
    assert_eq!(node.level, f.root.level);
}

/// The top directory is a directory, and its inode says so
#[test]
fn the_top_directory_is_a_directory() {
    let mut f = fx!("topdir");
    let ino = f
        .vol
        .inode(f.root.bytenr, f.root.root_dirid)
        .expect("inode 256 must exist");
    assert!(ino.is_dir(), "mode {:o} is not a directory", ino.mode);
    assert!(ino.nlink >= 1);
}

/// Each staged name is findable, only if the hash matches `mkfs.btrfs`'s key
#[test]
fn a_name_is_found_by_its_hash() {
    let mut f = fx!("hash");
    let dir = f.root.root_dirid;
    let tree = f.root.bytenr;

    for (name, _) in &f.staged.files {
        let e = f
            .vol
            .lookup(tree, dir, name.as_bytes())
            .unwrap_or_else(|e| panic!("{name:?} was not found: {e:?}"));
        assert_eq!(e.name, name.as_bytes());
        assert_eq!(e.ftype, FT_REG_FILE, "{name:?} should be a regular file");
        assert!(e.points_at_inode());
    }

    // The shapes that are not plain files
    let d = f.vol.lookup(tree, dir, b"subdir").expect("subdir");
    assert_eq!(d.ftype, FT_DIR);
    let l = f.vol.lookup(tree, dir, b"link").expect("link");
    assert_eq!(l.ftype, FT_SYMLINK);
}

/// An absent name is `NotFound`, not the entry with a nearby hash
#[test]
fn a_name_that_is_not_there_is_not_found() {
    let mut f = fx!("absent");
    for missing in [&b"nothing"[..], b"hello.tx", b"hello.txtt", b"", b"HELLO.TXT"] {
        assert!(
            matches!(
                f.vol.lookup(f.root.bytenr, f.root.root_dirid, missing),
                Err(BtrfsError::NotFound)
            ),
            "{:?} must not be found",
            String::from_utf8_lossy(missing)
        );
    }
}

/// Listing returns exactly what was staged, once each
#[test]
fn a_directory_lists_what_was_put_in_it() {
    let mut f = fx!("list");
    let mut seen: Vec<String> = Vec::new();
    f.vol
        .readdir(f.root.bytenr, f.root.root_dirid, &mut |e| {
            seen.push(String::from_utf8_lossy(&e.name).into_owned());
            Ok(true)
        })
        .expect("readdir must work");

    let mut expected: Vec<String> = f.staged.files.iter().map(|(n, _)| n.clone()).collect();
    expected.push("subdir".into());
    expected.push("link".into());

    let mut sorted_seen = seen.clone();
    sorted_seen.sort();
    expected.sort();
    assert_eq!(sorted_seen, expected, "the listing does not match what was staged");

    // A name appearing twice would mean DIR_INDEX and DIR_ITEM were walked
    // together
    let mut dedup = seen.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), seen.len(), "a name came back more than once");
}

/// Each listed entry can be looked up, and both agree on its target:
/// `DIR_INDEX` and `DIR_ITEM` describe one directory
#[test]
fn the_listing_and_the_lookup_agree() {
    let mut f = fx!("agree");
    let tree = f.root.bytenr;
    let dir = f.root.root_dirid;

    let mut entries: Vec<DirEntry> = Vec::new();
    f.vol
        .readdir(tree, dir, &mut |e| {
            entries.push(e.clone());
            Ok(true)
        })
        .unwrap();

    for e in &entries {
        let by_name = f
            .vol
            .lookup(tree, dir, &e.name)
            .unwrap_or_else(|err| panic!("{:?}: {err:?}", String::from_utf8_lossy(&e.name)));
        assert_eq!(by_name.location, e.location, "the two indexes disagree");
        assert_eq!(by_name.ftype, e.ftype);
    }
}

/// A file's inode carries its staged size
#[test]
fn an_inodes_size_is_the_size_of_the_file_that_was_staged() {
    let mut f = fx!("sizes");
    let tree = f.root.bytenr;
    let dir = f.root.root_dirid;

    for (name, body) in &f.staged.files {
        let e = f.vol.lookup(tree, dir, name.as_bytes()).unwrap();
        let ino = f
            .vol
            .inode(tree, e.location.objectid)
            .unwrap_or_else(|err| panic!("{name}: {err:?}"));
        assert_eq!(
            ino.size,
            body.len() as u64,
            "{name}: inode says {} bytes, staged {}",
            ino.size,
            body.len()
        );
        assert!(ino.is_regular(), "{name}: mode {:o}", ino.mode);
    }
}

/// A subdirectory's inode is reachable through its entry and lists as a
/// directory
#[test]
fn a_subdirectory_can_be_descended_into() {
    let mut f = fx!("descend");
    let tree = f.root.bytenr;

    let e = f.vol.lookup(tree, f.root.root_dirid, b"subdir").unwrap();
    let ino = f.vol.inode(tree, e.location.objectid).unwrap();
    assert!(ino.is_dir());

    let mut count = 0;
    f.vol
        .readdir(tree, e.location.objectid, &mut |_| {
            count += 1;
            Ok(true)
        })
        .unwrap();
    assert_eq!(count, 0, "the staged subdir is empty");
}

/// A stopping callback is obeyed
#[test]
fn a_listing_stops_when_told() {
    let mut f = fx!("stoplist");
    let mut count = 0;
    f.vol
        .readdir(f.root.bytenr, f.root.root_dirid, &mut |_| {
            count += 1;
            Ok(count < 3)
        })
        .unwrap();
    assert_eq!(count, 3);
}

// Hostile dir items
//
// `parse_dir_records` reads a length then that many bytes, twice, in a loop, as
// `check_dir_item` guards

fn one_record(key: &Key, name: &[u8], ftype: u8, data: &[u8], loc_kind: u8) -> Vec<u8> {
    let mut v = vec![0u8; dir_item::SIZE_OF];
    v[dir_item::LOCATION + 8] = loc_kind;
    v[dir_item::DATA_LEN..dir_item::DATA_LEN + 2]
        .copy_from_slice(&(data.len() as u16).to_le_bytes());
    v[dir_item::NAME_LEN..dir_item::NAME_LEN + 2]
        .copy_from_slice(&(name.len() as u16).to_le_bytes());
    v[dir_item::TYPE] = ftype;
    v.extend_from_slice(name);
    v.extend_from_slice(data);
    let _ = key;
    v
}

#[test]
fn a_well_formed_record_parses() {
    let name = b"control";
    let key = Key { objectid: 256, kind: DIR_ITEM_KEY, offset: name_hash(name) };
    let item = one_record(&key, name, FT_REG_FILE, &[], INODE_ITEM_KEY);
    let r = parse_dir_records(&key, &item).expect("the control case must parse");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].name, name);
}

/// The one that stops a crafted item from filing any name under any key
#[test]
fn a_name_that_does_not_match_its_key_is_refused() {
    let name = b"control";
    let key = Key { objectid: 256, kind: DIR_ITEM_KEY, offset: name_hash(b"something-else") };
    let item = one_record(&key, name, FT_REG_FILE, &[], INODE_ITEM_KEY);
    assert!(parse_dir_records(&key, &item).is_err());
}

#[test]
fn a_record_that_runs_past_the_item_is_refused() {
    let name = b"control";
    let key = Key { objectid: 256, kind: DIR_ITEM_KEY, offset: name_hash(name) };
    let full = one_record(&key, name, FT_REG_FILE, &[], INODE_ITEM_KEY);

    // A name length longer than what follows it
    let mut lying = full.clone();
    lying[dir_item::NAME_LEN..dir_item::NAME_LEN + 2].copy_from_slice(&999u16.to_le_bytes());
    assert!(parse_dir_records(&key, &lying).is_err());

    // And an item cut short of its own header
    for cut in [1usize, dir_item::SIZE_OF - 1, dir_item::SIZE_OF + 1] {
        assert!(
            parse_dir_records(&key, &full[..cut.min(full.len())]).is_err(),
            "an item of {cut} bytes must be refused"
        );
    }
}

#[test]
fn an_impossible_entry_type_is_refused() {
    let name = b"control";
    let key = Key { objectid: 256, kind: DIR_ITEM_KEY, offset: name_hash(name) };
    for bad in [FT_UNKNOWN, FT_MAX, FT_MAX + 1, 200] {
        let item = one_record(&key, name, bad, &[], INODE_ITEM_KEY);
        assert!(parse_dir_records(&key, &item).is_err(), "type {bad} must be refused");
    }
}

/// A name claiming to be an attribute or the reverse
#[test]
fn the_xattr_type_and_the_xattr_key_must_agree() {
    let name = b"user.thing";
    let dir_key = Key { objectid: 256, kind: DIR_ITEM_KEY, offset: name_hash(name) };
    let xattr_key = Key { objectid: 256, kind: XATTR_ITEM_KEY, offset: name_hash(name) };

    let as_xattr = one_record(&xattr_key, name, FT_XATTR, b"value", INODE_ITEM_KEY);
    parse_dir_records(&xattr_key, &as_xattr).expect("an xattr under an xattr key is fine");

    assert!(
        parse_dir_records(&dir_key, &as_xattr).is_err(),
        "an xattr record under a DIR_ITEM key must be refused"
    );

    let as_name = one_record(&xattr_key, name, FT_REG_FILE, &[], INODE_ITEM_KEY);
    assert!(
        parse_dir_records(&xattr_key, &as_name).is_err(),
        "a plain name under an XATTR key must be refused"
    );
}

/// Only an attribute has data; a name with a data length moves the next record
#[test]
fn a_name_with_data_is_refused() {
    let name = b"control";
    let key = Key { objectid: 256, kind: DIR_ITEM_KEY, offset: name_hash(name) };
    let item = one_record(&key, name, FT_REG_FILE, b"payload", INODE_ITEM_KEY);
    assert!(parse_dir_records(&key, &item).is_err());
}

/// A name may point at an inode or at a subvolume and at nothing else
#[test]
fn a_location_pointing_at_neither_an_inode_nor_a_subvolume_is_refused() {
    let name = b"control";
    let key = Key { objectid: 256, kind: DIR_ITEM_KEY, offset: name_hash(name) };
    for bad in [DIR_ITEM_KEY, EXTENT_DATA_KEY, 200u8] {
        let item = one_record(&key, name, FT_REG_FILE, &[], bad);
        assert!(
            parse_dir_records(&key, &item).is_err(),
            "a location of type {bad} must be refused"
        );
    }
}

/// Two names in one item: the collision case
#[test]
fn two_records_in_one_item_both_come_back() {
    let name = b"first";
    let key = Key { objectid: 256, kind: DIR_INDEX_KEY, offset: 3 };
    let mut item = one_record(&key, name, FT_REG_FILE, &[], INODE_ITEM_KEY);
    item.extend_from_slice(&one_record(&key, b"second", FT_DIR, &[], INODE_ITEM_KEY));

    // DIR_INDEX is not keyed by hash, so both records are accepted
    let r = parse_dir_records(&key, &item).expect("two records must parse");
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].name, b"first");
    assert_eq!(r[1].name, b"second");
    assert_eq!(r[1].ftype, FT_DIR);
}

/// The judges are present. Externally judged cases skip without `btrfs-progs`,
/// so this one fails, once per file. See `common::require_tools`
#[test]
fn the_external_judges_are_here() {
    common::require_tools();
}
