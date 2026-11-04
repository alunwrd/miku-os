// BAR1 in physical mode: the CPU writes video memory with ordinary stores
//
// `dev/gpu-memory-compute.md`, step 3.1. OpenRM binds BAR1 physical when all
// instance memory is in system memory (`bBar1PhysicalModeEnabled`,
// `kern_bus.c:56`), with the bind it uses for virtual mode
// (`kbusBar1InstBlkBind_GH100`, `kern_bus_gh100.c:2593`): LOW, then HIGH, which
// triggers it, then wait for `BAR1_PENDING` and `BAR1_OUTSTANDING` to clear. In
// physical mode a BAR1 offset is a video-memory offset.
//
// Virtual mode, through the directory GSP-RM built, answers every CPU read with
// a PRI error (`roadmap-desktop.md`, stage C). `nvidia bar1 probe` binds
// physical, checks both directions against PRAMIN and binds the old value back;
// `nvidia bar1 on` keeps it and gives the console its window.

use crate::drivers::gpu::nvidia::gsp_common::vram;
use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::drivers::gpu::nvidia::pramin;

/// `NV_VIRTUAL_FUNCTION_PRIV_FUNC_BAR1_BLOCK_LOW_ADDR` / `_HIGH_ADDR`
/// (`gh100/dev_vm.h:68,85`), in BAR0's VF window at 0xb80000
const BLK_LO: u32 = 0x00b8_0f60;
const BLK_HI: u32 = 0x00b8_0f64;
/// `_MODE` 9:9, `_VIRTUAL` = 1
const MODE_VIRTUAL: u32 = 1 << 9;
/// `_BAR1_PENDING` 0:0 and `_BAR1_OUTSTANDING` 1:1, read-only
const PENDING: u32 = 1 << 0;
const OUTSTANDING: u32 = 1 << 1;
/// `GPU_TIMEOUT_DEFAULT` is seconds; a bind not settled in one has not settled
const BIND_TIMEOUT_US: u64 = 1_000_000;

/// The binding BAR1 had, to put back
#[derive(Clone, Copy, Debug)]
pub struct Binding {
    pub lo: u32,
    pub hi: u32,
}

impl Binding {
    pub fn read(bar0: &MmioRegion) -> Binding {
        Binding { lo: bar0.read32(BLK_LO), hi: bar0.read32(BLK_HI) }
    }

    pub fn is_virtual(&self) -> bool {
        self.lo & MODE_VIRTUAL != 0
    }
}

/// When BAR1 was last bound, in milliseconds since boot; 0 for never
static LAST_BIND_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// How long ago BAR1 was last bound, for a stall report to put beside the
/// channels' state (`channel::unstick_engines`)
pub fn last_bind_ms_ago() -> Option<u64> {
    let t = LAST_BIND_MS.load(core::sync::atomic::Ordering::Relaxed);
    (t != 0).then(|| crate::kcore::time::since_boot_ms().saturating_sub(t))
}

/// Bind BAR1 as `kbusBar1InstBlkBind_GH100` does: LOW (mode, target, pointer;
/// the status bits are read-only), then HIGH, which triggers it, then wait for
/// both status bits to clear.
///
/// With no submission half-written meanwhile. OpenRM binds BAR1 in
/// `kbusStateLoad`, before any channel exists; this binds with the desktop
/// running, and a doorbell store racing the bind is suspected of stalling a
/// frame on the engines (`dev/gpu-memory-compute.md`, "The stall after the
/// probe")
fn bind(bar0: &MmioRegion, lo: u32, hi: u32) -> Result<(), &'static str> {
    let sw = crate::kcore::time::Stopwatch::start();
    let r = super::channel::with_submissions_held(|| {
        bar0.write32(BLK_LO, lo & !(PENDING | OUTSTANDING));
        bar0.write32(BLK_HI, hi);
        loop {
            let v = bar0.read32(BLK_LO);
            if MmioRegion::is_pri_error(v) {
                return Err("the BAR1 block register answers a PRI error");
            }
            if v & (PENDING | OUTSTANDING) == 0 {
                return Ok(());
            }
            if sw.elapsed_us() >= BIND_TIMEOUT_US {
                return Err("the BAR1 bind never settled (PENDING/OUTSTANDING stuck)");
            }
            crate::kcore::time::udelay(10);
        }
    });
    LAST_BIND_MS.store(crate::kcore::time::since_boot_ms().max(1), core::sync::atomic::Ordering::Relaxed);
    crate::kdebug!("[gpu/bar1] bound {} in {} us: {}",
        if lo & MODE_VIRTUAL != 0 { "virtual" } else { "physical" }, sw.elapsed_us(),
        match r { Ok(()) => "settled", Err(e) => e });
    r
}

/// BAR1 physical, keeping the instance-block pointer so that binding the old
/// value back is the same bind in reverse
fn bind_physical(bar0: &MmioRegion, old: Binding) -> Result<(), &'static str> {
    bind(bar0, old.lo & !MODE_VIRTUAL, old.hi)
}

fn restore(bar0: &MmioRegion, old: Binding) -> Result<(), &'static str> {
    bind(bar0, old.lo, old.hi)
}

/// Where BAR1 is, and how much of video memory it reaches in physical mode
#[derive(Clone, Copy, Debug)]
pub struct Aperture {
    pub phys: u64,
    pub size: u64,
}

fn aperture() -> Option<Aperture> {
    crate::drivers::gpu::nvidia::with_gb20x(|d| {
        let b = d.pci.bars[1];
        (b.phys != 0 && b.size != 0 && !b.is_io).then_some(Aperture { phys: b.phys, size: b.size })
    })
    .flatten()
}

/// Physical mode kept (`nvidia bar1 on`): the aperture, and what to put back
static ON: spin::Mutex<Option<(Aperture, Binding)>> = spin::Mutex::new(None);

/// The aperture, when BAR1 is kept physical. `None` means every CPU write to
/// video memory goes through PRAMIN or the copy engine
pub fn physical() -> Option<Aperture> {
    ON.lock().map(|(a, _)| a)
}

/// A kernel pointer to video memory at `vram`, `bytes` long, write-combining,
/// when BAR1 is physical and reaches that far
pub fn map(vram: u64, bytes: u64, wc: bool) -> Option<*mut u8> {
    let a = physical()?;
    if vram.checked_add(bytes)? > a.size {
        return None;
    }
    if wc {
        crate::mm::vmm::map_mmio_wc(a.phys + vram, bytes);
    } else {
        crate::mm::vmm::map_mmio_uc(a.phys + vram, bytes);
    }
    Some(crate::arch::x86_64::grub::phys_to_virt(a.phys + vram) as *mut u8)
}

/// What `probe` found, in words
pub struct Probe {
    pub lines: alloc::vec::Vec<alloc::string::String>,
    pub ok: bool,
}

/// Bind physical, check both directions against PRAMIN on a scratch block, bind
/// the old value back. Eight words each way: a failed BAR1 access has upset
/// PRAMIN and the next RPC. Stops at the first wrong answer
pub fn probe(bar0: &MmioRegion) -> Probe {
    let mut lines = alloc::vec::Vec::new();
    let mut say = |l: alloc::string::String| lines.push(l);
    let Some(ap) = aperture() else {
        return Probe { lines: alloc::vec![alloc::string::String::from("no BAR1 in the PCI configuration")], ok: false };
    };
    let old = Binding::read(bar0);
    say(alloc::format!(
        "BAR1 {:#x} + {} MiB, bound {} (lo {:#010x} hi {:#010x})",
        ap.phys, ap.size >> 20, if old.is_virtual() { "virtual" } else { "physical" }, old.lo, old.hi
    ));
    if physical().is_some() {
        say("already kept physical ('nvidia bar1 off' first)".into());
        return Probe { lines, ok: true };
    }

    // A block of video memory nothing else is using, low enough to be in reach
    const BYTES: u64 = 0x1000;
    let Some(scratch) = vram::alloc(BYTES, 0x1000, vram::Placement::Low, "bar1 probe") else {
        say("no video memory for the scratch block".into());
        return Probe { lines, ok: false };
    };
    if scratch + BYTES > ap.size {
        vram::free(scratch);
        say(alloc::format!("the scratch block at {:#x} is past the aperture", scratch));
        return Probe { lines, ok: false };
    }

    // One pattern in through PRAMIN
    let pattern: [u32; 8] = core::array::from_fn(|i| 0x6d69_6b75 ^ (i as u32).wrapping_mul(0x0101_0101));
    let bytes: alloc::vec::Vec<u8> = pattern.iter().flat_map(|w| w.to_le_bytes()).collect();
    pramin::copy_to_vram(bar0, scratch, &bytes);
    pramin::flush_fb(bar0);

    let mut ok = false;
    match bind_physical(bar0, old) {
        Err(e) => say(alloc::format!("bind physical: {}", e)),
        Ok(()) => {
            // Read through BAR1, uncached first: nothing can go wrong in the
            // mapping itself there
            crate::mm::vmm::map_mmio_uc(ap.phys + scratch, BYTES);
            let p = crate::arch::x86_64::grub::phys_to_virt(ap.phys + scratch) as *mut u32;
            let first = unsafe { core::ptr::read_volatile(p) };
            if first != pattern[0] {
                say(alloc::format!("BAR1 read {:#010x} where PRAMIN wrote {:#010x} - stopping", first, pattern[0]));
            } else {
                let rest_ok = (1..8).all(|i| unsafe { core::ptr::read_volatile(p.add(i)) } == pattern[i]);
                say(alloc::format!("PRAMIN -> BAR1: {}", if rest_ok { "8 of 8 words" } else { "first word only" }));
                if rest_ok {
                    // And the other way, write-combining, which is the mapping
                    // the console would use
                    crate::mm::vmm::map_mmio_wc(ap.phys + scratch, BYTES);
                    for (i, w) in pattern.iter().enumerate() {
                        unsafe { core::ptr::write_volatile(p.add(8 + i), !w) };
                    }
                    unsafe { core::arch::x86_64::_mm_sfence() };
                    pramin::flush_fb(bar0);
                    let mut back = [0u32; 8];
                    pramin::sample_vram(bar0, scratch + 32, &mut back);
                    let good = back.iter().zip(pattern.iter()).all(|(b, w)| *b == !*w);
                    say(alloc::format!(
                        "BAR1 (write-combining) -> PRAMIN: {}",
                        if good { "8 of 8 words".into() } else { alloc::format!("read {:#010x}, wrote {:#010x}", back[0], !pattern[0]) }
                    ));
                    ok = good;
                }
            }
            if let Err(e) = restore(bar0, old) {
                say(alloc::format!("binding the old value back: {} - reboot before trusting BAR1", e));
            } else {
                say(alloc::format!("bound back ({})", if old.is_virtual() { "virtual" } else { "physical" }));
            }
        }
    }
    vram::free(scratch);
    Probe { lines, ok }
}

/// Keep BAR1 physical, and give the console the fast path it has waited for
pub fn on(bar0: &MmioRegion) -> Result<Aperture, &'static str> {
    if let Some(a) = physical() {
        return Ok(a);
    }
    let ap = aperture().ok_or("no BAR1 in the PCI configuration")?;
    let old = Binding::read(bar0);
    bind_physical(bar0, old)?;
    *ON.lock() = Some((ap, old));
    crate::drivers::gpu::nvidia::scanout::bar1_physical_changed();
    // And every channel stores GP_PUT through it from here (3.5b)
    super::channel::attach_userd_all();
    Ok(ap)
}

/// Bind BAR1 back the way it was. Unload does this before GSP-RM goes
pub fn off(bar0: &MmioRegion) -> Result<(), &'static str> {
    let Some((_, old)) = ON.lock().take() else { return Ok(()) };
    // Every channel's USERD window is stale before BAR1 moves (3.5b). The
    // aperture goes first and the epoch second; `channel::attach_userd` reads
    // them in the opposite order, so a channel made in between gets a window
    // stale from the start
    crate::drivers::gpu::nvidia::gsp_common::chan::userd_windows_invalidate();
    crate::drivers::gpu::nvidia::scanout::bar1_physical_changed();
    restore(bar0, old)
}
