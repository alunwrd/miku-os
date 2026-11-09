// What this crate thinks is free, against what btrfs-progs thinks is free
//
// A bitmap decoded with each byte's bits reversed still gives runs inside their
// group, ordered, and adding up to the same total; only the boundaries move. So
// two cases ask an outside tool: `the_free_space_agrees_with_btrfs_progs`
// compares our free runs with those `btrfs inspect-internal dump-tree` reads
// from the same tree, and
// `the_space_this_crate_counts_is_the_space_btrfs_progs_counts` compares totals
// with numbers worked out from the extent tree.
//
// `mkfs.btrfs` does not produce the bitmap representation: a group converts
// past the threshold in `set_free_space_tree_thresholds`, a dozen or more free
// runs in one group, and mkfs reaches 4 on a 1 GiB image with three thousand
// files. Fragmenting a mounted volume needs root. So
// `the_bitmap_representation_reads_the_same_space` re-encodes a real volume's
// free space tree into bitmaps in place and runs `btrfs check`, which compares
// the free space tree with the extent tree. A flipped bit makes it report
// "there is no free space entry for ...".

mod common;

use btrfs::error::BtrfsError;
use btrfs::space::*;
use btrfs::structs::{BLOCK_GROUP_DATA, BLOCK_GROUP_METADATA, BLOCK_GROUP_SYSTEM};
use btrfs::{Key, Volume};
use common::{into_bitmaps, tool, Image, IMG_BYTES};
use fshost::{BlockHost, HostError};
use std::path::{Path, PathBuf};
use std::process::Command;

const EVERY_KIND: u64 = BLOCK_GROUP_DATA | BLOCK_GROUP_METADATA | BLOCK_GROUP_SYSTEM;

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

fn volume_over(bytes: &[u8]) -> Volume<FileHost> {
    let mut vol = Volume::open(FileHost { data: bytes.to_vec() }, 0).expect("the volume must open");
    vol.load_chunk_tree().expect("the chunk tree must load");
    vol
}

fn volume_of(img: &Image) -> Volume<FileHost> {
    volume_over(&std::fs::read(&img.path).expect("cannot read image"))
}

/// An image with `mkfs` options. `common::Image::new` takes none, so the result
/// is wrapped back into an `Image` for its tool runner and cleanup
fn image_with(tag: &str, opts: &[&str]) -> Option<Image> {
    let mkfs = tool("mkfs.btrfs")?;
    let mut path = std::env::temp_dir();
    path.push(format!("btrfs-{tag}-{}.img", std::process::id()));

    let f = std::fs::File::create(&path).expect("cannot create image");
    f.set_len(IMG_BYTES).expect("cannot size image");
    drop(f);

    let out = Command::new(&mkfs)
        .arg("-q")
        .args(opts)
        .arg(&path)
        .output()
        .expect("mkfs.btrfs did not run");
    assert!(
        out.status.success(),
        "mkfs.btrfs {opts:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Some(Image { path })
}

macro_rules! fixture {
    ($tag:expr) => {
        match Image::new($tag, None) {
            Some(i) => i,
            None => {
                eprintln!("btrfs-progs not available - skipping");
                return;
            }
        }
    };
    ($tag:expr, $opts:expr) => {
        match image_with($tag, $opts) {
            Some(i) => i,
            None => {
                eprintln!("btrfs-progs not available - skipping");
                return;
            }
        }
    };
}

// The judge
//
// `dump-tree` prints one line per item, with the key in it:
//
//     item 1 key (13631488 FREE_SPACE_EXTENT 8388608) itemoff 16275 itemsize 0
//
// and a line under it for items with contents to print. The key holds a free
// extent's length, the same field the driver reads.

fn keys_of(dump: &str) -> Vec<(u64, String, u64)> {
    let mut out = Vec::new();
    for line in dump.lines() {
        let Some(rest) = line.split_once("key (") else {
            continue;
        };
        let Some((inside, _)) = rest.1.split_once(')') else {
            continue;
        };
        let f: Vec<&str> = inside.split_whitespace().collect();
        if f.len() != 3 {
            continue;
        }
        let (Ok(objectid), Ok(offset)) = (f[0].parse::<u64>(), f[2].parse::<u64>()) else {
            continue;
        };
        out.push((objectid, f[1].to_string(), offset));
    }
    out
}

/// Whether the tool's flags word names this type. Matches `|`-separated words,
/// since "METADATA|DUP" contains "DATA"
fn names(flags: &str, word: &str) -> bool {
    flags.split('|').any(|w| w == word)
}

/// The tool's block groups as `(start, length, used, flags)`, flags in the
/// tool's words ("DATA|single", "METADATA|DUP")
fn tool_block_groups(img: &Image, from_block_group_tree: bool) -> Vec<(u64, u64, u64, String)> {
    let tree = if from_block_group_tree { "block-group" } else { "extent" };
    let dump = img.btrfs(&["inspect-internal", "dump-tree", "-t", tree]);

    let mut out: Vec<(u64, u64, u64, String)> = Vec::new();
    let mut pending: Option<(u64, u64)> = None;
    for line in dump.lines() {
        if let Some((_, rest)) = line.split_once("key (") {
            pending = None;
            if let Some((inside, _)) = rest.split_once(')') {
                let f: Vec<&str> = inside.split_whitespace().collect();
                if f.len() == 3 && f[1] == "BLOCK_GROUP_ITEM" {
                    pending = Some((f[0].parse().unwrap(), f[2].parse().unwrap()));
                }
            }
            continue;
        }
        let t = line.trim();
        if let (Some((start, length)), true) = (pending, t.starts_with("block group used ")) {
            let f: Vec<&str> = t.split_whitespace().collect();
            let used: u64 = f[3].parse().unwrap();
            let flags = f.last().unwrap().to_string();
            out.push((start, length, used, flags));
            pending = None;
        }
    }
    out.sort_by_key(|g| g.0);
    out
}

/// The tool's free space as `(group start, group length, runs)`, in free space
/// tree order
fn tool_free_space(img: &Image) -> Vec<(u64, u64, Vec<(u64, u64)>)> {
    let dump = img.btrfs(&["inspect-internal", "dump-tree", "-t", "free-space"]);
    let mut out: Vec<(u64, u64, Vec<(u64, u64)>)> = Vec::new();
    for (objectid, kind, offset) in keys_of(&dump) {
        match kind.as_str() {
            "FREE_SPACE_INFO" => out.push((objectid, offset, Vec::new())),
            "FREE_SPACE_EXTENT" => out
                .last_mut()
                .expect("a free space extent before any info item")
                .2
                .push((objectid, offset)),
            "FREE_SPACE_BITMAP" => panic!("this fixture was expected to use extents"),
            _ => {}
        }
    }
    out
}

// the cases

/// External judge: every free run this crate reports against every free run
/// `btrfs inspect-internal dump-tree` reports, on a real volume
#[test]
fn the_free_space_agrees_with_btrfs_progs() {
    let img = fixture!("space-judge");
    let mut vol = volume_of(&img);
    let info = read_free_space(&mut vol).expect("a fresh volume must have a free space model");

    // What `mkfs.btrfs` does today, so a changed default fails here and not in
    // the block group parser
    assert!(
        info.from_block_group_tree,
        "mkfs.btrfs has set BLOCK_GROUP_TREE by default since 6.x"
    );

    let theirs = tool_free_space(&img);
    assert!(!theirs.is_empty(), "the tool found no free space tree at all");
    assert_eq!(
        info.groups.len(),
        theirs.len(),
        "we found {} block groups, the tool found {}",
        info.groups.len(),
        theirs.len()
    );

    let mut total = 0u64;
    for (ours, (start, length, runs)) in info.groups.iter().zip(theirs.iter()) {
        assert_eq!((ours.start, ours.length), (*start, *length));
        assert_eq!(
            &ours.runs, runs,
            "group at {start}: our free runs are not the ones the tool reports"
        );
        total += runs.iter().map(|&(_, l)| l).sum::<u64>();
    }

    assert_eq!(
        info.total_free(EVERY_KIND),
        total,
        "total free space disagrees with the tool"
    );

    // The numbers, for the test output
    eprintln!(
        "judge: {} groups, {} bytes free in total (data {}, metadata {}, system {})",
        info.groups.len(),
        total,
        info.total_free(BLOCK_GROUP_DATA),
        info.total_free(BLOCK_GROUP_METADATA),
        info.total_free(BLOCK_GROUP_SYSTEM),
    );

    // The other half of the model: the block group items
    let bgs = tool_block_groups(&img, info.from_block_group_tree);
    assert_eq!(bgs.len(), info.groups.len());
    for (ours, (start, length, used, flags)) in info.groups.iter().zip(bgs.iter()) {
        assert_eq!((ours.start, ours.length, ours.used), (*start, *length, *used));
        assert_eq!(ours.is_data(), names(flags, "DATA"), "group at {start}");
        assert_eq!(ours.is_metadata(), names(flags, "METADATA"), "group at {start}");
        assert_eq!(ours.is_system(), names(flags, "SYSTEM"), "group at {start}");
    }

    // Per kind, against the tool's own division of the groups
    for (mask, word) in [
        (BLOCK_GROUP_DATA, "DATA"),
        (BLOCK_GROUP_METADATA, "METADATA"),
        (BLOCK_GROUP_SYSTEM, "SYSTEM"),
    ] {
        let want: u64 = bgs
            .iter()
            .zip(theirs.iter())
            .filter(|((_, _, _, flags), _)| names(flags, word))
            .map(|(_, (_, _, runs))| runs.iter().map(|&(_, l)| l).sum::<u64>())
            .sum();
        assert_eq!(info.total_free(mask), want, "{word} free space");
    }
}

/// External judge for the totals: what this crate says is used and free,
/// against numbers btrfs-progs worked out without the free space tree
///
/// `btrfs check` prints `found N bytes used` after walking the extent tree;
/// `btrfs filesystem show --raw` prints what the superblock records. Our `used`
/// comes from the block group items, our free runs from the free space tree,
/// and `GroupScan::finish` refuses a group unless the two add up, so pinning
/// `used` to the extent tree's answer also checks the free space tree.
///
/// Two units. `found N bytes used`, `FS bytes used` and a block group item's
/// `used` are logical bytes, as are `total_free` and the addresses from
/// `find_free`. `devid ... used` is device bytes: a chunk's logical length
/// times the copies its profile keeps. `mkfs.btrfs` on a plain file makes
/// metadata and system DUP, so the two differ.
#[test]
fn the_space_this_crate_counts_is_the_space_btrfs_progs_counts() {
    let img = fixture!("space-totals");
    let mut vol = volume_of(&img);
    let info = read_free_space(&mut vol).expect("a fresh volume must have a free space model");

    let used: u64 = info.groups.iter().map(|g| g.used).sum();
    let length: u64 = info.groups.iter().map(|g| g.length).sum();

    // The extent tree's own count, which nothing in this crate has read
    let check = img.btrfs(&["check"]);
    assert!(check.contains("no error found"), "btrfs check rejected the fixture:\n{check}");
    assert_eq!(
        number_after(&check, "bytes used", "found"),
        used,
        "btrfs check walked the extent tree and found a different number of bytes in use"
    );

    // The superblock's copy of the same fact
    let show = img.btrfs(&["filesystem", "show", "--raw"]);
    assert_eq!(
        number_after(&show, "FS bytes used", "used"),
        used,
        "the superblock records a different number of bytes in use"
    );

    // What is not used is free, in logical bytes
    assert_eq!(
        info.total_free(EVERY_KIND),
        length - used,
        "the free runs do not account for the part of the volume that is not in use"
    );

    // The device side, where the profile factor lives. `devid ... used` is disk
    // occupied by chunks: a DUP group costs twice its logical length, a single
    // group once
    let raw: u64 = info
        .groups
        .iter()
        .map(|g| {
            let chunk = vol.chunks.find(g.start).expect("every group is a chunk");
            g.length * chunk.profile.copies() as u64
        })
        .sum();
    assert_eq!(
        number_after(&show, "devid", "used"),
        raw,
        "the chunks take a different number of device bytes than their profiles say"
    );
    assert_ne!(raw, length, "a default volume has DUP groups, so the two currencies differ");

    eprintln!(
        "totals: {used} bytes used, {} free, {length} logical in groups, {raw} on the device",
        length - used
    );
}

/// The integer after `word` on the first line of `text` containing `line_has`.
/// The tools print these numbers in prose
fn number_after(text: &str, line_has: &str, word: &str) -> u64 {
    for line in text.lines() {
        if !line.contains(line_has) {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if let Some(i) = f.iter().position(|w| *w == word) {
            if let Some(Ok(n)) = f.get(i + 1).map(|w| w.parse::<u64>()) {
                return n;
            }
        }
    }
    panic!("no `{word} <number>` on a line holding `{line_has}`:\n{text}");
}

/// Holds on every group of every volume: what is not used is free, and the free
/// runs are inside the group, in order, and not touching
#[test]
fn every_group_accounts_for_itself() {
    let img = fixture!("space-arith");
    let mut vol = volume_of(&img);
    let info = read_free_space(&mut vol).expect("a fresh volume must have a free space model");
    let sector = info.sectorsize as u64;

    for g in &info.groups {
        assert_eq!(
            g.length - g.used,
            g.free(),
            "group at {}: {} used out of {}, but the runs add up to {}",
            g.start,
            g.used,
            g.length,
            g.free()
        );

        let mut prev_end = g.start;
        for &(start, len) in &g.runs {
            assert!(start >= prev_end, "group at {}: runs out of order", g.start);
            assert!(len > 0);
            assert_eq!(start % sector, 0);
            assert_eq!(len % sector, 0);
            assert!(
                start + len <= g.end(),
                "group at {}: a run reaches past the group",
                g.start
            );
            prev_end = start + len;
        }
    }
}

/// A volume from before `BLOCK_GROUP_TREE`: block group items sit in the extent
/// tree, told apart by key type
#[test]
fn the_extent_tree_layout_is_read_too() {
    let img = fixture!("space-old", &["-O", "^block-group-tree"]);
    let mut vol = volume_of(&img);
    let info = read_free_space(&mut vol).expect("an older volume must still have a model");

    assert!(
        !info.from_block_group_tree,
        "this fixture was made without the feature, so the groups are in the extent tree"
    );

    let theirs = tool_free_space(&img);
    assert_eq!(info.groups.len(), theirs.len());
    for (ours, (start, length, runs)) in info.groups.iter().zip(theirs.iter()) {
        assert_eq!((ours.start, ours.length), (*start, *length));
        assert_eq!(&ours.runs, runs, "group at {start}");
    }

    let bgs = tool_block_groups(&img, false);
    assert_eq!(bgs.len(), info.groups.len());
    for (ours, (_, _, used, _)) in info.groups.iter().zip(bgs.iter()) {
        assert_eq!(ours.used, *used);
    }
}

/// Without a free space tree there is no answer, which differs from nothing
/// being free
#[test]
fn a_volume_without_a_free_space_tree_is_refused() {
    let img = fixture!("space-none", &["-O", "^free-space-tree"]);
    let mut vol = volume_of(&img);
    assert_eq!(read_free_space(&mut vol), Err(BtrfsError::Unsupported));
}

/// An address from `find_free` is aligned, inside a run reported free, and in a
/// group of the requested kind
#[test]
fn find_free_lands_in_a_run_of_the_right_kind() {
    let img = fixture!("space-find");
    let mut vol = volume_of(&img);
    let info = read_free_space(&mut vol).expect("a fresh volume must have a free space model");
    let sector = info.sectorsize as u64;

    for kind in [BLOCK_GROUP_DATA, BLOCK_GROUP_METADATA] {
        for &align in &[sector, 65536u64, 1 << 20] {
            for &len in &[sector, 64 * 1024, 1 << 20] {
                let at = info
                    .find_free(kind, len, align)
                    .unwrap_or_else(|| panic!("nothing free for {len} bytes at align {align}"));
                assert_eq!(at % align, 0, "{at} is not aligned to {align}");

                let g = info
                    .groups
                    .iter()
                    .find(|g| at >= g.start && at < g.end())
                    .expect("the address is in no block group at all");
                assert!(g.flags & kind != 0, "the address is in a group of the wrong kind");
                assert!(
                    g.runs
                        .iter()
                        .any(|&(s, l)| at >= s && at + len <= s + l),
                    "{at}+{len} is not inside any free run of the group at {}",
                    g.start
                );
            }
        }
    }

    // More than the whole volume, and an alignment that is not a power of two.
    // No address satisfies either
    assert_eq!(info.find_free(EVERY_KIND, IMG_BYTES * 2, 4096), None);
    assert_eq!(info.find_free(EVERY_KIND, 4096, 3), None);
    assert_eq!(info.find_free(EVERY_KIND, 0, 4096), None);
    // A kind no group has
    assert_eq!(info.find_free(1 << 40, 4096, 4096), None);
}

// The bitmap case
//
// Re-encodes a real volume's free space tree, so the bitmap decoder reads a
// real filesystem.

fn btrfs_check(path: &Path) -> (bool, String) {
    let btrfs = tool("btrfs").expect("btrfs tool vanished");
    let out = Command::new(btrfs).arg("check").arg(path).output().expect("btrfs did not run");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

/// A file that goes away with the test that made it
struct Scratch {
    path: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The same volume in bitmaps: `btrfs check` accepts them, and this crate reads
/// the same runs as from the extent items
#[test]
fn the_bitmap_representation_reads_the_same_space() {
    let img = fixture!("space-bitmap");
    let bytes = std::fs::read(&img.path).expect("cannot read image");

    let mut vol = volume_over(&bytes);
    let extents = read_free_space(&mut vol).expect("the extent representation must read");

    let rewritten = into_bitmaps(&bytes);

    let scratch = Scratch { path: img.path.with_extension("bitmaps.img") };
    std::fs::write(&scratch.path, &rewritten).expect("cannot write the rewritten image");

    // `btrfs check` loads the free space tree, bitmaps included, and compares
    // it with the extent tree. A wrong bitmap gives "there is no free space
    // entry for ..."
    let (ok, text) = btrfs_check(&scratch.path);
    assert!(ok && text.contains("no error found"), "btrfs check rejected the bitmaps:\n{text}");

    let mut vol = volume_over(&rewritten);
    let bitmaps = read_free_space(&mut vol).expect("the bitmap representation must read");

    assert_eq!(bitmaps.groups.len(), extents.groups.len());
    for (b, e) in bitmaps.groups.iter().zip(extents.groups.iter()) {
        assert_eq!(b, e, "group at {}: the two representations disagree", e.start);
    }
    assert_eq!(bitmaps.total_free(EVERY_KIND), extents.total_free(EVERY_KIND));
    eprintln!(
        "bitmaps: {} groups, {} bytes free, decoded from {} bitmap items",
        bitmaps.groups.len(),
        bitmaps.total_free(EVERY_KIND),
        bitmaps.groups.len(),
    );
}

// Hostile items
//
// `GroupScan` takes items as the walk hands them over, so these need no
// filesystem: a block group, an info item, and whatever a hostile tree put
// under it.

fn group(start: u64, length: u64, used: u64, flags: u64) -> FreeSpace {
    FreeSpace { start, length, used, flags, runs: Vec::new() }
}

fn info(count: u32, flags: u32) -> Vec<u8> {
    let mut v = Vec::from(count.to_le_bytes());
    v.extend_from_slice(&flags.to_le_bytes());
    v
}

fn key(objectid: u64, kind: u8, offset: u64) -> Key {
    Key { objectid, kind, offset }
}

const GROUP_START: u64 = 1 << 20;
const GROUP_LEN: u64 = 8 << 20;
const SECTOR: u32 = 4096;

fn scan(g: FreeSpace, count: u32, flags: u32) -> GroupScan {
    let k = key(g.start, FREE_SPACE_INFO_KEY, g.length);
    GroupScan::new(g, &k, &info(count, flags), SECTOR).expect("the info item must be accepted")
}

/// The info item counts the runs. A mismatch means items were lost or gained,
/// or bitmaps decoded into the wrong number of runs
#[test]
fn an_extent_count_that_disagrees_is_refused() {
    let g = group(GROUP_START, GROUP_LEN, 0, BLOCK_GROUP_DATA);
    let mut s = scan(g, 2, 0);
    s.item(&key(GROUP_START, FREE_SPACE_EXTENT_KEY, GROUP_LEN), &[])
        .expect("one whole-group run is a legal item");
    assert_eq!(s.finish(), Err(BtrfsError::Corrupt));

    // ...and the same items with the count the tree really holds are fine
    let g = group(GROUP_START, GROUP_LEN, 0, BLOCK_GROUP_DATA);
    let mut s = scan(g, 1, 0);
    s.item(&key(GROUP_START, FREE_SPACE_EXTENT_KEY, GROUP_LEN), &[]).unwrap();
    assert_eq!(s.finish().unwrap().free(), GROUP_LEN);
}

/// A run past its group would be handed out as free space of the next group, or
/// of no chunk
#[test]
fn a_free_run_outside_its_group_is_refused() {
    let g = group(GROUP_START, GROUP_LEN, 0, BLOCK_GROUP_DATA);
    let mut s = scan(g, 1, 0);
    assert_eq!(
        s.item(&key(GROUP_START + GROUP_LEN - 4096, FREE_SPACE_EXTENT_KEY, 8192), &[]),
        Err(BtrfsError::Corrupt)
    );

    // Before the group, too
    let g = group(GROUP_START, GROUP_LEN, 0, BLOCK_GROUP_DATA);
    let mut s = scan(g, 1, 0);
    assert_eq!(
        s.item(&key(GROUP_START - 4096, FREE_SPACE_EXTENT_KEY, 8192), &[]),
        Err(BtrfsError::Corrupt)
    );
}

/// Two runs over one byte are counted twice by `total_free` and handed out
/// twice by `find_free`
#[test]
fn two_overlapping_runs_are_refused() {
    let g = group(GROUP_START, GROUP_LEN, GROUP_LEN - 12288, BLOCK_GROUP_DATA);
    let mut s = scan(g, 2, 0);
    s.item(&key(GROUP_START, FREE_SPACE_EXTENT_KEY, 8192), &[]).unwrap();
    assert_eq!(
        s.item(&key(GROUP_START + 4096, FREE_SPACE_EXTENT_KEY, 8192), &[]),
        Err(BtrfsError::Corrupt)
    );
}

/// An item of the representation the info item did not name
#[test]
fn an_item_of_the_other_representation_is_refused() {
    let g = group(GROUP_START, GROUP_LEN, 0, BLOCK_GROUP_DATA);
    let mut s = scan(g, 1, 0);
    assert_eq!(
        s.item(&key(GROUP_START, FREE_SPACE_BITMAP_KEY, GROUP_LEN), &[0; 256]),
        Err(BtrfsError::Corrupt)
    );

    let g = group(GROUP_START, GROUP_LEN, 0, BLOCK_GROUP_DATA);
    let mut s = scan(g, 1, FREE_SPACE_USING_BITMAPS);
    assert_eq!(
        s.item(&key(GROUP_START, FREE_SPACE_EXTENT_KEY, GROUP_LEN), &[]),
        Err(BtrfsError::Corrupt)
    );
}

/// A bitmap holds one bit per sector of the range its key claims. A shorter one
/// makes the decoder read past the item; a longer one means key and item
/// describe different ranges
#[test]
fn a_bitmap_of_the_wrong_length_is_refused() {
    for size in [0usize, 128, 255, 257, 512] {
        let g = group(GROUP_START, GROUP_LEN, 0, BLOCK_GROUP_DATA);
        let mut s = scan(g, 1, FREE_SPACE_USING_BITMAPS);
        let item = vec![0xFFu8; size];
        assert_eq!(
            s.item(&key(GROUP_START, FREE_SPACE_BITMAP_KEY, 8 << 20), &item),
            Err(BtrfsError::Corrupt),
            "a {size}-byte item for an 8 MiB range"
        );
    }
}

/// The bits stated by hand, so the decoder's convention is not checked against
/// itself
///
/// Sixteen sectors of 4 KiB, two bytes, least significant bit first in each
/// byte: bit 0 of the second byte is sector 8, not sector 15
#[test]
fn a_bitmap_decodes_to_the_runs_its_bits_name() {
    const LEN: u64 = 16 * 4096;
    // sectors 0, 2, 3 | sectors 8 and 15
    let bits = [0b0000_1101u8, 0b1000_0001u8];
    let want = vec![
        (GROUP_START, 4096),
        (GROUP_START + 2 * 4096, 2 * 4096),
        (GROUP_START + 8 * 4096, 4096),
        (GROUP_START + 15 * 4096, 4096),
    ];
    let free: u64 = want.iter().map(|&(_, l)| l).sum();
    assert_eq!(free, 5 * 4096);

    let g = group(GROUP_START, LEN, LEN - free, BLOCK_GROUP_DATA);
    let mut s = scan(g, want.len() as u32, FREE_SPACE_USING_BITMAPS);
    s.item(&key(GROUP_START, FREE_SPACE_BITMAP_KEY, LEN), &bits).unwrap();
    assert_eq!(s.finish().unwrap().runs, want);

    // The same bitmap against a block group saying one more sector is used. The
    // runs are unchanged; this catches a bitmap decoded into plausible nonsense
    let g = group(GROUP_START, LEN, LEN - free + 4096, BLOCK_GROUP_DATA);
    let mut s = scan(g, want.len() as u32, FREE_SPACE_USING_BITMAPS);
    s.item(&key(GROUP_START, FREE_SPACE_BITMAP_KEY, LEN), &bits).unwrap();
    assert_eq!(s.finish(), Err(BtrfsError::Corrupt));
}

/// A run open at the end of one bitmap item does not carry across a gap to the
/// next. The bytes between are described by nothing
#[test]
fn a_run_does_not_carry_across_a_gap_between_bitmaps() {
    const LEN: u64 = 16 * 4096;
    let all_free = [0xFFu8, 0xFF];

    // Two bitmap items, each 16 free sectors, with 16 uncovered sectors between
    let g = group(GROUP_START, 3 * LEN, 3 * LEN - 2 * LEN, BLOCK_GROUP_DATA);
    let mut s = scan(g, 2, FREE_SPACE_USING_BITMAPS);
    s.item(&key(GROUP_START, FREE_SPACE_BITMAP_KEY, LEN), &all_free).unwrap();
    s.item(&key(GROUP_START + 2 * LEN, FREE_SPACE_BITMAP_KEY, LEN), &all_free).unwrap();
    let out = s.finish().expect("two runs, two items, and the count says two");
    assert_eq!(out.runs, vec![(GROUP_START, LEN), (GROUP_START + 2 * LEN, LEN)]);

    // Adjacent items are one run, as the kernel counts them, and the info
    // item's count follows that
    let g = group(GROUP_START, 2 * LEN, 0, BLOCK_GROUP_DATA);
    let mut s = scan(g, 1, FREE_SPACE_USING_BITMAPS);
    s.item(&key(GROUP_START, FREE_SPACE_BITMAP_KEY, LEN), &all_free).unwrap();
    s.item(&key(GROUP_START + LEN, FREE_SPACE_BITMAP_KEY, LEN), &all_free).unwrap();
    assert_eq!(s.finish().unwrap().runs, vec![(GROUP_START, 2 * LEN)]);
}

/// The info item: wrong size, unknown flag, a key naming another group, a count
/// larger than the group has sectors
#[test]
fn a_bad_free_space_info_is_refused() {
    let g = || group(GROUP_START, GROUP_LEN, 0, BLOCK_GROUP_DATA);
    let k = key(GROUP_START, FREE_SPACE_INFO_KEY, GROUP_LEN);

    assert_eq!(GroupScan::new(g(), &k, &[0; 4], SECTOR).err(), Some(BtrfsError::Corrupt));
    assert_eq!(GroupScan::new(g(), &k, &[0; 16], SECTOR).err(), Some(BtrfsError::Corrupt));
    assert_eq!(GroupScan::new(g(), &k, &info(1, 0x2), SECTOR).err(), Some(BtrfsError::Corrupt));
    // More runs than fit: runs cannot touch, so each costs a sector and a gap
    // sector
    let sectors = (GROUP_LEN / SECTOR as u64) as u32;
    assert_eq!(
        GroupScan::new(g(), &k, &info(sectors + 1, 0), SECTOR).err(),
        Some(BtrfsError::Corrupt)
    );
    // A key naming another range means the walk and the block group list have
    // come apart, and every later item would be filed under the wrong group
    let elsewhere = key(GROUP_START + 4096, FREE_SPACE_INFO_KEY, GROUP_LEN);
    assert_eq!(GroupScan::new(g(), &elsewhere, &info(1, 0), SECTOR).err(), Some(BtrfsError::Corrupt));
}

/// The block group item, the source of `used`
#[test]
fn a_bad_block_group_item_is_refused() {
    let good = |used: u64, chunk_objectid: u64, flags: u64| {
        let mut v = Vec::from(used.to_le_bytes());
        v.extend_from_slice(&chunk_objectid.to_le_bytes());
        v.extend_from_slice(&flags.to_le_bytes());
        v
    };
    let k = key(GROUP_START, BLOCK_GROUP_ITEM_KEY, GROUP_LEN);

    let item = good(4096, FIRST_CHUNK_TREE_OBJECTID, BLOCK_GROUP_DATA);
    let g = parse_block_group_item(&k, &item, SECTOR).expect("an ordinary item");
    assert_eq!((g.start, g.length, g.used), (GROUP_START, GROUP_LEN, 4096));
    assert!(g.is_data() && !g.is_metadata() && !g.is_system());

    // More used than the group holds. `length - used` underflows below this
    let over = good(GROUP_LEN + 4096, FIRST_CHUNK_TREE_OBJECTID, BLOCK_GROUP_DATA);
    assert_eq!(parse_block_group_item(&k, &over, SECTOR), Err(BtrfsError::Corrupt));

    // Somebody else's chunk tree
    let wrong = good(0, 257, BLOCK_GROUP_DATA);
    assert_eq!(parse_block_group_item(&k, &wrong, SECTOR), Err(BtrfsError::Corrupt));

    // No type at all, and every type at once
    let none = good(0, FIRST_CHUNK_TREE_OBJECTID, 0);
    assert_eq!(parse_block_group_item(&k, &none, SECTOR), Err(BtrfsError::Corrupt));
    let all = good(0, FIRST_CHUNK_TREE_OBJECTID, BLOCK_GROUP_DATA | BLOCK_GROUP_SYSTEM);
    assert_eq!(parse_block_group_item(&k, &all, SECTOR), Err(BtrfsError::Corrupt));

    // Data and metadata together is a MIXED_GROUPS volume, refused at mount: an
    // unsupported format, not a broken item
    let mixed = good(0, FIRST_CHUNK_TREE_OBJECTID, BLOCK_GROUP_DATA | BLOCK_GROUP_METADATA);
    assert_eq!(parse_block_group_item(&k, &mixed, SECTOR), Err(BtrfsError::Unsupported));

    // A zero-length group, and an item that is not the struct
    let zero = key(GROUP_START, BLOCK_GROUP_ITEM_KEY, 0);
    let item = good(0, FIRST_CHUNK_TREE_OBJECTID, BLOCK_GROUP_DATA);
    assert_eq!(parse_block_group_item(&zero, &item, SECTOR), Err(BtrfsError::Corrupt));
    assert_eq!(parse_block_group_item(&k, &item[..16], SECTOR), Err(BtrfsError::Corrupt));
    // `REMAP_TREE` layout, not implemented here
    let mut v2 = item.clone();
    v2.resize(36, 0);
    assert_eq!(parse_block_group_item(&k, &v2, SECTOR), Err(BtrfsError::Unsupported));
}

/// A `FREE_SPACE_EXTENT` is only a key: `objectid` is where the run starts,
/// `offset` is its length, and the body is empty
///
/// `check_free_space_extent` in `tree-checker.c` refuses a leaf over an item
/// with bytes. A driver reading the length from the body would report a free
/// run of whatever a hostile tree put there
#[test]
fn a_free_space_extent_carries_nothing_but_its_key() {
    // The length is the key's, and the key's alone
    let g = group(GROUP_START, GROUP_LEN, GROUP_LEN - 8192, BLOCK_GROUP_DATA);
    let mut s = scan(g, 1, 0);
    s.item(&key(GROUP_START, FREE_SPACE_EXTENT_KEY, 8192), &[])
        .expect("an empty item is what this key type carries");
    assert_eq!(s.finish().unwrap().runs, vec![(GROUP_START, 8192)]);

    // The same key with a body. The middle one is eight bytes naming a length
    // of its own
    for body in [&[0u8][..], &4096u64.to_le_bytes()[..], &[0xFFu8; 24][..]] {
        let g = group(GROUP_START, GROUP_LEN, GROUP_LEN - 8192, BLOCK_GROUP_DATA);
        let mut s = scan(g, 1, 0);
        assert_eq!(
            s.item(&key(GROUP_START, FREE_SPACE_EXTENT_KEY, 8192), body),
            Err(BtrfsError::Corrupt),
            "a {}-byte body under a free space extent",
            body.len()
        );
    }
}

/// A group whose free runs do not add up to `length - used` is refused
///
/// The extent representation: the right number of items, each inside the group,
/// in order, not touching, and one sector too much free space. Only the
/// arithmetic notices
#[test]
fn a_group_whose_runs_do_not_add_up_is_refused() {
    let g = group(GROUP_START, GROUP_LEN, GROUP_LEN - 8192, BLOCK_GROUP_DATA);
    let mut s = scan(g, 2, 0);
    s.item(&key(GROUP_START, FREE_SPACE_EXTENT_KEY, 4096), &[]).unwrap();
    s.item(&key(GROUP_START + 8192, FREE_SPACE_EXTENT_KEY, 8192), &[]).unwrap();
    assert_eq!(s.finish(), Err(BtrfsError::Corrupt));

    // The same two items under a block group item matching them, so only the
    // arithmetic changed
    let g = group(GROUP_START, GROUP_LEN, GROUP_LEN - 12288, BLOCK_GROUP_DATA);
    let mut s = scan(g, 2, 0);
    s.item(&key(GROUP_START, FREE_SPACE_EXTENT_KEY, 4096), &[]).unwrap();
    s.item(&key(GROUP_START + 8192, FREE_SPACE_EXTENT_KEY, 8192), &[]).unwrap();
    assert_eq!(s.finish().unwrap().free(), 12288);
}

/// The judges are present. Externally judged cases skip without `btrfs-progs`,
/// so this one fails, once per file. See `common::require_tools`
#[test]
fn the_external_judges_are_here() {
    common::require_tools();
}
