// The chunk map and the self-verifying check
//
// `the_chunk_root_is_where_the_map_says`: each tree node has its logical
// address at header offset 48. Map the chunk root, read `nodesize` bytes there,
// and ask the block where it lives.
//
// That alone is not enough: the chunk root is 16 KiB into its chunk, so masking
// the offset to 16 bits still lands on it.
// `the_offset_arithmetic_holds_across_the_chunk` uses addresses far enough in
// for each offset bit to matter.

mod common;

use btrfs::chunk::{map_from_sys_array, Profile, STRIPE_LEN};
use btrfs::structs::*;
use btrfs::{check_superblock, Superblock};
use common::Image;

/// A tree node's header, the first 101 bytes of each block; only what this file
/// needs
#[allow(dead_code)]
mod header {
    pub const CSUM: usize = 0;
    pub const FSID: usize = 32;
    pub const BYTENR: usize = 48;
    pub const GENERATION: usize = 80;
    pub const OWNER: usize = 88;
    pub const NRITEMS: usize = 96;
    pub const LEVEL: usize = 100;
}

fn image_and_super(tag: &str) -> Option<(Image, Superblock, btrfs::Geometry)> {
    let img = Image::new(tag, None)?;
    let sb = img.read_super();
    let geo = check_superblock(&sb, Some(SUPER_INFO_OFFSET)).expect("a fresh volume must validate");
    Some((img, sb, geo))
}

macro_rules! fixture {
    ($tag:expr) => {
        match image_and_super($tag) {
            Some(t) => t,
            None => {
                eprintln!("btrfs-progs not available - skipping");
                return;
            }
        }
    };
}

/// The bootstrap array parses into a map of system chunks on `mkfs`'s profile
#[test]
fn the_bootstrap_array_becomes_a_map() {
    let (_img, sb, geo) = fixture!("map");
    let map = map_from_sys_array(&sb, geo.sectorsize).expect("sys_chunk_array must map");

    assert!(!map.is_empty(), "the bootstrap array describes at least one chunk");
    for c in map.iter() {
        assert!(c.is_system(), "only system chunks belong in the bootstrap array");
        assert_eq!(c.length % geo.sectorsize as u64, 0);
        // This machine's mkfs default, asserted so a change fails here
        assert_eq!(c.profile, Profile::Dup, "mkfs makes SYSTEM chunks DUP on one device");
        assert_eq!(c.stripes.len(), 2, "DUP is two copies");
        assert_ne!(
            c.stripes[0].offset, c.stripes[1].offset,
            "the two copies are at different places on the same device"
        );
        assert_eq!(
            c.stripes[0].devid, c.stripes[1].devid,
            "and DUP keeps both on one device - that is what makes it DUP"
        );
    }
}

/// Maps the chunk root, reads what is there, and asks the block its address
#[test]
fn the_chunk_root_is_where_the_map_says() {
    let (img, sb, geo) = fixture!("root");
    let map = map_from_sys_array(&sb, geo.sectorsize).expect("sys_chunk_array must map");

    let logical = geo.chunk_root;
    let phys = map
        .map(logical, 0)
        .unwrap_or_else(|e| panic!("chunk root {logical} is not in the bootstrap map: {e:?}"));

    assert!(
        phys.len >= geo.nodesize as u64,
        "a whole node has to be contiguous from there: {} < {}",
        phys.len,
        geo.nodesize
    );

    let mut node = vec![0u8; geo.nodesize as usize];
    img.read_at(phys.offset, &mut node);

    let says = u64_at(&node, header::BYTENR).unwrap();
    assert_eq!(
        says, logical,
        "the block at the mapped place thinks it lives at {says}, not {logical} - the mapping is wrong"
    );

    // It belongs to this filesystem, catching a block of another volume on the
    // disk
    assert_eq!(
        &node[header::FSID..header::FSID + FSID_SIZE],
        sb.fsid(),
        "the block carries another filesystem's uuid"
    );
    assert_eq!(
        u8_at(&node, header::LEVEL).unwrap(),
        geo.chunk_root_level,
        "the superblock and the node disagree about the tree's depth"
    );
    assert!(u32_at(&node, header::NRITEMS).unwrap() > 0, "an empty chunk root");
}

/// The checksum proves the bytes too: crc32c from byte 32 on, as for the
/// superblock
#[test]
fn the_node_we_read_passes_its_own_checksum() {
    let (img, sb, geo) = fixture!("csum");
    let map = map_from_sys_array(&sb, geo.sectorsize).unwrap();
    let phys = map.map(geo.chunk_root, 0).unwrap();

    let mut node = vec![0u8; geo.nodesize as usize];
    img.read_at(phys.offset, &mut node);

    let stored = u32_at(&node, header::CSUM).unwrap();
    let computed = fshost::crc32c::crc32c(0, &node[CSUM_SIZE..]);
    assert_eq!(
        stored, computed,
        "the chunk root's checksum does not match its bytes - either the map is \
         wrong or the checksum convention is"
    );
}

/// Both copies of a DUP chunk hold the same bytes, independently checking
/// stripe offsets
#[test]
fn both_copies_of_a_dup_chunk_are_the_same_bytes() {
    let (img, sb, geo) = fixture!("dup");
    let map = map_from_sys_array(&sb, geo.sectorsize).unwrap();
    let logical = geo.chunk_root;

    let a = map.map(logical, 0).unwrap();
    let b = map.map(logical, 1).unwrap();
    assert_ne!(a.offset, b.offset, "two copies at one place is not two copies");

    let mut first = vec![0u8; geo.nodesize as usize];
    let mut second = vec![0u8; geo.nodesize as usize];
    img.read_at(a.offset, &mut first);
    img.read_at(b.offset, &mut second);
    assert_eq!(first, second, "the two DUP copies differ");
}

/// Every offset bit reaches the answer, walking far enough for high bits to
/// matter
#[test]
fn the_offset_arithmetic_holds_across_the_chunk() {
    let (_img, sb, geo) = fixture!("arith");
    let map = map_from_sys_array(&sb, geo.sectorsize).unwrap();
    let c = map.iter().next().unwrap().clone();
    assert!(
        c.length > STRIPE_LEN * 4,
        "this case needs a chunk several stripes long, have {}",
        c.length
    );

    for delta in [
        0u64,
        geo.sectorsize as u64,
        STRIPE_LEN - 1,
        STRIPE_LEN,
        STRIPE_LEN + 1,
        STRIPE_LEN * 3 + 4096,
        c.length - geo.sectorsize as u64,
    ] {
        let logical = c.start + delta;
        for mirror in 0..c.profile.copies() {
            let p = map.map(logical, mirror).unwrap();
            assert_eq!(
                p.offset,
                c.stripes[mirror as usize].offset + delta,
                "copy {mirror} at +{delta}: physical must be the stripe's offset \
                 plus the whole distance into the chunk"
            );
            assert_eq!(p.len, c.length - delta, "+{delta}: contiguous to the chunk end");
            assert_eq!(p.devid, c.stripes[mirror as usize].devid);
        }
    }
}

/// A missing mirror is an error, not copy zero
#[test]
fn asking_for_a_copy_that_is_not_there_is_an_error() {
    let (_img, sb, geo) = fixture!("mirror");
    let map = map_from_sys_array(&sb, geo.sectorsize).unwrap();
    assert!(map.map(geo.chunk_root, 2).is_err());
    assert!(map.map(geo.chunk_root, u16::MAX).is_err());
}

/// An address outside all chunks has no answer
#[test]
fn an_address_in_no_chunk_is_not_found() {
    let (_img, sb, geo) = fixture!("gap");
    let map = map_from_sys_array(&sb, geo.sectorsize).unwrap();

    let c = map.iter().next().unwrap();
    assert!(map.map(c.start.wrapping_sub(1), 0).is_err(), "one byte before");
    assert!(map.map(c.end(), 0).is_err(), "one byte past the end");
    assert!(map.map(u64::MAX, 0).is_err());
}

/// `len` is how much is contiguous, shrinking toward the chunk's end
#[test]
fn the_contiguous_length_stops_at_the_chunk_end() {
    let (_img, sb, geo) = fixture!("len");
    let map = map_from_sys_array(&sb, geo.sectorsize).unwrap();
    let c = map.iter().next().unwrap().clone();

    let at_start = map.map(c.start, 0).unwrap();
    assert_eq!(at_start.len, c.length);

    let near_end = c.end() - geo.sectorsize as u64;
    let there = map.map(near_end, 0).unwrap();
    assert_eq!(there.len, geo.sectorsize as u64);
    assert_eq!(
        there.offset,
        c.stripes[0].offset + (near_end - c.start),
        "physical is the stripe's offset plus the distance into the chunk"
    );
}

/// The shapes `tree-checker.c` refuses
#[test]
fn hostile_chunks_are_refused() {
    let (_img, _sb, geo) = fixture!("hostile");
    let ss = geo.sectorsize;

    // A well-formed DUP chunk to mutilate, built the way mkfs builds one
    let good = |f: &mut dyn FnMut(&mut [u8])| -> Vec<u8> {
        let mut buf = vec![0u8; CHUNK_SIZE + STRIPE_SIZE];
        buf[chunk::LENGTH..chunk::LENGTH + 8].copy_from_slice(&(8u64 << 20).to_le_bytes());
        buf[chunk::STRIPE_LEN..chunk::STRIPE_LEN + 8].copy_from_slice(&STRIPE_LEN.to_le_bytes());
        let ty = BLOCK_GROUP_SYSTEM | BLOCK_GROUP_DUP;
        buf[chunk::TYPE..chunk::TYPE + 8].copy_from_slice(&ty.to_le_bytes());
        buf[chunk::SECTOR_SIZE..chunk::SECTOR_SIZE + 4].copy_from_slice(&ss.to_le_bytes());
        buf[chunk::NUM_STRIPES..chunk::NUM_STRIPES + 2].copy_from_slice(&2u16.to_le_bytes());
        buf[chunk::SUB_STRIPES..chunk::SUB_STRIPES + 2].copy_from_slice(&1u16.to_le_bytes());
        f(&mut buf);
        buf
    };

    // The unmutilated one has to parse, or nothing below proves anything
    let base = good(&mut |_| {});
    btrfs::chunk::parse_chunk(&base, 0, 1 << 20, ss).expect("the control case must parse");

    let cases: Vec<(&str, Box<dyn Fn(&mut [u8])>)> = vec![
        (
            "zero stripes - the count the size arithmetic divides by",
            Box::new(|b: &mut [u8]| {
                b[chunk::NUM_STRIPES..chunk::NUM_STRIPES + 2].copy_from_slice(&0u16.to_le_bytes())
            }),
        ),
        (
            "a stripe count that does not match the profile",
            Box::new(|b: &mut [u8]| {
                b[chunk::NUM_STRIPES..chunk::NUM_STRIPES + 2].copy_from_slice(&3u16.to_le_bytes())
            }),
        ),
        (
            "zero length - a chunk that covers nothing but claims an address",
            Box::new(|b: &mut [u8]| {
                b[chunk::LENGTH..chunk::LENGTH + 8].copy_from_slice(&0u64.to_le_bytes())
            }),
        ),
        (
            "a length that is not a whole number of sectors",
            Box::new(|b: &mut [u8]| {
                b[chunk::LENGTH..chunk::LENGTH + 8].copy_from_slice(&((8u64 << 20) + 1).to_le_bytes())
            }),
        ),
        (
            "a length large enough to wrap the logical space",
            Box::new(|b: &mut [u8]| {
                b[chunk::LENGTH..chunk::LENGTH + 8].copy_from_slice(&u64::MAX.to_le_bytes())
            }),
        ),
        (
            "a stripe length that is not the one btrfs uses",
            Box::new(|b: &mut [u8]| {
                b[chunk::STRIPE_LEN..chunk::STRIPE_LEN + 8]
                    .copy_from_slice(&(STRIPE_LEN * 2).to_le_bytes())
            }),
        ),
        (
            "no type bit at all",
            Box::new(|b: &mut [u8]| {
                b[chunk::TYPE..chunk::TYPE + 8].copy_from_slice(&BLOCK_GROUP_DUP.to_le_bytes())
            }),
        ),
        (
            "two profile bits, so the layout is two different things",
            Box::new(|b: &mut [u8]| {
                let ty = BLOCK_GROUP_SYSTEM | BLOCK_GROUP_DUP | BLOCK_GROUP_RAID1;
                b[chunk::TYPE..chunk::TYPE + 8].copy_from_slice(&ty.to_le_bytes())
            }),
        ),
        (
            "a system chunk that also claims to hold data",
            Box::new(|b: &mut [u8]| {
                let ty = BLOCK_GROUP_SYSTEM | BLOCK_GROUP_DATA | BLOCK_GROUP_DUP;
                b[chunk::TYPE..chunk::TYPE + 8].copy_from_slice(&ty.to_le_bytes())
            }),
        ),
        (
            "data and metadata in one chunk without mixed-groups",
            Box::new(|b: &mut [u8]| {
                let ty = BLOCK_GROUP_DATA | BLOCK_GROUP_METADATA | BLOCK_GROUP_DUP;
                b[chunk::TYPE..chunk::TYPE + 8].copy_from_slice(&ty.to_le_bytes())
            }),
        ),
        (
            "a sector size that disagrees with the filesystem's",
            Box::new(|b: &mut [u8]| {
                b[chunk::SECTOR_SIZE..chunk::SECTOR_SIZE + 4].copy_from_slice(&512u32.to_le_bytes())
            }),
        ),
    ];

    for (what, mutate) in cases {
        let mut buf = base.clone();
        mutate(&mut buf);
        assert!(
            btrfs::chunk::parse_chunk(&buf, 0, 1 << 20, ss).is_err(),
            "must be refused: {what}"
        );
    }

    // An unaligned start, in the key
    assert!(
        btrfs::chunk::parse_chunk(&base, 0, (1 << 20) + 1, ss).is_err(),
        "must be refused: a chunk starting off a sector boundary"
    );
}

/// Striping and parity are refused, as mirror arithmetic on RAID0 reads another
/// address's valid 64 KiB
#[test]
fn striped_and_parity_profiles_are_refused() {
    let (_img, _sb, geo) = fixture!("striped");
    let ss = geo.sectorsize;

    for (profile, stripes, what) in [
        (BLOCK_GROUP_RAID0, 2u16, "raid0"),
        (BLOCK_GROUP_RAID10, 4, "raid10"),
        (BLOCK_GROUP_RAID5, 3, "raid5"),
        (BLOCK_GROUP_RAID6, 4, "raid6"),
    ] {
        let mut buf = vec![0u8; CHUNK_SIZE + STRIPE_SIZE * stripes as usize];
        buf[chunk::LENGTH..chunk::LENGTH + 8].copy_from_slice(&(8u64 << 20).to_le_bytes());
        buf[chunk::STRIPE_LEN..chunk::STRIPE_LEN + 8].copy_from_slice(&STRIPE_LEN.to_le_bytes());
        let ty = BLOCK_GROUP_DATA | profile;
        buf[chunk::TYPE..chunk::TYPE + 8].copy_from_slice(&ty.to_le_bytes());
        buf[chunk::SECTOR_SIZE..chunk::SECTOR_SIZE + 4].copy_from_slice(&ss.to_le_bytes());
        buf[chunk::NUM_STRIPES..chunk::NUM_STRIPES + 2].copy_from_slice(&stripes.to_le_bytes());
        buf[chunk::SUB_STRIPES..chunk::SUB_STRIPES + 2].copy_from_slice(&1u16.to_le_bytes());

        assert!(
            btrfs::chunk::parse_chunk(&buf, 0, 1 << 20, ss).is_err(),
            "{what} must be refused rather than read with mirror arithmetic"
        );
    }
}

/// Two chunks claiming one address would make the answer depend on insertion
/// order
#[test]
fn overlapping_chunks_are_refused() {
    use btrfs::chunk::{Chunk, ChunkMap, Stripe};
    let mk = |start: u64, length: u64| Chunk {
        start,
        length,
        ty: BLOCK_GROUP_DATA,
        profile: Profile::Single,
        stripes: vec![Stripe { devid: 1, offset: start }],
    };

    let mut map = ChunkMap::new();
    map.insert(mk(1 << 20, 1 << 20)).unwrap();

    assert!(map.insert(mk(1 << 20, 1 << 21)).is_err(), "same start, other length");
    assert!(map.insert(mk((1 << 20) + 4096, 1 << 20)).is_err(), "overlaps from above");
    assert!(map.insert(mk(0, (1 << 20) + 4096)).is_err(), "overlaps from below");

    // The same chunk twice is normal: the bootstrap array and chunk tree both
    // describe system chunks
    map.insert(mk(1 << 20, 1 << 20)).expect("an exact repeat is not a conflict");
    assert_eq!(map.len(), 1);

    // And one that merely touches is fine
    map.insert(mk(2 << 20, 1 << 20)).expect("adjacent is not overlapping");
    assert_eq!(map.len(), 2);
}

/// The judges are present. Externally judged cases skip without `btrfs-progs`,
/// so this one fails, once per file. See `common::require_tools`
#[test]
fn the_external_judges_are_here() {
    common::require_tools();
}
