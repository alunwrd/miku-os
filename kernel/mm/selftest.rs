// `mm test`, or `run=mm_test` on the GRUB line
//
// The heap, device memory and the frame allocator, on the machine
// (`dev/kernel-core.md`). What only a boot can say: that device memory is what
// a device needs (one physical run, aligned, below its limit), that the heap
// holds up on every CPU at once and gives memory back, and that the frame
// counts and the CPU caches behave as the copy-on-write code assumes.

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::kcore::selftest::{Report, Runner};
use crate::mm::pmm;

pub fn run() -> Report {
    let mut t = Runner::new();
    check_device_memory(&mut t);
    check_alignment(&mut t);
    check_churn(&mut t);
    check_cost(&mut t);
    check_fault_parts(&mut t);
    check_gives_back(&mut t);
    check_mapped_area(&mut t);
    check_shrinker(&mut t);
    check_slabdebug(&mut t);
    check_cow_counts(&mut t);
    check_double_free(&mut t);
    check_all_memory(&mut t);
    t.report()
}

/// A driver's memory for its device (`mm::dma::DmaBox`): ATA's channel, 64 KiB
/// aligned below 4 GiB, and NVMe's queue memory anywhere - each one physical
/// run, with `phys_of` answering for a field inside it
fn check_device_memory(t: &mut Runner) {
    use crate::mm::dma::{DmaBox, DMA32};
    #[repr(C, align(65536))]
    struct Channel {
        bounce: [u8; 65536],
        prdt: [u8; 8],
    }
    // SAFETY: bytes only
    unsafe impl crate::mm::dma::Zeroed for Channel {}
    let ata = DmaBox::<Channel>::new_below(DMA32);
    let nvme = DmaBox::<[u8; 76 << 10]>::new();
    let (Ok(ata), Ok(nvme)) = (ata, nvme) else {
        t.check("dma/device-memory", false, String::from("no frames for it"));
        return;
    };
    let hhdm = crate::arch::x86_64::grub::hhdm();
    // One run: every page the CPU sees is the frame the device is given
    let one_run = |virt: u64, phys: u64, len: u64| (0..len).step_by(4096).all(|at| {
        crate::arch::x86_64::grub::any_virt_to_phys(virt + at) == phys + at
    });
    let ata_ok = ata.phys() % 65536 == 0
        && ata.phys() + core::mem::size_of::<Channel>() as u64 <= DMA32
        && ata.phys_of(ata.prdt.as_ptr()) == ata.phys() + 65536
        && one_run(ata.bounce.as_ptr() as u64, ata.phys(), 65536)
        && ata.bounce.iter().all(|&b| b == 0);
    let nvme_ok = nvme.phys_of(nvme[4096..].as_ptr()) == nvme.phys() + 4096
        && one_run(nvme.as_ptr() as u64, nvme.phys(), 76 << 10)
        && nvme.as_ptr() as u64 == nvme.phys() + hhdm;
    t.check("dma/device-memory", ata_ok && nvme_ok,
            format!("ATA's channel at {:#x} ({}), NVMe's 76 KiB at {:#x} ({})",
                    ata.phys(), ata_ok, nvme.phys(), nvme_ok));
}

fn check_alignment(t: &mut Runner) {
    use core::alloc::Layout;
    let mut ok = true;
    let mut detail = String::new();
    for (size, align) in [(64usize, 64usize), (4096, 4096), (8192, 4096), (300 << 10, 4096), (100, 2048)] {
        let layout = Layout::from_size_align(size, align).unwrap();
        let p = unsafe { alloc::alloc::alloc(layout) };
        let good = !p.is_null() && p as usize % align == 0;
        if good {
            unsafe {
                core::ptr::write_bytes(p, 0xc3, size);
                alloc::alloc::dealloc(p, layout);
            }
        } else {
            ok = false;
            detail = format!("{size} bytes at {align} came back at {p:?}");
        }
    }
    t.check("heap/aligned-blocks", ok, if ok { String::from("64/64, 4K/4K, 8K/4K, 300K/4K, 100/2K") } else { detail });
}

fn check_churn(t: &mut Runner) {
    // Mixed sizes kept and replaced, as a compositor's frame does with its
    // lists, every block filled and checked before it goes
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
    let mut live: Vec<(Vec<u8>, u8)> = Vec::new();
    let mut ok = true;
    let watch = crate::kcore::time::Stopwatch::start();
    for i in 0..20_000u32 {
        let r = next();
        if live.len() < 2000 || r % 3 != 0 {
            let len = match r % 100 { 0..=84 => 1 + (r >> 8) as usize % 700, 85..=98 => 700 + (r >> 8) as usize % 30_000, _ => 300 << 10 };
            let fill = (r >> 40) as u8;
            live.push((alloc::vec![fill; len], fill));
        } else {
            let k = (r >> 16) as usize % live.len();
            let (v, fill) = live.swap_remove(k);
            if !v.iter().all(|&b| b == fill) {
                ok = false;
            }
        }
        if i % 4000 == 0 && !live.iter().all(|(v, f)| v.first().map_or(true, |&b| b == *f)) {
            ok = false;
        }
    }
    let took = watch.elapsed_us();
    drop(live);
    t.check("heap/churn", ok,
            format!("20000 operations in {} us, heap {} KiB used of {} KiB held",
                    took, crate::mm::heap::used() / 1024, crate::mm::heap::total() / 1024));
}

/// What a demand-zero fault's parts cost (`mmap::handle_anon_fault`), for
/// stdbench's page fault case: a frame, zeroing it, a mapping in a user
/// address space, and the unmap that frees it. Per page, over `N` pages
fn check_fault_parts(t: &mut Runner) {
    use x86_64::structures::paging::PageTableFlags as F;
    const N: usize = 4096;
    const BASE: u64 = 0x0000_6000_0000_0000;
    let Some(space) = crate::mm::vmm::AddressSpace::new_user() else {
        t.skip("fault/parts", String::from("no frame for an address space"));
        return;
    };
    let now = crate::kcore::time::now_ns;
    let hhdm = crate::arch::x86_64::grub::hhdm();
    let mut frames = Vec::with_capacity(N);
    let t0 = now();
    for _ in 0..N {
        match pmm::alloc_frame() {
            Some(f) => frames.push(f),
            None => break,
        }
    }
    let t1 = now();
    for &f in &frames {
        unsafe { core::ptr::write_bytes((f + hhdm) as *mut u8, 0, 4096) };
    }
    let t2 = now();
    let flags = F::PRESENT | F::WRITABLE | F::USER_ACCESSIBLE | F::NO_EXECUTE;
    let mut mapped = 0;
    for (i, &f) in frames.iter().enumerate() {
        if matches!(space.map_page_if_absent(BASE + i as u64 * 4096, f, flags), crate::mm::vmm::MapOutcome::Mapped) {
            mapped += 1;
        }
    }
    let t3 = now();
    for i in 0..frames.len() {
        space.unmap_page(BASE + i as u64 * 4096);
    }
    let t4 = now();
    drop(space);
    let n = frames.len().max(1) as u64;
    t.check("fault/parts", mapped == frames.len() && frames.len() == N,
        format!("per page: frame {} ns, zero {} ns, map {} ns, unmap+free {} ns ({} pages)",
            (t1 - t0) / n, (t2 - t1) / n, (t3 - t2) / n, (t4 - t3) / n, frames.len()));
}

/// Pairs each `cost_worker` runs, and what it found
const COST_PAIRS: u64 = 200_000;
static COST_GO: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static COST_DONE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static COST_NS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Objects that came back with somebody else's bytes in them
static COST_BAD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// `COST_PAIRS` allocations and frees, a ring of 64 kept alive across sizes
/// a kernel asks for, and the nanoseconds a pair took. Each object is marked
/// at both ends and checked before it goes, so CPUs that ever shared one
/// object would show here
fn cost_run() -> u64 {
    use core::alloc::Layout;
    const SIZES: [usize; 8] = [16, 32, 64, 128, 256, 512, 1024, 3000];
    let mut ring: [(*mut u8, usize, u8); 64] = [(core::ptr::null_mut(), 0, 0); 64];
    // This CPU's marks differ from every other's
    let salt = (crate::arch::x86_64::percpu::current_index() as u8).wrapping_mul(37);
    let t0 = crate::kcore::time::now_ns();
    for i in 0..COST_PAIRS as usize {
        let slot = i % 64;
        let (old, old_size, mark) = ring[slot];
        if !old.is_null() {
            let ok = unsafe { old.read_volatile() == mark && old.add(old_size - 1).read_volatile() == !mark };
            if !ok {
                COST_BAD.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
            unsafe { alloc::alloc::dealloc(old, Layout::from_size_align_unchecked(old_size, 8)) };
        }
        let size = SIZES[(i * 7 + i / 64) % SIZES.len()];
        let p = unsafe { alloc::alloc::alloc(Layout::from_size_align_unchecked(size, 8)) };
        let mark = (i as u8) ^ salt;
        if !p.is_null() {
            unsafe {
                p.write_volatile(mark);
                p.add(size - 1).write_volatile(!mark);
            }
        }
        ring[slot] = (p, size, mark);
    }
    let took = crate::kcore::time::now_ns() - t0;
    for (p, size, _) in ring {
        if !p.is_null() {
            unsafe { alloc::alloc::dealloc(p, Layout::from_size_align_unchecked(size, 8)) };
        }
    }
    took / COST_PAIRS
}

fn cost_worker() -> ! {
    while !COST_GO.load(core::sync::atomic::Ordering::Acquire) {
        core::hint::spin_loop();
    }
    let ns = cost_run();
    COST_NS.fetch_max(ns, core::sync::atomic::Ordering::AcqRel);
    COST_DONE.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
    loop {
        crate::sched::sleep_ns(10_000_000);
    }
}

/// What an allocation and a free cost: alone, and with every other CPU
/// allocating at once. A measurement: it passes when the run finished
fn check_cost(t: &mut Runner) {
    use core::sync::atomic::Ordering;
    COST_BAD.store(0, Ordering::Release);
    let alone = cost_run();
    let cpus = crate::arch::x86_64::percpu::online_cpus().max(1);
    let others = cpus.saturating_sub(1).min(7);
    COST_GO.store(false, Ordering::Release);
    COST_DONE.store(0, Ordering::Release);
    COST_NS.store(0, Ordering::Release);
    let workers: Vec<u64> = (0..others).map(|_| crate::sched::spawn_named(cost_worker, "mm-test-cost", 10)).collect();
    // Until they are running somewhere
    crate::sched::sleep_ns(20_000_000);
    COST_GO.store(true, Ordering::Release);
    let mine = cost_run();
    let mut finished = false;
    for _ in 0..2000 {
        if COST_DONE.load(Ordering::Acquire) == others {
            finished = true;
            break;
        }
        crate::sched::sleep_ns(1_000_000);
    }
    for w in workers {
        crate::sched::kill(w);
    }
    let worst = COST_NS.load(Ordering::Acquire).max(mine);
    let bad = COST_BAD.load(Ordering::Acquire);
    t.check("heap/cost", finished && bad == 0,
            format!("{alone} ns a pair alone; {worst} ns at worst with {} CPUs at once; {bad} objects came back changed",
                    others + 1));
}

/// A burst of small objects, all freed: the frames go back to the frame
/// allocator. What may stay is two empty slabs, the depot's sixteen magazines
/// and the CPUs' own: well under 2 MiB
fn check_gives_back(t: &mut Runner) {
    // More than the churn above leaves held, so the burst takes new frames
    const OBJECTS: usize = 384 * 1024;
    let (before, _) = pmm::stats();
    let held_before = crate::mm::heap::total();
    let mut burst: Vec<alloc::boxed::Box<[u8; 256]>> = Vec::new();
    let _ = burst.try_reserve_exact(OBJECTS);
    for i in 0..OBJECTS {
        burst.push(alloc::boxed::Box::new([i as u8; 256]));
    }
    let (peak, _) = pmm::stats();
    drop(burst);
    let (after, _) = pmm::stats();
    let held_after = crate::mm::heap::total();
    let kept = after.saturating_sub(before) * 4096;
    t.check("heap/gives-back", peak.saturating_sub(before) * 4096 > 64 << 20 && kept < 2 << 20,
            format!("96 MiB of 256-byte objects: {} MiB of frames at the peak, {} MiB still held after; \
                     the heap held {} MiB before and {} MiB after",
                    peak.saturating_sub(before) * 4 / 1024, after.saturating_sub(before) * 4 / 1024,
                    held_before >> 20, held_after >> 20));
}

/// An object above 32 KiB is single frames in the mapped area: 64 MiB of
/// it, every page the one written, and unmapped when it goes. Its range is
/// handed out again only after a purge. Asked of the object itself: the
/// machine's frame and mapping counts move under other tasks meanwhile
fn check_mapped_area(t: &mut Runner) {
    if !crate::mm::vmap::ready() {
        t.check("vmap/big-object", false, String::from("the mapped area is not set up"));
        return;
    }
    const LEN: usize = 64 << 20;
    let mapped_before = crate::mm::vmap::mapped_pages();
    let mut big: Vec<u8> = Vec::new();
    if big.try_reserve_exact(LEN).is_err() {
        t.check("vmap/big-object", false, String::from("64 MiB refused"));
        return;
    }
    let at = big.as_ptr() as u64;
    let inside = crate::mm::vmap::contains(at);
    // Every page its own frame, each one written and read back
    for page in 0..LEN / 4096 {
        big.push(page as u8);
        big.extend(core::iter::repeat((page >> 8) as u8).take(4095));
    }
    let mut ok = big.len() == LEN;
    let mut runs = 0;
    let mut last = 0u64;
    for page in (0..LEN / 4096).step_by(37) {
        let v = at + (page * 4096) as u64;
        ok &= big[page * 4096] == page as u8 && big[page * 4096 + 4095] == (page >> 8) as u8;
        let phys = crate::mm::vmap::phys_of(v).unwrap_or(0);
        ok &= phys != 0;
        if phys != last + 37 * 4096 {
            runs += 1;
        }
        last = phys;
    }
    let pages = crate::mm::vmap::mapped_pages().saturating_sub(mapped_before);
    let guard = crate::mm::vmap::phys_of(at + LEN as u64).is_none();
    drop(big);
    let gone = (0..LEN / 4096).step_by(97).all(|p| crate::mm::vmap::phys_of(at + (p * 4096) as u64).is_none());
    t.check("vmap/big-object", inside && ok && pages >= LEN / 4096 && guard && gone,
            format!("64 MiB at {:#x} in the area {inside}, {pages} pages mapped, read back {ok}, \
                     {runs} breaks in 443 samples of its frames; guard page unmapped {guard}; unmapped after {gone}",
                    at));
    // The freed range waits for a shootdown: an object made before the
    // purge cannot land on it, one made after can
    let overlaps = |p: u64| p < at + LEN as u64 + 4096 && at < p + LEN as u64;
    let mut before_purge: Vec<u8> = Vec::new();
    let kept_out = before_purge.try_reserve_exact(LEN).is_ok() && !overlaps(before_purge.as_ptr() as u64);
    drop(before_purge);
    crate::mm::vmap::purge();
    let mut after_purge: Vec<u8> = Vec::new();
    let reused = after_purge.try_reserve_exact(LEN).is_ok() && overlaps(after_purge.as_ptr() as u64);
    drop(after_purge);
    crate::mm::vmap::purge();
    t.check("vmap/purge", kept_out && reused,
            format!("the freed range stays out before a purge {kept_out}, is handed out again after it {reused}"));
}

/// After a burst, what the heap holds for nothing (empty slabs, full magazines)
/// is counted by its shrinker, and a scan gives it back
fn check_shrinker(t: &mut Runner) {
    let mut burst: Vec<alloc::boxed::Box<[u8; 96]>> = Vec::new();
    for i in 0..64 * 1024 {
        burst.push(alloc::boxed::Box::new([i as u8; 96]));
    }
    drop(burst);
    let s = &crate::mm::heap::SHRINKER;
    let counted = (s.count)();
    let (before, _) = pmm::stats();
    let freed = (s.scan)(usize::MAX, false);
    let (after, _) = pmm::stats();
    let left = (s.count)();
    t.check("heap/shrinker", freed > 0 && left < counted,
            format!("{counted} frames counted, {freed} given back by a scan ({} left the heap), {left} counted after",
                    before.saturating_sub(after)));
}

/// `slabdebug` on the boot line: a write past an object's end, a second free
/// and a write after free are each reported once, and the second free is
/// ignored. Skipped without the flag.
///
/// Every pointer goes through `black_box`: the compiler may leave out an
/// allocation nothing uses along with its free, and may assume a new one
/// differs from one freed
fn check_slabdebug(t: &mut Runner) {
    use core::alloc::Layout;
    use core::hint::black_box;
    let alloc = |l: Layout| black_box(unsafe { alloc::alloc::alloc(l) });
    if !crate::mm::heap::debugging() {
        t.skip("heap/slabdebug", String::from("skipped: boot with slabdebug to run it"));
        return;
    }
    let layout = Layout::from_size_align(40, 8).unwrap();
    let r0 = crate::mm::heap::reports();
    unsafe {
        // 40 bytes in a 48-byte class: the eight after them are the red zone
        let p = alloc(layout);
        p.add(44).write_volatile(1);
        alloc::alloc::dealloc(p, layout);
    }
    let r1 = crate::mm::heap::reports();
    let (q, again) = unsafe {
        let q = alloc(layout);
        alloc::alloc::dealloc(q, layout);
        alloc::alloc::dealloc(q, layout);
        // The second free was ignored: the object comes out once, not twice
        let a = alloc(layout);
        let b = alloc(layout);
        let twice = a == b;
        alloc::alloc::dealloc(a, layout);
        alloc::alloc::dealloc(b, layout);
        (q, twice)
    };
    let r2 = crate::mm::heap::reports();
    let reused = unsafe {
        // The last freed is the next handed out while its slab has something
        // else out (an empty slab waits behind the partial ones), so the middle
        // one of eight made in a row: a neighbour shares its slab. Another CPU
        // may take it first (no magazines under `slabdebug`), and then its
        // allocation is the one that reports
        let row: [*mut u8; 8] = core::array::from_fn(|_| alloc(layout));
        let w = row[4];
        alloc::alloc::dealloc(w, layout);
        w.add(20).write_volatile(0x55);
        let x = alloc(layout);
        alloc::alloc::dealloc(x, layout);
        for (k, &p) in row.iter().enumerate() {
            if k != 4 {
                alloc::alloc::dealloc(p, layout);
            }
        }
        x == w
    };
    let r3 = crate::mm::heap::reports();
    t.check("heap/slabdebug", r1 == r0 + 1 && r2 == r1 + 1 && !again && r3 == r2 + 1,
            format!("past the end: {} report, freed twice: {} (and {:p} handed out twice: {again}), \
                     written after free: {} (the same object again {reused})", r1 - r0, r2 - r1, q, r3 - r2));
}

fn check_cow_counts(t: &mut Runner) {
    let Some(f) = pmm::alloc_frame() else {
        t.check("frames/cow-counts", false, String::from("no frame"));
        return;
    };
    let one = pmm::ref_get(f);
    pmm::ref_inc(f);
    pmm::ref_inc(f);
    let three = pmm::ref_get(f);
    let shared = matches!(pmm::put_cow_ref(f), pmm::CowPut::Shared);
    let unique = matches!(pmm::put_cow_ref(f), pmm::CowPut::Unique);
    let last = matches!(pmm::put_cow_ref(f), pmm::CowPut::Last);
    pmm::free_frame(f);
    t.check("frames/cow-counts", one == 1 && three == 3 && shared && unique && last,
            format!("{one} holder, {three} after two more, then shared {shared}, unique {unique}, last {last}"));
}

fn check_double_free(t: &mut Runner) {
    // The frame goes into this CPU's cache on the first free; the second must
    // not put it there twice, or the next two allocations are one frame
    let Some(f) = pmm::alloc_frame() else {
        t.check("frames/double-free", false, String::from("no frame"));
        return;
    };
    pmm::free_frame(f);
    pmm::free_frame(f);
    let a = pmm::alloc_frame();
    let b = pmm::alloc_frame();
    let distinct = a.is_some() && b.is_some() && a != b;
    if let Some(a) = a { pmm::free_frame(a); }
    if let Some(b) = b { pmm::free_frame(b); }
    t.check("frames/double-free", distinct, format!("freed {f:#x} twice; then {a:x?} and {b:x?}"));
}

fn check_all_memory(t: &mut Runner) {
    // The bitmap is sized by the top of memory
    let ram_kb = pmm::total_ram_kb();
    let (_, total) = pmm::stats();
    let usable_kb = total as u64 * 4;
    if ram_kb <= 16 << 20 {
        t.skip("frames/above-16g", format!("{} MiB of RAM: nothing above 16 GiB to see", ram_kb / 1024));
        return;
    }
    t.check("frames/above-16g", usable_kb > 16 << 20,
            format!("{} MiB in the memory map, {} MiB usable frames", ram_kb / 1024, usable_kb / 1024));
}

/// `mm test`
pub fn cmd_selftest() {
    crate::cprintln!(57, 197, 187, "mm: selftest");
    let report = run();
    for c in &report.cases {
        if c.skipped {
            crate::cprintln!(220, 220, 100, "  skip  {}", c.name);
        } else if c.ok {
            crate::cprintln!(100, 220, 150, "  ok    {}", c.name);
        } else {
            crate::print_error!("  FAIL  {}", c.name);
        }
        crate::println!("          {}", c.detail);
        crate::serial_println!(
            "[mm-test] {} {} - {}",
            if c.skipped { "SKIP" } else if c.ok { "ok  " } else { "FAIL" },
            c.name,
            c.detail
        );
    }
    let (pass, fail, skip) = (report.passed(), report.failed(), report.skipped());
    crate::serial_println!("[mm-test] {} passed, {} failed, {} skipped", pass, fail, skip);
    if fail == 0 {
        crate::print_success!("mm: {} passed, {} skipped", pass, skip);
    } else {
        crate::print_error!("mm: {} passed, {} FAILED, {} skipped", pass, fail, skip);
    }
}
