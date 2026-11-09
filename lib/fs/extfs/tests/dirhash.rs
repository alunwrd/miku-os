// The directory hash against e2fsprogs. An htree index is keyed by each name's
// hash, which must match Linux's bit for bit.
//
// Each vector came from
//
//     debugfs -R "dx_hash -h <version> -s <uuid> <name>"
//
// with e2fsprogs 1.47.4 and the all-zero seed.

use extfs::ext2::dir::{ext2_dx_hash_full, DX_HASH_HALF_MD4, DX_HASH_LEGACY, DX_HASH_TEA};

/// The seed of a filesystem with no `s_hash_seed`: all zero, meaning the
/// standard MD4 initial state
const NO_SEED: [u32; 4] = [0, 0, 0, 0];

fn check(version: u8, name: &str, hash: u32, minor: u32) {
    let got = ext2_dx_hash_full(name.as_bytes(), version, &NO_SEED);
    assert_eq!(
        got.hash, hash,
        "major hash of {name:?} (version {version}): got {:#010x}, e2fsprogs says {hash:#010x}",
        got.hash
    );
    assert_eq!(
        got.minor, minor,
        "minor hash of {name:?} (version {version}): got {:#010x}, e2fsprogs says {minor:#010x}",
        got.minor
    );
}

#[test]
fn legacy_matches_e2fsprogs() {
    check(DX_HASH_LEGACY, "a", 0xe74b53e2, 0x0);
    check(DX_HASH_LEGACY, "ab", 0xda0d8cd0, 0x0);
    check(DX_HASH_LEGACY, "abc", 0x75afd992, 0x0);
    check(DX_HASH_LEGACY, "hello", 0x32252546, 0x0);
    check(DX_HASH_LEGACY, "lost+found", 0x5e2aba24, 0x0);
    check(DX_HASH_LEGACY, ".bashrc", 0x8df6eb22, 0x0);
    check(
        DX_HASH_LEGACY,
        "averylongfilenamethatspansmultiplehashblocks",
        0x8aa16e3c,
        0x0,
    );
}

#[test]
fn half_md4_matches_e2fsprogs() {
    check(DX_HASH_HALF_MD4, "a", 0xd5fa7d7a, 0xacb48187);
    check(DX_HASH_HALF_MD4, "ab", 0xee5ef564, 0x4bcbe18e);
    check(DX_HASH_HALF_MD4, "abc", 0xd196a868, 0xc420eb28);
    check(DX_HASH_HALF_MD4, "hello", 0x1746da32, 0x420013b5);
    check(DX_HASH_HALF_MD4, "lost+found", 0x591de422, 0x6ffc56e0);
    check(DX_HASH_HALF_MD4, ".bashrc", 0x21c5aa16, 0xded5b68d);
}

/// Longer than one 32-byte block, so the transform runs more than once
#[test]
fn half_md4_spans_multiple_blocks() {
    check(
        DX_HASH_HALF_MD4,
        "averylongfilenamethatspansmultiplehashblocks",
        0x11c272d4,
        0x601063a8,
    );
}

#[test]
fn tea_matches_e2fsprogs() {
    check(DX_HASH_TEA, "a", 0x6d0ea4c0, 0xc18922df);
    check(DX_HASH_TEA, "ab", 0xd27d8658, 0xb8458267);
    check(DX_HASH_TEA, "abc", 0xb1435ec4, 0x3f7eaa0e);
    check(DX_HASH_TEA, "hello", 0x6f5bb1a8, 0x231917c2);
    check(DX_HASH_TEA, "lost+found", 0x2dbf9e80, 0xbfebee4f);
    check(DX_HASH_TEA, ".bashrc", 0x83615c9c, 0xedbb7c26);
}

/// TEA consumes 16 bytes a round, so this name takes three
#[test]
fn tea_spans_multiple_blocks() {
    check(
        DX_HASH_TEA,
        "averylongfilenamethatspansmultiplehashblocks",
        0xb76422dc,
        0x8152c721,
    );
}

/// The low bit is reserved as a collision marker, not part of the hash. Linux
/// relies on this to walk an index in order
#[test]
fn the_low_bit_is_always_clear() {
    for version in [DX_HASH_LEGACY, DX_HASH_HALF_MD4, DX_HASH_TEA] {
        for name in ["a", "hello", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz", "x"] {
            let got = ext2_dx_hash_full(name.as_bytes(), version, &NO_SEED);
            assert_eq!(got.hash & 1, 0, "{name:?} under version {version}");
        }
    }
}

/// A volume's seed must change the result
#[test]
fn the_seed_changes_the_hash() {
    let seeded = [0x1111_1111u32, 0x2222_2222, 0x3333_3333, 0x4444_4444];
    let a = ext2_dx_hash_full(b"hello", DX_HASH_HALF_MD4, &NO_SEED);
    let b = ext2_dx_hash_full(b"hello", DX_HASH_HALF_MD4, &seeded);
    assert_ne!(a, b);
}

/// An index with an unimplemented hash version is not guessed at. Zero sends
/// each lookup down the linear fallback, slow and correct
#[test]
fn an_unknown_hash_version_yields_nothing() {
    let got = ext2_dx_hash_full(b"hello", 99, &NO_SEED);
    assert_eq!(got.hash, 0);
    assert_eq!(got.minor, 0);
}
