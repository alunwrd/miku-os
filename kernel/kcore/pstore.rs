// Persistent RAM store: a log that survives a reboot without touching a disk
//
// Some failures take the console down before they finish. In the GB206 GSP-RM
// bring-up, GSP-RM remaps BAR1 under the firmware framebuffer partway through,
// so later lines go nowhere; the kernel's own buffers do not help when the
// machine needs a reset.
//
// Linux's ramoops/pstore idea, cut down to one region: a small buffer at a
// fixed physical address, out of the frame allocator, written as things happen.
// A warm reset does not retrain DRAM, so the bytes survive; init() takes them
// before anything can overwrite them and hands them to 'pstore' in the shell.
//
// Nothing is guaranteed: a firmware scrubbing memory on reset, or a power
// cycle, loses the contents. A miss reads as "no log from the previous boot",
// not as wrong data: the header carries a magic and a checksum over the body.
//
// Each append is followed by clflush over the bytes it touched, so a reset
// before any natural writeback finds the log.

use alloc::string::String;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::x86_64::grub;

/// 'MIKUPSTR' little-endian. Bumped whenever the layout below changes, so an
/// older boot's region is rejected
const MAGIC: u64 = 0x5254_5350_554B_494E;

/// 512 KiB: about 6000 log lines. The GPU bring-up path alone uses most of 64
/// KiB, and a log stopping early reads like a kernel that died early
const REGION_SIZE: u64 = 512 * 1024;

/// Where the region goes if nothing is in the way: above the kernel image and
/// any GRUB module, below the 1 GiB mark so a small-RAM machine has it
const PREFERRED_BASE: u64 = 256 * 1024 * 1024;

/// Step used when the preferred base collides with a module or is not backed
/// by usable RAM
const PROBE_STEP: u64 = 16 * 1024 * 1024;
const PROBE_TRIES: u32 = 64;

#[repr(C)]
struct Header {
    magic: u64,
    /// Increments once per boot finding a valid region, telling "the previous
    /// boot wrote nothing" from "this is stale"
    seq: u32,
    /// Bytes of body in use
    len: u32,
    /// Sum of the body bytes in use, so a region partly written by a reset
    /// mid-append is rejected
    sum: u32,
    _pad: u32,
}

const HEADER_SIZE: u64 = 24;

const BODY_SIZE: usize = (REGION_SIZE - HEADER_SIZE) as usize;

/// Physical base of the live region, 0 while the store is off
static BASE: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);
/// Set once the body fills, so the "truncated" marker is written exactly once
static FULL: AtomicBool = AtomicBool::new(false);

/// Text recovered from the previous boot, taken before anything overwrites it
static PREVIOUS: spin::Mutex<Option<String>> = spin::Mutex::new(None);

#[inline]
fn hdr(base: u64) -> *mut Header {
    grub::phys_to_virt(base) as *mut Header
}

#[inline]
fn body(base: u64) -> *mut u8 {
    (grub::phys_to_virt(base) + HEADER_SIZE) as *mut u8
}

fn checksum(bytes: &[u8]) -> u32 {
    checksum_more(0x811c_9dc5, bytes)
}

/// The same sum carried on over more bytes. A fold: the sum of a body grown by
/// an append is the old sum carried over the new bytes, so an append costs its
/// own length
fn checksum_more(mut s: u32, bytes: &[u8]) -> u32 {
    // Trivial: this guards against a torn write, not tampering
    for &b in bytes {
        s = s.rotate_left(5) ^ b as u32;
    }
    s
}

/// One writer at a time. Appends come from every CPU once the kernel log is
/// mirrored here (`klogpstore`), interrupt handlers included, so it is taken
/// with interrupts off and given up after a bounded wait: a CPU halted by a
/// panic while holding it must not take the panic's own line down
static WRITER: spin::Mutex<()> = spin::Mutex::new(());
const LOCK_SPINS: u32 = 2_000_000;

/// Push the bytes covering [ptr, ptr+len) out of the cache hierarchy, so a
/// reset that follows immediately still finds them in DRAM
fn flush(ptr: *const u8, len: usize) {
    crate::mm::dma::flush_lines(ptr, len);
}

/// Pick a physical base clear of the kernel image and every GRUB module.
/// Returns None if no candidate in the probe range is usable RAM
fn choose_base(kernel_end: u64) -> Option<u64> {
    let floor = kernel_end.max(grub::modules_end()).max(PREFERRED_BASE);
    let mut base = (floor + PROBE_STEP - 1) & !(PROBE_STEP - 1);
    for _ in 0..PROBE_TRIES {
        if grub::is_usable_ram(base, REGION_SIZE) {
            return Some(base);
        }
        base += PROBE_STEP;
    }
    None
}

/// Claim the region, recover what the previous boot left in it, and start a
/// fresh log. Runs after the frame allocator has its regions and before
/// anything can hand out the frames: main.rs calls it next to the other
/// reserve_region calls
pub fn init(kernel_end: u64) {
    let Some(base) = choose_base(kernel_end) else {
        crate::serial_println!("[pstore] no usable RAM for the region; persistent log off");
        return;
    };
    crate::mm::pmm::reserve_region(base, REGION_SIZE);

    // Read the previous boot out before touching anything
    let (magic, seq, len, sum) = unsafe {
        let h = hdr(base);
        (
            core::ptr::addr_of!((*h).magic).read_volatile(),
            core::ptr::addr_of!((*h).seq).read_volatile(),
            core::ptr::addr_of!((*h).len).read_volatile(),
            core::ptr::addr_of!((*h).sum).read_volatile(),
        )
    };

    let mut next_seq = 1u32;
    if magic == MAGIC && len as usize <= BODY_SIZE {
        next_seq = seq.wrapping_add(1);
        if len > 0 {
            let slice = unsafe { core::slice::from_raw_parts(body(base), len as usize) };
            if checksum(slice) == sum {
                // Lossy: the region holds what write_str put there, ASCII, but
                // a half-written multi-byte tail must not make recovery panic
                *PREVIOUS.lock() = Some(String::from_utf8_lossy(slice).into_owned());
                crate::serial_println!(
                    "[pstore] recovered {} bytes from boot #{} at {:#x}", len, seq, base
                );
            } else {
                crate::serial_println!(
                    "[pstore] region at {:#x} failed its checksum (torn write); discarded", base
                );
            }
        }
    } else {
        crate::serial_println!("[pstore] region at {:#x} is cold; starting a new log", base);
    }

    unsafe {
        let h = hdr(base);
        core::ptr::addr_of_mut!((*h).magic).write_volatile(MAGIC);
        core::ptr::addr_of_mut!((*h).seq).write_volatile(next_seq);
        core::ptr::addr_of_mut!((*h).len).write_volatile(0);
        core::ptr::addr_of_mut!((*h).sum).write_volatile(checksum(&[]));
        core::ptr::addr_of_mut!((*h)._pad).write_volatile(0);
    }
    flush(hdr(base) as *const u8, HEADER_SIZE as usize);

    BASE.store(base, Ordering::Release);
    ARMED.store(true, Ordering::Release);
}

/// True once the region is claimed and writable
pub fn available() -> bool {
    ARMED.load(Ordering::Acquire)
}

/// Physical base of the region, for diagnostics
pub fn base_phys() -> u64 {
    BASE.load(Ordering::Acquire)
}

/// Everything the previous boot logged, if it survived
pub fn previous() -> Option<String> {
    PREVIOUS.lock().clone()
}

/// Forget the recovered text. The region itself is already reset by init()
pub fn clear_previous() {
    *PREVIOUS.lock() = None;
}

/// Append a line. Does nothing if the store is off. Once the body is full the
/// log stops growing, not wrapping: the interesting part of a boot that dies is
/// its beginning, and a ring would cost a second flush pass
pub fn write_line(s: &str) {
    if !ARMED.load(Ordering::Acquire) {
        return;
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut spins = 0u32;
        let _g = loop {
            if let Some(g) = WRITER.try_lock() {
                break g;
            }
            spins += 1;
            if spins >= LOCK_SPINS {
                return;
            }
            core::hint::spin_loop();
        };
        write_line_locked(s);
    });
}

fn write_line_locked(s: &str) {
    let base = BASE.load(Ordering::Acquire);
    let h = hdr(base);
    let used = unsafe { core::ptr::addr_of!((*h).len).read_volatile() } as usize;
    let bytes = s.as_bytes();
    let room = BODY_SIZE.saturating_sub(used);
    // A silently truncated log reads like a kernel that stopped, so the last
    // few bytes say which happened
    const FULL_MARK: &str = "*** pstore full - log truncated here ***";
    if room < bytes.len() + 1 {
        if FULL.swap(true, Ordering::AcqRel) || room < FULL_MARK.len() + 1 {
            return;
        }
        write_raw(base, h, used, FULL_MARK.as_bytes());
        return;
    }
    let n = bytes.len();
    write_raw(base, h, used, &bytes[..n]);
}

/// Append `bytes` plus a newline at `used`, then republish the header. The
/// caller has checked that it fits
fn write_raw(base: u64, h: *mut Header, used: usize, bytes: &[u8]) {
    let n = bytes.len();
    unsafe {
        let dst = body(base).add(used);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, n);
        *dst.add(n) = b'\n';
    }
    let new_len = used + n + 1;
    let old_sum = unsafe { core::ptr::addr_of!((*h).sum).read_volatile() };
    let grown = unsafe { core::slice::from_raw_parts(body(base).add(used), n + 1) };
    let sum = checksum_more(old_sum, grown);
    // Body first, then the header: a reset between the two leaves a header
    // describing less than what is there, never more
    flush(unsafe { body(base).add(used) }, n + 1);
    unsafe {
        core::ptr::addr_of_mut!((*h).len).write_volatile(new_len as u32);
        core::ptr::addr_of_mut!((*h).sum).write_volatile(sum);
    }
    flush(h as *const u8, HEADER_SIZE as usize);
}

/// Empty the region for this boot and start again from its first byte, for a
/// test needing all of it (`pstore mirror`)
pub fn restart() {
    if !ARMED.load(Ordering::Acquire) {
        return;
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        let _g = WRITER.lock();
        let h = hdr(BASE.load(Ordering::Acquire));
        unsafe {
            core::ptr::addr_of_mut!((*h).len).write_volatile(0);
            core::ptr::addr_of_mut!((*h).sum).write_volatile(checksum(&[]));
        }
        flush(h as *const u8, HEADER_SIZE as usize);
        FULL.store(false, Ordering::Release);
    });
}

/// Bytes used and total, for a boot to report how close it is to going mute
pub fn usage() -> (usize, usize) {
    if !ARMED.load(Ordering::Acquire) {
        return (0, 0);
    }
    let h = hdr(BASE.load(Ordering::Acquire));
    let used = unsafe { core::ptr::addr_of!((*h).len).read_volatile() } as usize;
    (used, BODY_SIZE)
}

/// Append a formatted line: 'pstore_println!("gsp: {} events", n)'
#[macro_export]
macro_rules! pstore_println {
    ($($arg:tt)*) => {
        if $crate::kcore::pstore::available() {
            $crate::kcore::pstore::write_line(&alloc::format!($($arg)*));
        }
    };
}
