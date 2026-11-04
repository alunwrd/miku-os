// Checksum tests against published vectors and reference definitions. ext4 uses
// `crc32c_raw`; btrfs uses the standard form for blocks and the raw form for
// directory hashes

use fshost::crc32c::{crc16, crc32c, crc32c_raw, crc32c_raw_table};

/// Castagnoli check value: CRC-32C of "123456789", seed 0, is 0xE3069283
#[test]
fn the_published_check_value_comes_out() {
    assert_eq!(crc32c(0, b"123456789"), 0xE306_9283);
}

/// Empty input returns the seed in the raw form, so chaining works
#[test]
fn nothing_leaves_the_seed_alone() {
    assert_eq!(crc32c_raw(0, b""), 0);
    assert_eq!(crc32c_raw(0xDEAD_BEEF, b""), 0xDEAD_BEEF);
    assert_eq!(crc32c_raw_table(0xDEAD_BEEF, b""), 0xDEAD_BEEF);
}

/// Splitting the input anywhere gives the same result as one call
#[test]
fn the_raw_form_chains_at_any_split() {
    let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
    let whole = crc32c_raw(0x1234_5678, &data);
    for split in [0, 1, 7, 8, 9, 63, 64, 65, 499, 999, 1000] {
        let (a, b) = data.split_at(split);
        let piecewise = crc32c_raw(crc32c_raw(0x1234_5678, a), b);
        assert_eq!(whole, piecewise, "split at {}", split);
    }
}

/// The two forms disagree for the same seed
#[test]
fn the_standard_and_raw_forms_disagree() {
    // Except on empty input, where the two inversions cancel
    for input in [&b"a"[..], b"123456789", b"hello.txt", b"docs"] {
        assert_ne!(
            crc32c(0, input),
            crc32c_raw(0, input),
            "the two forms agreed on {:?}, so nothing distinguishes them",
            core::str::from_utf8(input).unwrap_or("?")
        );
    }
    // They differ by inversion at each end
    assert_eq!(crc32c(0, b"123456789"), !crc32c_raw(!0, b"123456789"));
}

/// btrfs keys a directory entry by the raw accumulation seeded with ~1
/// (`crc32c((u32)~1, name)`). `lib/btrfs` tests its hash against real images
#[test]
fn a_seed_changes_the_answer() {
    let name = b"hello.txt";
    let seeds = [0u32, 1, !0, !1];
    for (i, &a) in seeds.iter().enumerate() {
        for &b in &seeds[i + 1..] {
            assert_ne!(crc32c_raw(a, name), crc32c_raw(b, name), "seeds {:#x} and {:#x}", a, b);
        }
    }
}

/// Hardware and table paths must match
#[test]
fn the_instruction_and_the_table_agree() {
    let data: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
    // Lengths through the eight-byte boundary and past it
    for len in (0..80).chain([255, 256, 257, 1023, 1024, 4095, 4096]) {
        for &seed in &[0u32, 0xFFFF_FFFF, 0x1234_5678] {
            assert_eq!(
                crc32c_raw(seed, &data[..len]),
                crc32c_raw_table(seed, &data[..len]),
                "len {} seed {:#x}",
                len,
                seed
            );
        }
    }
}

/// Every single-byte change must change the value
#[test]
fn every_byte_of_a_block_is_covered() {
    let base = vec![0xA5u8; 4096];
    let want = crc32c_raw(0, &base);
    for i in [0usize, 1, 7, 8, 9, 2048, 4087, 4094, 4095] {
        let mut altered = base.clone();
        altered[i] ^= 0x01;
        assert_ne!(crc32c_raw(0, &altered), want, "a change at byte {} went unnoticed", i);
    }
}

/// Order matters
#[test]
fn transposing_two_bytes_is_noticed() {
    assert_ne!(crc32c_raw(0, b"ab"), crc32c_raw(0, b"ba"));
}

/// CRC-16/ARC check value over "123456789" is 0xBB3D
#[test]
fn the_legacy_crc16_matches_its_published_vector() {
    assert_eq!(crc16(0, b"123456789"), 0xBB3D);
    assert_eq!(crc16(0, b""), 0);
}
