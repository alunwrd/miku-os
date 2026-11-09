// `MemDisk`'s model, used by crash tests in `lib/fs/extfs` and `lib/fs/btrfs`:
//
// write_sectors           volatile, lost by a crash
// write_sectors_sync      write-through, on the medium at return
// write_sectors_barrier   write-through and flushed (FUA)
//   flush                   the cache becomes the medium
// crash()                 the cache is gone, the medium is not

use fshost::{BlockHost, HostError, MemDisk};

const SECTORS: usize = 64;

fn sector(fill: u8) -> Vec<u8> {
    vec![fill; 512]
}

fn read1(d: &mut MemDisk, lba: u64) -> u8 {
    let mut buf = sector(0);
    d.read_sectors(lba, 1, &mut buf).unwrap();
    // Each write fills a whole sector with one byte, checked over the whole
    // sector
    assert!(buf.iter().all(|&b| b == buf[0]), "sector {} is not uniform", lba);
    buf[0]
}

fn volatile_disk() -> MemDisk {
    let mut d = MemDisk::new(SECTORS);
    d.set_volatile(true);
    d
}

// the cache itself

/// A durable write after a lazy one to the same sector must remove the cached
/// copy, or a read returns stale data and a flush writes it back
#[test]
fn a_write_through_write_is_not_shadowed_by_an_older_cached_one() {
    let mut d = volatile_disk();
    d.write_sectors(10, 1, &sector(0xAA)).unwrap();
    d.write_sectors_sync(10, 1, &sector(0xBB)).unwrap();
    assert_eq!(read1(&mut d, 10), 0xBB, "the read returned the older cached write");

    d.flush().unwrap();
    assert_eq!(read1(&mut d, 10), 0xBB, "the flush replayed a stale cache entry");
}

/// Invalidation takes only the requested sectors of a straddling entry
#[test]
fn invalidating_the_cache_keeps_the_sectors_either_side() {
    let mut d = volatile_disk();
    // One lazy entry covering 10..14
    let mut run = Vec::new();
    for _ in 0..4 {
        run.extend_from_slice(&sector(0xAA));
    }
    d.write_sectors(10, 4, &run).unwrap();

    // A durable write into the middle of it
    d.write_sectors_sync(11, 2, &[sector(0xBB), sector(0xBB)].concat()).unwrap();

    for (lba, want) in [(10, 0xAA), (11, 0xBB), (12, 0xBB), (13, 0xAA)] {
        assert_eq!(read1(&mut d, lba), want, "sector {} before the flush", lba);
    }
    d.flush().unwrap();
    for (lba, want) in [(10, 0xAA), (11, 0xBB), (12, 0xBB), (13, 0xAA)] {
        assert_eq!(read1(&mut d, lba), want, "sector {} after the flush", lba);
    }
}

/// Reads see the write cache; the data is only lost by a crash
#[test]
fn a_read_sees_the_write_cache() {
    let mut d = volatile_disk();
    d.write_sectors(5, 1, &sector(0x77)).unwrap();
    assert_eq!(read1(&mut d, 5), 0x77);
    assert_eq!(d.image()[5 * 512], 0x00, "it reached the medium before a flush");
}

/// Two lazy writes to one sector: the later wins, before and after the flush
#[test]
fn the_later_of_two_cached_writes_wins() {
    let mut d = volatile_disk();
    d.write_sectors(5, 1, &sector(0x11)).unwrap();
    d.write_sectors(5, 1, &sector(0x22)).unwrap();
    assert_eq!(read1(&mut d, 5), 0x22);
    d.flush().unwrap();
    assert_eq!(read1(&mut d, 5), 0x22);
}

// the crash

/// Which writes survive a power cut, per method. The flush commits everything
/// cached then, so the sector to lose is written after it
#[test]
fn a_crash_keeps_exactly_what_went_to_the_medium() {
    let mut d = volatile_disk();
    d.write_sectors(1, 1, &sector(0x11)).unwrap();          // volatile...
    d.write_sectors_sync(2, 1, &sector(0x22)).unwrap();     // write-through
    d.write_sectors_barrier(3, 1, &sector(0x33)).unwrap();  // FUA
    d.flush().unwrap();                                     // now flushed
    d.write_sectors(4, 1, &sector(0x44)).unwrap();          // volatile, after
    d.write_sectors_sync(5, 1, &sector(0x55)).unwrap();     // durable, after

    d.crash();

    assert_eq!(read1(&mut d, 1), 0x11, "a flushed write was lost");
    assert_eq!(read1(&mut d, 2), 0x22, "a write-through write was lost");
    assert_eq!(read1(&mut d, 3), 0x33, "a barrier write was lost");
    assert_eq!(read1(&mut d, 4), 0x00, "an unflushed write survived");
    assert_eq!(read1(&mut d, 5), 0x55, "a write-through write was lost");
}

/// A barrier write is durable with no flush, as btrfs's `write_all_supers`
/// relies on with `REQ_FUA`
#[test]
fn a_barrier_write_needs_no_flush_to_survive() {
    let mut d = volatile_disk();
    d.write_sectors(1, 1, &sector(0x11)).unwrap();          // must be lost
    d.write_sectors_barrier(2, 1, &sector(0x22)).unwrap();  // must not be
    d.write_sectors_sync(3, 1, &sector(0x33)).unwrap();     // nor this
    d.crash();

    assert_eq!(read1(&mut d, 1), 0x00, "an unflushed write survived");
    assert_eq!(read1(&mut d, 2), 0x22, "a barrier write did not reach the medium");
    assert_eq!(read1(&mut d, 3), 0x33, "a write-through write did not reach the medium");
}

/// A crash with nothing cached changes nothing
#[test]
fn a_crash_with_an_empty_cache_loses_nothing() {
    let mut d = volatile_disk();
    d.write_sectors(7, 1, &sector(0x99)).unwrap();
    d.flush().unwrap();
    d.crash();
    assert_eq!(read1(&mut d, 7), 0x99);
}

/// Without volatile modeling, an ordinary write is on the medium
#[test]
fn a_disk_that_is_not_volatile_keeps_an_ordinary_write() {
    let mut d = MemDisk::new(SECTORS);
    d.write_sectors(9, 1, &sector(0x5A)).unwrap();
    d.crash();
    assert_eq!(read1(&mut d, 9), 0x5A);
}

/// Turning modeling off commits the cache
#[test]
fn turning_volatile_off_commits_the_cache() {
    let mut d = volatile_disk();
    d.write_sectors(9, 1, &sector(0x5A)).unwrap();
    d.set_volatile(false);
    d.crash();
    assert_eq!(read1(&mut d, 9), 0x5A);
}

// the cut point

/// `crash_at` stops at a named write. Later writes do nothing and report no
/// error
#[test]
fn the_power_goes_out_at_the_write_it_was_told_to() {
    let mut d = MemDisk::new(SECTORS);
    d.crash_at(2);

    d.write_sectors_sync(1, 1, &sector(0x11)).unwrap();
    d.write_sectors_sync(2, 1, &sector(0x22)).unwrap();
    // Past the cut: accepted, with no effect
    d.write_sectors_sync(3, 1, &sector(0x33)).unwrap();
    d.write_sectors_sync(4, 1, &sector(0x44)).unwrap();

    assert!(d.did_crash(), "the sweep never reached its cut point");
    assert_eq!(read1(&mut d, 1), 0x11);
    assert_eq!(read1(&mut d, 2), 0x22);
    assert_eq!(read1(&mut d, 3), 0x00, "a write past the cut reached the medium");
    assert_eq!(read1(&mut d, 4), 0x00, "a write past the cut reached the medium");
}

/// The cut also drops the cache
#[test]
fn the_cut_loses_the_cache_too() {
    let mut d = volatile_disk();
    d.write_sectors(1, 1, &sector(0x11)).unwrap();
    d.crash_at(0);
    d.write_sectors(2, 1, &sector(0x22)).unwrap();

    assert!(d.did_crash());
    assert_eq!(read1(&mut d, 1), 0x00, "the cache survived the cut");
    assert_eq!(read1(&mut d, 2), 0x00);
}

/// `did_crash` separates a cut from a clean shutdown
#[test]
fn a_cut_point_that_is_never_reached_says_so() {
    let mut d = MemDisk::new(SECTORS);
    d.crash_at(100);
    d.write_sectors_sync(1, 1, &sector(0x11)).unwrap();
    assert!(!d.did_crash());
    assert_eq!(read1(&mut d, 1), 0x11);
}

/// After power returns the medium stays and the counter restarts
#[test]
fn power_on_takes_writes_again() {
    let mut d = MemDisk::new(SECTORS);
    d.crash_at(0);
    d.write_sectors_sync(1, 1, &sector(0x11)).unwrap();
    assert_eq!(read1(&mut d, 1), 0x00);

    d.power_on();
    d.write_sectors_sync(1, 1, &sector(0x11)).unwrap();
    assert_eq!(read1(&mut d, 1), 0x11);
    assert!(!d.did_crash());
}

// the edges

/// A discard drops the data and its cached copy
#[test]
fn a_discard_drops_the_medium_and_the_cache() {
    let mut d = volatile_disk();
    d.write_sectors_sync(12, 1, &sector(0xEE)).unwrap();
    d.write_sectors(12, 1, &sector(0xDD)).unwrap();
    d.discard(12, 1).unwrap();

    assert_eq!(read1(&mut d, 12), 0x00, "a discarded sector still reads");
    d.flush().unwrap();
    assert_eq!(read1(&mut d, 12), 0x00, "a flush wrote back a discarded sector");
}

/// `fail_after` reports errors, unlike a power cut
#[test]
fn a_failing_device_reports_the_write() {
    let mut d = MemDisk::new(SECTORS);
    d.fail_after = Some(1);
    assert!(d.write_sectors(1, 1, &sector(0x11)).is_ok());
    assert!(matches!(d.write_sectors(2, 1, &sector(0x22)), Err(HostError::Io)));
    assert_eq!(read1(&mut d, 2), 0x00, "a write that reported failure still landed");
}

/// Past the end of the medium is an error, not a panic or partial write
#[test]
fn reading_or_writing_past_the_end_is_refused() {
    let mut d = MemDisk::new(SECTORS);
    let last = SECTORS as u64;
    let mut buf = sector(0);

    assert!(matches!(d.read_sectors(last, 1, &mut buf), Err(HostError::Io)));
    assert!(matches!(d.write_sectors(last, 1, &sector(0x11)), Err(HostError::Io)));
    assert!(matches!(d.write_sectors_sync(last - 1, 2, &[sector(1), sector(2)].concat()), Err(HostError::Io)));
    assert!(matches!(d.read_sectors(u64::MAX, 1, &mut buf), Err(HostError::Io)));
}

/// A short buffer is refused
#[test]
fn a_buffer_that_is_too_small_is_refused() {
    let mut d = MemDisk::new(SECTORS);
    let mut small = vec![0u8; 256];
    assert!(matches!(d.read_sectors(0, 1, &mut small), Err(HostError::Io)));
    assert!(matches!(d.write_sectors(0, 1, &small), Err(HostError::Io)));
}

/// `image` is the medium, `image_flushed` the medium after a clean shutdown
#[test]
fn the_image_and_the_flushed_image_are_different_questions() {
    let mut d = volatile_disk();
    d.write_sectors(3, 1, &sector(0xC3)).unwrap();
    assert_eq!(d.image()[3 * 512], 0x00);
    assert_eq!(d.image_flushed()[3 * 512], 0xC3);
    // It really flushed
    d.crash();
    assert_eq!(read1(&mut d, 3), 0xC3);
}

/// An external image opens as a disk
#[test]
fn an_image_from_outside_opens_as_a_disk() {
    let mut raw = vec![0u8; SECTORS * 512];
    raw[512..1024].fill(0x42);
    let mut d = MemDisk::from_image(raw);
    assert_eq!(d.sectors(), SECTORS as u64);
    assert_eq!(read1(&mut d, 1), 0x42);
}

/// Fixed non-zero clock, since a checker reads a small `i_dtime` on an unlinked
/// inode as an orphan-list pointer
#[test]
fn the_clock_answers() {
    let d = MemDisk::new(SECTORS);
    assert!(d.wall_clock().is_some_and(|t| t > 1_600_000_000));
}
