// The screen the system is driving: the console on it, double buffering, and
// the blits the compositor submits every frame
//
// The window fetches its surface afresh every frame, so a "flip" in the EVO
// sense, a window UPDATE, is needed only to change which surface is scanned.
// Drawing is: write video memory, flush, done.
//
// `flush_fb` is the NV_UFLUSH register nouveau pokes in `g84_bar_flush`:
// without it a surface can be complete in memory and invisible to an engine
// reading it.
//
// This module is not vendor-neutral. Every function needing to know where the
// screen is calls `blackwell::boot::live_scanout()`, and the copy-engine
// submissions go to `blackwell::boot::ce_*`. Those call sites are the seam a
// second card would replace. There is no `ScanoutBackend` trait, since there is
// one implementation. `Surface` and its drawing primitives, which have no GPU
// in them, are in `surface.rs`.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::mmio::MmioRegion;
use super::pramin;

// `Surface` and its drawing primitives are in `surface.rs`, re-exported here
// for callers of `scanout::Surface`.
pub use super::surface::{rgb, Surface};

extern crate alloc;
use alloc::vec::Vec;

// The console on this monitor
//
// MikuOS's console renders through a raw pointer with a stride, and once GSP-RM
// has the display no pointer reaches the scanned-out surface: BAR1 is the
// vidmem aperture and GSP-RM remaps it to virtual mode during display init, so
// GRUB's framebuffer addresses nothing.
//
// So the console is pointed at a buffer in system memory, and what it draws is
// copied into video memory here, through PRAMIN, one MMIO write per pixel. The
// console reports the rectangle it touched, so a typed character costs about a
// hundred and fifty writes, not the two million of a full screen.

/// The scanout surface as ordinary memory, when BAR1 has been made to reach it.
///
/// Set by `bar1_physical_changed` when BAR1 is bound physical
/// (`blackwell::bar1`, `nvidia bar1 on`): a BAR1 offset is then a video-memory
/// offset and the surface's own address is its window. Absent, every flush
/// takes the copy engine or, failing that, PRAMIN one word at a time
/// (`gmmu.rs`, `dev/roadmap-desktop.md` stage C)
static BAR1: spin::Mutex<Option<super::gmmu::Bar1Window>> = spin::Mutex::new(None);

pub fn bar1_window() -> Option<super::gmmu::Bar1Window> {
    *BAR1.lock()
}

/// Where the console draws, and where those pixels have to end up
struct ConsoleTarget {
    /// The console's framebuffer, in system memory. Leaked on attach: the
    /// `Console` holds `&'static mut [u8]` over the same allocation
    buf: *const u32,
    /// Pixels per scanline in `buf`; the surface's own pitch may be larger
    stride: usize,
    /// Physical address of `buf`, when it came from a contiguous DMA
    /// allocation. `None` means the copy engine cannot read it and every flush
    /// goes through PRAMIN a pixel at a time
    phys: Option<u64>,
    surface: Surface,
    bar0: MmioRegion,
}

// The pointer is into a leaked allocation that outlives everything, and the
// only other user of it is the Console writing the pixels this reads
unsafe impl Send for ConsoleTarget {}

static CONSOLE: spin::Mutex<Option<ConsoleTarget>> = spin::Mutex::new(None);

/// Move MikuOS's console onto the surface this card is scanning out.
///
/// The console repaints itself from its own character grid onto the new
/// framebuffer, so the boot log, the prompt and the line being typed come
/// across.
///
/// Returns the geometry and how long the first, full-screen push took, which is
/// the cost of a scroll: a scroll dirties the whole text area.
///
/// The BAR is an argument since the only caller is `gb206-boot`, attaching from
/// inside its own `with_gb20x` closure when the window arms
pub fn attach_console_with(bar0: MmioRegion) -> Result<(u32, u32, u64), &'static str> {
    use crate::drivers::gpu::nvidia::blackwell::boot;

    if CONSOLE.lock().is_some() {
        return Err("console is already on the scanout surface");
    }
    let live = boot::live_scanout().ok_or("nothing is being scanned out")?;
    // Before the allocation below: a screen's worth of buffer is 8 MiB, leaked
    // on the way to the console
    if !crate::io::console::exists() {
        return Err("there is no console to move");
    }

    let (w, h) = (live.width as usize, live.height as usize);
    // The shadow is what the copy engine reads the console from, so it is
    // device memory: one run of frames, with its physical address. A screen is
    // 8 MiB in one piece, asked for right after a GSP-RM boot has had tens of
    // megabytes of firmware through memory; with no run that long the console
    // still moves, onto the heap, with no address for the copy engine and the
    // slower path (`dev/kernel-core.md`, "Devices get device memory")
    let bytes_len = w * h * 4;
    let (bytes, phys): (&'static mut [u8], Option<u64>) =
        match crate::mm::dma::DmaBuffer::alloc(bytes_len.div_ceil(4096)) {
            Ok(mut buf) => {
                buf.zero();
                let (all, phys) = buf.leak();
                (&mut all[..bytes_len], Some(phys))
            }
            Err(_) => {
                // Leaked, and handed over as bytes: one `&'static mut` over the
                // allocation, never a second view of it
                let mut pixels: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
                if pixels.try_reserve_exact(w * h).is_err() {
                    crate::serial_println!(
                        "[gb206] console buffer: {} KiB refused, {} KiB free on the heap",
                        bytes_len / 1024, crate::mm::heap::free() / 1024
                    );
                    return Err("not enough memory for a screen-sized buffer");
                }
                pixels.resize(w * h, 0);
                let px: &'static mut [u32] = pixels.leak();
                let n = px.len() * 4;
                (unsafe { core::slice::from_raw_parts_mut(px.as_mut_ptr() as *mut u8, n) }, None)
            }
        };
    let buf = bytes.as_ptr() as *const u32;
    crate::serial_println!(
        "[gb206] console shadow virt {:#x} phys {:#x?} contiguous {}",
        buf as u64, phys, phys.is_some()
    );
    crate::serial_println!(
        "[gb206] console buffer {}x{} at {:p}, heap {} KiB free",
        w, h, buf, crate::mm::heap::free() / 1024
    );
    // is_bgr must be true: `Console::pixel_value` with it set produces
    // 0xFF000000 | r<<16 | g<<8 | b, how the surface stores a pixel. With it
    // clear the console comes up with red and blue swapped
    let retargeted = crate::io::console::retarget(
        bytes,
        crate::io::console::FrameBufferConfig {
            width: w,
            height: h,
            stride: w,
            bytes_per_pixel: 4,
            is_bgr: true,
        },
    );
    if !retargeted {
        return Err("the console refused the new geometry");
    }
    crate::serial_println!(
        "[gb206] console retargeted onto vram {:#x} pitch {} - pushing first paint",
        live.vram, live.pitch
    );

    // The head's own refresh rate, read back from the raster the modeset just
    // programmed. `may_output.mode` is what a client paces itself against. 60
    // Hz stands in only if the head is not scanning yet
    let refresh_mhz = raster_timing(&bar0)
        .map(|t| t.refresh_mhz())
        .filter(|&m| m > 0)
        .unwrap_or(60_000) as i32;

    *CONSOLE.lock() = Some(ConsoleTarget {
        buf,
        stride: w,
        phys,
        surface: Surface {
            vram: live.vram,
            pitch: live.pitch,
            width: live.width,
            height: live.height,
        },
        bar0,
    });
    // The screen the compositor lays windows out on is this one now.
    //
    // `mayland::init` publishes GRUB's framebuffer at boot, which differs once
    // the live mode does. With the console as a window, the layout has nowhere
    // else to learn the screen's size
    crate::mayland::server::set_output(
        live.width as i32, live.height as i32, refresh_mhz);
    // Through BAR1, if it is bound physical: the window is the new surface's
    bar1_physical_changed();
    crate::io::console::set_flush_hook(|| { flush_console(); });
    // The console marked its whole area damaged when repainted, so this first
    // flush is a full-screen write: two million MMIO writes, the only push that
    // large
    let sw = crate::kcore::time::Stopwatch::start();
    let pushed = flush_console();
    let ms = sw.elapsed_ms();
    crate::serial_println!(
        "[gb206] first paint: {} pixels in {} ms", pushed, ms);
    Ok((live.width, live.height, ms))
}

/// Stop carrying the console to the panel. The console keeps drawing into the
/// system-memory buffer; nothing shows it
pub fn detach_console() -> bool {
    crate::io::console::clear_flush_hook();
    // The window described the surface being given up
    BAR1.lock().take();
    CONSOLE.lock().take().is_some()
}

/// BAR1 was bound physical, or bound back (`blackwell::bar1`): point the
/// console's window at its surface through BAR1, or take the window away.
/// Physical mode makes a BAR1 offset a video-memory offset, so the window is
/// the surface's own address, when the aperture reaches that far
pub fn bar1_physical_changed() {
    let surface = CONSOLE.lock().as_ref().map(|t| t.surface);
    let win = surface.and_then(|s| {
        let bytes = s.pitch as u64 * s.height as u64;
        let cpu = super::blackwell::bar1::map(s.vram, bytes, true)?;
        Some(super::gmmu::Bar1Window { va: s.vram, cpu, bytes })
    });
    *BAR1.lock() = win;
}

pub fn console_attached() -> bool {
    CONSOLE.lock().is_some()
}

/// Stop pushing the console to the panel without giving up the target.
///
/// For when the compositor has the screen. Resuming is one call, with no second
/// search for eight megabytes of contiguous memory
pub fn suspend_console_flush() {
    crate::io::console::clear_flush_hook();
}

/// Carry the console to the panel again. False if there is no target to carry
/// it to
pub fn resume_console_flush() -> bool {
    if CONSOLE.lock().is_none() {
        return false;
    }
    crate::io::console::set_flush_hook(|| { flush_console(); });
    true
}

/// Reinstall the hook without asking whether there is a target.
///
/// For the panic path: a lock held by a just-halted thread is never released,
/// so this installs a hook that does nothing with no target. The flush itself
/// still takes that lock, so a panic with the target lock held loses the screen
/// either way
pub fn force_console_flush_hook() {
    crate::io::console::set_flush_hook(|| { flush_console(); });
}

/// How long the last flush took, in microseconds, and whether the copy engine
/// did it. Read by the shell
static LAST_FLUSH: spin::Mutex<(u64, bool, &'static str)> =
    spin::Mutex::new((0, false, "nothing flushed yet"));

pub fn last_flush() -> (u64, bool, &'static str) {
    *LAST_FLUSH.lock()
}

pub fn flush_console() -> usize {
    let Some((x0, y0, x1, y1)) = crate::io::console::take_damage() else {
        return 0;
    };
    let guard = CONSOLE.lock();
    let Some(t) = guard.as_ref() else {
        return 0;
    };

    let t0 = crate::kcore::time::rdtsc();
    let mut by_ce = false;
    let mut why = "";
    let x1 = x1.min(t.surface.width as usize);
    let y1 = y1.min(t.surface.height as usize);
    if x0 >= x1 || y0 >= y1 {
        return 0;
    }

    match bar1_window() {
        // The surface is ordinary memory: one memcpy per scanline, and the
        // write-combining mapping turns those into full-width bursts. Only a
        // window onto this very surface: one left over from the last is not
        Some(w) if w.va == t.surface.vram => {
            let pitch = t.surface.pitch as usize;
            for y in y0..y1 {
                let src = unsafe { t.buf.add(y * t.stride + x0) as *const u8 };
                let dst = unsafe { w.cpu.add(y * pitch + x0 * 4) };
                unsafe { core::ptr::copy_nonoverlapping(src, dst, (x1 - x0) * 4) };
            }
            super::gmmu::flush(&t.bar0);
        }
        _ => {
            // The copy engine, if there is one and the shadow is contiguous.
            // One 2D blit for the whole damaged rectangle: `lines` rows of
            // `(x1-x0)*4` bytes, source pitch the shadow's, destination pitch
            // the surface's. A full-screen repaint costs about a millisecond
            // here, against forty-two
            let done = match t.phys {
                None => {
                    why = "the console shadow is not physically contiguous";
                    false
                }
                Some(_) if !super::blackwell::boot::copy_engine_ready() => {
                    why = "there is no copy-engine channel";
                    false
                }
                Some(base) => {
                    let src = base + (y0 * t.stride + x0) as u64 * 4;
                    let dst = t.surface.vram + y0 as u64 * t.surface.pitch as u64
                        + x0 as u64 * 4;
                    super::blackwell::boot::ce_copy_to_vram(
                        &t.bar0,
                        src,
                        dst,
                        (t.stride * 4) as u32,
                        t.surface.pitch,
                        ((x1 - x0) * 4) as u32,
                        (y1 - y0) as u32,
                    )
                }
            };
            by_ce = done;
            if !done && why.is_empty() {
                why = "the copy engine did not signal in time";
            }
            if !done {
                let mut win = pramin::Window::new(&t.bar0);
                for y in y0..y1 {
                    let row = unsafe {
                        core::slice::from_raw_parts(t.buf.add(y * t.stride + x0), x1 - x0)
                    };
                    t.surface.write_span(&mut win, x0 as u32, y as u32, row);
                }
                drop(win);
                // Otherwise the writes can be complete in memory and invisible
                // to the engine fetching it. `gb206-draw` ends with the same
                // flush
                pramin::flush_fb(&t.bar0);
            }
        }
    }
    *LAST_FLUSH.lock() = (
        crate::kcore::time::rdtsc().wrapping_sub(t0)
            / (crate::kcore::time::tsc_khz().max(1) / 1000).max(1),
        by_ce,
        if by_ce { "" } else { why },
    );
    (x1 - x0) * (y1 - y0)
}

/// The raster's frame number for the head that is scanning out.
///
/// `NV_PDISP_RG_IN_LOADV_COUNTER` increments once per scanned frame;
/// `kheadGetLoadVCounter_v05_01` in RM reads this register. A polled vertical
/// sync needs nothing programmed
pub fn frame_counter(bar0: &MmioRegion) -> Option<u32> {
    let head = super::blackwell::boot::live_scanout()?.head;
    Some(super::gsp_common::modeset::head_frame_counters(bar0, head).0)
}

/// The head's vblank interrupt, counted.
///
/// Notified by the display interrupt handler once per frame. The LOADV counter
/// below says the same, but watching it costs a core. One waiter slot: the one
/// thing waiting for a frame boundary is the compositor
static VBLANK: crate::kcore::isr_event::IsrEvent<1> = crate::kcore::isr_event::IsrEvent::new();

/// Called from interrupt context, once per frame. Atomics and one
/// `sched::wakeup`, which is safe from an ISR
pub fn note_vblank() {
    VBLANK.notify();
}

/// How many frames the interrupt has counted. For the shell, and for telling
/// "the head stopped" from "nobody enabled the interrupt"
pub fn vblank_count() -> u64 {
    VBLANK.seq()
}

/// Block until the raster starts a new frame. False on timeout means the head
/// stopped scanning.
///
/// With the display interrupt up the thread leaves the run queue and the frame
/// boundary wakes it. Without it the LOADV counter is polled at 5 us: with
/// `nointr`, before `gb206-boot` has armed anything, and in the case below.
///
/// Sleeping needs two surfaces. A wake makes a thread runnable, not running:
/// `sched::wakeup` queues it, and on the CPU taking the interrupt nothing
/// preempts until the next tick, 4 ms against a 16.6 ms frame. A compositor
/// drawing into the surface being scanned would then write in front of the
/// beam. With two surfaces the frame goes into one nobody is scanning, so
/// giving up the poll is free
pub fn wait_next_frame(bar0: &MmioRegion, timeout_us: u64) -> bool {
    if super::intr::active() && VBLANK.seq() != 0 && flipping() {
        return wait_next_frame_intr(timeout_us);
    }
    let Some(start) = frame_counter(bar0) else { return false };
    let mut waited = 0u64;
    while waited < timeout_us {
        if frame_counter(bar0) != Some(start) {
            return true;
        }
        crate::kcore::time::udelay(5);
        waited += 5;
    }
    false
}

/// Which of the two `wait_next_frame` would take now, and why not the other.
/// For the shell: "the interrupt arrives" differs from "the frame loop uses it"
pub fn vsync_path() -> &'static str {
    if !super::intr::active() {
        "polled - no GPU interrupt"
    } else if VBLANK.seq() == 0 {
        "polled - no vblank has arrived yet"
    } else if !flipping() {
        "polled - one surface, so the frame has to start in the blanking interval"
    } else {
        "asleep until vblank"
    }
}

/// The interrupt-driven half of `wait_next_frame`.
///
/// A timer tick is 4 ms and a frame is 16.6, so sleeping a tick at a time is
/// fine, and the vblank ends the wait. The backstop turns a missed wake into
/// latency
fn wait_next_frame_intr(timeout_us: u64) -> bool {
    let start = VBLANK.seq();
    let t0 = crate::kcore::time::rdtsc();
    let khz = crate::kcore::time::tsc_khz().max(1);
    loop {
        if VBLANK.seq() != start {
            return true;
        }
        if crate::kcore::time::rdtsc().wrapping_sub(t0) * 1000 / khz >= timeout_us {
            return false;
        }
        VBLANK.sleep_unless(start, 1, || false);
    }
}

/// Which line the raster is on, and how far across it.
///
/// `NV_PDISP_RG_DPCA(head)` holds the vertical position and the next register
/// the horizontal one; reading the vertical latches the horizontal, so the pair
/// is consistent. This is `gv100_head_rgpos` in nouveau, and it tells whether a
/// copy landed in the blanking interval or in front of the beam
pub fn raster_pos(bar0: &MmioRegion) -> Option<(u16, u16)> {
    let head = super::blackwell::boot::live_scanout()?.head;
    let off = head * 0x800;
    let vline = (bar0.read32(REG_RG_DPCA + off) & 0xffff) as u16;
    let hline = (bar0.read32(REG_RG_DPCA + 4 + off) & 0xffff) as u16;
    Some((vline, hline))
}

const REG_RG_DPCA: u32 = 0x0061_6330;

/// What the panel is running at, in whole hertz. Sixty when nothing is
/// scanning.
///
/// So a limit expressed as "eight frames" can mean the same time on a 60 Hz
/// panel and a 240 Hz one
pub fn refresh_hz() -> u32 {
    (refresh_mhz() + 500) / 1000
}

/// The same, in thousandths of a hertz: what `may_output.mode` carries, and the
/// only form telling 59.94 from 60
pub fn refresh_mhz() -> u32 {
    super::blackwell::boot::live_scanout()
        .and_then(|_| console_bar0())
        .and_then(|bar0| raster_timing(&bar0))
        .map(|t| t.refresh_mhz())
        .filter(|&m| m > 0)
        .unwrap_or(60_000)
}

pub fn raster_timing(bar0: &MmioRegion) -> Option<super::gsp_common::modeset::RasterTiming> {
    let head = super::blackwell::boot::live_scanout()?.head;
    let t = super::gsp_common::modeset::head_raster(bar0, head);
    if t.valid() { Some(t) } else { None }
}

// Two surfaces and a flip
//
// Weston's DRM backend commits a new framebuffer and the kernel latches it at
// the vertical blank; tearing is a flag you ask for. The hardware does the same
// directly: `SET_SURFACE_ADDRESS_ISO` with `MIN_PRESENT_INTERVAL 1` stages an
// address and the window arms it at the next frame boundary. So a frame is
// composed into whichever surface is not being scanned and handed over whole,
// however long it took.
//
// The swap moves `ConsoleTarget.surface.vram`, which every drawing path in this
// file writes through: the console's PRAMIN spans and the copy-engine blits.

/// The two surfaces, when there are two
struct Flip {
    /// Physical video memory of each. `front` says which the head is scanning;
    /// the other is being drawn into
    vram: [u64; 2],
    front: usize,
    /// Where the window channel's ring has been written up to. The channel's
    /// state, kept between flips
    push_cur: usize,
    /// Flips asked for, and flips the window channel refused
    count: u64,
    refused: u64,
}

static FLIP: spin::Mutex<Option<Flip>> = spin::Mutex::new(None);


pub fn flipping() -> bool {
    FLIP.lock().is_some()
}

/// Both surfaces, for anything needing all the addresses a frame can be
/// composed into before the first frame
pub fn flip_surfaces() -> Option<[u64; 2]> {
    FLIP.lock().as_ref().map(|f| f.vram)
}

/// Which surface a frame composed now would land in, or None when there is only
/// one.
///
/// Damage is owed to a surface until drawn into that surface, and this does not
/// alternate reliably: a refused flip draws the same one twice in a row
pub fn draw_slot() -> Option<usize> {
    FLIP.lock().as_ref().map(|f| 1 - f.front)
}

/// Flips asked for, and flips the window channel would not take
pub fn flip_counts() -> (u64, u64) {
    FLIP.lock().as_ref().map(|f| (f.count, f.refused)).unwrap_or((0, 0))
}

/// Put a second surface behind the one being scanned, so frames can be composed
/// whole and handed over at the frame boundary.
///
/// The back buffer is its own allocation, the same size and alignment as the
/// front. It starts as a copy of the front, since the first flip shows it
pub fn enable_flip() -> Result<(), &'static str> {
    // FLIP before CONSOLE, the order on every path taking both
    if FLIP.lock().is_some() {
        return Err("there are already two surfaces");
    }
    let guard = CONSOLE.lock();
    let Some(t) = guard.as_ref() else { return Err("the console is not on a scanout surface") };
    let live = super::blackwell::boot::live_scanout()
        .ok_or("nothing is being scanned out")?;
    if live.vram != t.surface.vram {
        return Err("the surface being scanned is not the one being drawn into");
    }

    let bytes = t.surface.pitch as u64 * t.surface.height as u64;
    let back = super::gsp_common::vram::alloc(
        bytes,
        super::blackwell::boot::SURFACE_ALIGN,
        super::gsp_common::vram::Placement::Low,
        "back surface",
    )
    .ok_or("no video memory for a second surface")?;

    // The first flip shows this, so it starts as what is already there
    if !super::blackwell::boot::ce_copy_to_vram(
        &t.bar0, live.vram, back, t.surface.pitch, t.surface.pitch,
        t.surface.pitch, t.surface.height,
    ) {
        return Err("the copy engine would not seed the second surface");
    }

    // Where the modeset left the window channel's ring. A writer starting at
    // zero would set PUT behind GET and hand the channel the rest of the ring
    // as methods; on an idle channel GET is that cursor
    let push_cur = super::gsp_common::pushbuf::channel_get(
        &t.bar0, super::gsp_common::pushbuf::wndw_chan_user(live.window));

    // The console lock goes before FLIP is taken. Every path holding both takes
    // FLIP first, and the frame loop holds CONSOLE inside
    // `blit_batch_to_screen` and asks `flipping()` for FLIP. Two spin locks
    // with no timeout
    drop(guard);
    *FLIP.lock() = Some(Flip {
        vram: [live.vram, back],
        front: 0,
        push_cur,
        count: 0,
        refused: 0,
    });
    // Everything now draws into the back buffer
    if let Some(t) = CONSOLE.lock().as_mut() {
        t.surface.vram = back;
    }
    Ok(())
}

/// Video memory nothing scans out, for the blur chain's intermediate levels.
///
/// Video memory since the GPU reads and writes those levels and the processor
/// never looks at them.
///
/// The caller names the block, for the map a person reads: a full-screen
/// blurred region and a 240x135 level are both scratch
pub fn vram_scratch(bytes: u64, owner: &'static str) -> Option<u64> {
    super::gsp_common::vram::alloc(
        bytes,
        super::blackwell::boot::SURFACE_ALIGN,
        super::gsp_common::vram::Placement::Low,
        owner,
    )
}

/// GSP-RM was unloaded: nothing is scanned out, and the two surfaces were video
/// memory the allocator no longer describes. Taken without `disable_flip`,
/// which would free one into that allocator
pub fn forget() {
    detach_console();
    FLIP.lock().take();
    BAR1.lock().take();
}

/// Back to one surface: whatever is being scanned stays on the screen and
/// drawing goes to it directly
pub fn disable_flip() {
    let Some(f) = FLIP.lock().take() else { return };
    let live = f.vram[f.front];
    if let Some(t) = CONSOLE.lock().as_mut() {
        t.surface.vram = live;
    }
    // The one nobody is scanning goes back. Which that is depends on the number
    // of flips: after an odd number the head is reading what started as the
    // back buffer
    super::gsp_common::vram::free(f.vram[1 - f.front]);
    // The one that stayed is the screen from here on. After an odd number of
    // flips that is the block called "back surface", so it is renamed for the
    // map
    super::gsp_common::vram::rename(live, "scanout surface");
}

/// The same, for the panic path.
///
/// A panic while the compositor runs would otherwise print into the surface
/// nobody is scanning.
///
/// `try_lock` throughout, as in `force_console_flush_hook`: a lock held by a
/// just-halted thread is never released. If one is held the screen is lost
/// either way: the flush that follows takes the same target lock
pub fn force_front_surface() {
    let Some(mut guard) = FLIP.try_lock() else { return };
    let Some(f) = guard.take() else { return };
    if let Some(mut g) = CONSOLE.try_lock() {
        if let Some(t) = g.as_mut() {
            t.surface.vram = f.vram[f.front];
        }
    }
}

/// Whether the flip makes the hardware wait for the copy engine.
///
/// The hardware path is what the reference driver does, but its failure mode is
/// worse than the processor-side wait: a window told to wait for a value
/// nothing will write stops arming for good, with no hardware timeout.
///
/// `noflipsema` on the boot line turns it off, the same shape as `nogpu`,
/// `nointr`, `nosmap` and `edidbase`
static FLIP_SEMA: AtomicBool = AtomicBool::new(true);

/// Nothing here remembers an armed acquire.
///
/// Writing the engine's fence word from the processor to free a stalled window
/// would make `wait_fence` and `ce_wait_last` succeed for a batch the engine
/// has not finished, since that address is the copy engine's own fence. Instead
/// the pipeline is held to one flip, so the window is never parked on a fence
/// more than one batch old. See `acq_fence`.

/// Turn the hardware path off before it is used: `noflipsema`
pub fn disable_flip_semaphore() {
    FLIP_SEMA.store(false, Ordering::Relaxed);
}

/// Turned off for the rest of the boot when the window stops retiring flips.
///
/// A one-way latch: coming back to the mechanism would stop the window again.
/// With the pipeline held to one flip there should be nothing to latch; if this
/// fires, the serial line says so
static FLIP_SEMA_GIVEN_UP: AtomicBool = AtomicBool::new(false);

/// Is the hardware holding the flip back now? False means the frame loop waits
/// for the engine itself
pub fn flip_semaphore_active() -> bool {
    FLIP_SEMA.load(Ordering::Relaxed) && !FLIP_SEMA_GIVEN_UP.load(Ordering::Relaxed)
}

/// Stop using it, for the rest of the boot
pub fn give_up_flip_semaphore() {
    if FLIP_SEMA.load(Ordering::Relaxed) && !FLIP_SEMA_GIVEN_UP.swap(true, Ordering::Relaxed) {
        crate::serial_println!(
            "[nvidia] the window stopped retiring flips - the processor waits for the \
             copy engine again");
    }
}

/// The fence a flip should wait on, or None when it should not wait at all
fn acq_fence() -> Option<(u64, u32)> {
    flip_semaphore_active()
        .then(super::blackwell::boot::ce_fence_target)
        .flatten()
}

/// Should this frame arm a flip at all?
///
/// This keeps the ring from backing up. `SET_PRESENT_CONTROL` is
/// `MIN_PRESENT_INTERVAL 1`, so an update is eligible only at a frame boundary,
/// and the acquire holds that same update: an acquire unsatisfied at the
/// boundary costs the whole next period.
///
/// Arming unconditionally while the engine copies retires at best one flip
/// every two frames, against one pushed per frame. The window channel's ring is
/// 1023 dwords, about thirty flips, so it backs up in a fraction of a second
/// and `reserve` spins inside the frame loop.
///
/// So a flip is armed only when the copies it would show have landed. Otherwise
/// the composed surface stays and the next frame shows it: one frame of
/// latency. The reference also refuses to send a flip while one is pending on
/// the layer
pub fn should_flip() -> bool {
    if !flip_semaphore_active() {
        return true;
    }
    if super::blackwell::boot::ce_idle() {
        SKIPPED_FLIPS.store(0, Ordering::Relaxed);
        return true;
    }
    // A gate must not stop the screen for ever: the compositor would keep
    // composing into a surface nobody scans, which looks like a dead machine.
    //
    // So the skip is a delay. After a few frames the flip goes out regardless:
    // a torn frame beats a still one, and reaching this means the engine is not
    // keeping up, which is reported
    SKIPPED_TOTAL.fetch_add(1, Ordering::Relaxed);
    let n = SKIPPED_FLIPS.fetch_add(1, Ordering::Relaxed) + 1;
    if n >= max_skipped_flips() {
        SKIPPED_FLIPS.store(0, Ordering::Relaxed);
        return true;
    }
    false
}

/// Consecutive frames whose flip was held back waiting for the copy engine
static SKIPPED_FLIPS: AtomicU64 = AtomicU64::new(0);

/// How many in a row before the flip goes out anyway.
///
/// About twelve milliseconds, whatever the panel runs at: long enough for an
/// engine that is merely late, short enough that nobody sees a still picture.
/// Derived, since "three frames" is a third of the patience at 240 Hz
fn max_skipped_flips() -> u64 {
    (refresh_hz() as u64 * 12 / 1000).clamp(1, 16)
}

/// How many flips have been held back for the engine, for `mayland status`
pub fn skipped_flips() -> u64 {
    SKIPPED_TOTAL.load(Ordering::Relaxed)
}

static SKIPPED_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Rectangles the backdrop buffer would not take.
///
/// Apart from `refused`: the screen still got them, so the frame is correct on
/// the panel. Non-zero means the buffer and the screen disagree about their
/// size, a bring-up bug
static BACKDROP_REFUSED: AtomicU64 = AtomicU64::new(0);

/// For `nvidia blur`
pub fn backdrop_refused() -> u64 {
    BACKDROP_REFUSED.load(Ordering::Relaxed)
}

pub fn flip() -> bool {
    use super::gsp_common::{modeset, pushbuf::PushBuf};

    let mut guard = FLIP.lock();
    let Some(f) = guard.as_mut() else { return false };
    let back = 1 - f.front;
    f.count += 1;

    let Some(live) = super::blackwell::boot::live_scanout() else {
        f.refused += 1;
        return false;
    };
    let (bar0, pitch, width, height) = {
        let g = CONSOLE.lock();
        let Some(t) = g.as_ref() else { f.refused += 1; return false };
        (t.bar0.clone(), t.surface.pitch, t.surface.width, t.surface.height)
    };

    // SET_SURFACE_ADDRESS, the geometry that goes with it, and UPDATE: about
    // twenty dwords, reserved so the ring winds before the batch and not in the
    // middle
    let user = super::gsp_common::pushbuf::wndw_chan_user(live.window);
    // The display pushbuffers alone: a flip never waits on GSP-RM, so it takes
    // none of what an RPC holds
    let ok = super::blackwell::boot::with_disp_mem(|mem| {
        // By head: one window pushbuffer per head. `get_mut`, since a bad head
        // index should be a refused flip, not a panic in the compositor's frame
        // loop
        let Some(Some(buf)) = mem.wndw_push.get_mut(live.head as usize)
        else { return false };
        let mut push = PushBuf::resume(buf.as_mut_slice(), user, f.push_cur);
        // Surface address first, then the semaphore holding the arm back, then
        // UPDATE: the order of `EvoProgramSemaphore6` (`nvkms-evo3.c:3719`,
        // called at the end of `nvEvoFlipC6`). Nothing takes effect until
        // UPDATE, and matching the reference lets the streams be compared.
        //
        // The semaphore keeps the head from latching a half-written surface.
        // Without it the window arms at the next frame boundary whether or not
        // the copies have landed. `fence` is the engine's monotonic counter, so
        // a prefetched value cannot match
        let fence = acq_fence();
        let built = push.reserve(&bar0, 40, 100_000_000).is_ok()
            && modeset::window_set_image(
                   &mut push, f.vram[back], pitch, width, height,
                   modeset::SURFACE_TARGET_VRAM,
               ).is_ok()
            && match fence {
                Some((phys, value)) => modeset::window_set_acq_semaphore(
                    &mut push, phys, modeset::SURFACE_TARGET_PCI_COHERENT, value,
                ).is_ok(),
                // Cleared: the slot is channel state, not part of the update,
                // so a window that stopped using it would keep the last acquire
                // armed
                None => modeset::window_clear_acq_semaphore(&mut push).is_ok(),
            }
            // Not interlocked with the core: the head is not changing, only
            // which surface the window fetches. The plain page flip
            // `window_update` describes
            && modeset::window_update(&mut push, false, 0).is_ok();
        if built {
            push.kick(&bar0);
            f.push_cur = push.cursor();
            return true;
        }

        // A failed push means a ring whose cursor we no longer know.
        //
        // `reserve` winds the ring before it waits for room: it writes the
        // jump, moves the cursor to zero and puts PUT there. If it then times
        // out, the hardware has wound and this side has dropped the cursor, so
        // the next flip resumes at the old position, reads GET as zero, and
        // waits for GET to leave zero without kicking. Every flip then does the
        // same, `front` never moves, and the monitor holds one still image.
        //
        // GET is true whatever `reserve` did, and `enable_flip` seeds this
        // cursor from it too: on an idle channel GET is the cursor. So the ring
        // resynchronises from the hardware
        f.push_cur = super::gsp_common::pushbuf::channel_get(&bar0, user);
        false
    })
    .unwrap_or(false);

    if !ok {
        f.refused += 1;
        return false;
    }

    // The one just handed over becomes the front, and drawing moves to the
    // other. Nothing waits for the arm: the surface taken to draw into has been
    // on the screen for the last frame, and the compositor leaves it until the
    // next frame boundary
    f.front = back;
    // What the monitor shows is now the surface just handed over. The window
    // arms it at the next frame boundary, so this is a frame early, the same
    // approximation as `t.surface.vram` below, erring on the safe side.
    //
    // `live_scanout` is what `enable_flip` compares against and what anything
    // asking "where is the picture" reads
    super::blackwell::boot::set_live_surface(f.vram[back]);
    let draw = f.vram[1 - back];
    drop(guard);
    if let Some(t) = CONSOLE.lock().as_mut() {
        t.surface.vram = draw;
    }
    true
}

/// Push a rectangle of system memory onto the scanning surface, once, with the
/// copy engine.
///
/// The caller owns the source buffer and its physical address. Returns false if
/// there is no copy engine or no live surface. Nothing animates yet
#[allow(dead_code)]
pub fn blit_to_screen(
    src_phys: u64,
    src_pitch: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> bool {
    let guard = CONSOLE.lock();
    let Some(t) = guard.as_ref() else { return false };
    if x + w > t.surface.width || y + h > t.surface.height {
        return false;
    }
    let dst = t.surface.vram + y as u64 * t.surface.pitch as u64 + x as u64 * 4;
    super::blackwell::boot::ce_copy_to_vram(
        &t.bar0, src_phys, dst, src_pitch, t.surface.pitch, w * 4, h,
    )
}

/// One rectangle of the screen to overwrite, for `blit_batch_to_screen`
#[derive(Clone, Copy)]
pub struct ScreenBlit {
    pub src_phys: u64,
    pub src_pitch: u32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// The source is video memory, the compositor's copy of a window
    /// (`mayland::render::texture`). Only the engine can read one; the
    /// processor paths refuse it
    pub src_vram: bool,
}

/// Push several rectangles onto the scanning surface in one go.
///
/// Returns (copied, refused). The engine executes the list front to back, so
/// the last of a stack of windows lands on top.
///
/// A caller with more than one rectangle comes here: looping over
/// `blit_to_screen` leaves the processor between the copies, and a preempted
/// compositor thread has half a frame on screen meanwhile
pub fn blit_batch_to_screen(blits: &[ScreenBlit]) -> (u64, u64) {
    use super::gsp_common::chan::{Copy2d, MAX_BATCH};

    // Asked before the console lock. Every other path touching both (`flip`,
    // `enable_flip`, `disable_flip`) takes FLIP first; two orders for two spin
    // locks deadlock the frame loop against the watcher thread starting or
    // stopping the second surface
    let deferred = flipping();

    let guard = CONSOLE.lock();
    let Some(t) = guard.as_ref() else { return (0, blits.len() as u64) };

    let mut copies: Vec<Copy2d> = Vec::with_capacity(blits.len().min(MAX_BATCH));
    let (mut ok, mut refused) = (0u64, 0u64);

    // Anything off the screen is refused here: the engine would take the
    // addresses at their word.
    //
    // Counted apart from an engine refusing a batch: different bugs with the
    // same symptom, a rectangle nobody wrote. The caller redraws the whole
    // frame either way
    for b in blits {
        if b.w == 0 || b.h == 0
            || b.x + b.w > t.surface.width || b.y + b.h > t.surface.height
        {
            refused += 1;
            REFUSED_BOUNDS.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        copies.push(Copy2d {
            src: b.src_phys,
            dst: t.surface.vram + b.y as u64 * t.surface.pitch as u64 + b.x as u64 * 4,
            pitch_in: b.src_pitch,
            pitch_out: t.surface.pitch,
            line_len: b.w * 4,
            lines: b.h,
            src_sysmem: !b.src_vram,
            dst_sysmem: false,
        });
    }

    // Past the batch limit the list is split, putting the processor between
    // chunks; 48 rectangles a frame is more than any stack of windows produces.
    //
    // Whether the frame may be left running depends on what it is written into.
    // With two surfaces the copies go into one nobody scans, shown at the next
    // frame boundary, so the engine has a whole raster period and the processor
    // need not watch. With one surface they go into the surface the beam is
    // crossing, and only landing inside the blanking interval keeps a
    // half-written window off the panel: returning early would read `line_out`
    // with the copies in flight.
    //
    // So: queued when there is somewhere safe to queue into, waited for
    // otherwise
    for chunk in copies.chunks(MAX_BATCH) {
        let sent = if deferred {
            super::blackwell::boot::ce_submit_batch_to_vram(&t.bar0, chunk)
        } else {
            super::blackwell::boot::ce_copy_batch(&t.bar0, chunk)
        };
        if sent {
            ok += chunk.len() as u64;
        } else {
            refused += chunk.len() as u64;
            REFUSED_ENGINE.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
    }
    (ok, refused)
}

/// Rectangles this driver would not put on the screen, by reason.
///
/// A refusal is a piece of the screen never written, with the damage already
/// counted as paid above. The compositor redraws the whole frame when it sees
/// one
static REFUSED_BOUNDS: AtomicU64 = AtomicU64::new(0);
static REFUSED_ENGINE: AtomicU64 = AtomicU64::new(0);

/// One line for the shell: rectangles off the surface, and batches the engine
/// would not take
pub fn refusals() -> alloc::string::String {
    alloc::format!(
        "off-surface {}, engine {}",
        REFUSED_BOUNDS.load(Ordering::Relaxed),
        REFUSED_ENGINE.load(Ordering::Relaxed),
    )
}

/// Is there a backdrop buffer for the effects pass to read?
///
/// False under `nobackdrop`, and when video memory would not give up the eight
/// megabytes. With a backdrop, what lies under an effect window goes only
/// there; without one, the pass reads the surface it writes and the region is
/// laid on the screen first
pub fn backdrop_live() -> bool {
    super::blur::backdrop_surface().is_some()
}

/// Map whatever the effects pass is about to read for the first time: step 0 of
/// `compose_frame`, on its own.
///
/// For a caller with work to queue ahead of the frame, the windows' uploads
/// into their copies (`mayland::render::texture`), which must not be on the
/// engine while a mapping's TLB invalidate waits for the MMU
pub fn map_effect_windows(windows: &[super::blur::Window]) -> bool {
    let Some(bar0) = console_bar0() else { return false };
    super::blackwell::boot::effects_map(&bar0, windows)
}

/// Where the shell below the windows is, for the compositor to fold those
/// layers into (`dev/mayland-below-windows.md`): `(vram, pitch in bytes)`, or
/// `None` when the effects pass has no image for it
pub fn base_surface() -> Option<(u64, u32)> {
    super::blur::base_surface().map(|s| (s.vram, s.pitch))
}

/// Map the surfaces about to be folded into the shell below the windows. Its
/// own step like `map_effect_windows`: called before the frame queues anything
/// on an engine, while a TLB invalidate costs microseconds
pub fn base_map(layers: &[super::blur::Window]) -> bool {
    let Some(bar0) = console_bar0() else { return false };
    super::blackwell::boot::effects_map(&bar0, layers)
}

/// Bring the shell below the windows up to date (`blur::submit_base`) and wait
/// for it. After whatever the copy engine was last given (the layers' copies,
/// uploaded just before) and before the frame's own copies read it. Only on a
/// frame where something below the windows changed; the wait is a few hundred
/// microseconds
pub fn base_compose(
    layers: &[super::blur::Window],
    ops: &[super::blur::BaseOp],
    blur: Option<f32>,
) -> Result<(), &'static str> {
    let Some(bar0) = console_bar0() else { return Err("no card") };
    // The launch slots this chain fills are the ones the last frame's pass
    // used. A frame whose pass had no cursor batch behind it gives no proof
    // that pass finished, and rewriting a slot the engine is reading is a fault
    // in the engine
    if !super::blackwell::boot::compute_wait_last(200_000) {
        return Err("the compute engine is still busy with the last frame");
    }
    let after = super::blackwell::boot::ce_after();
    super::blackwell::boot::effects_base(&bar0, layers, ops, blur, after)?;
    if !super::blackwell::boot::compute_wait_last(200_000) {
        super::blur::forget_blurred();
        return Err("the chain below the windows did not finish");
    }
    Ok(())
}

pub fn compose_frame(
    backdrop: &[ScreenBlit],
    into_backdrop: &[ScreenBlit],
    windows: &[super::blur::Window],
    cursor: &[ScreenBlit],
) -> (u64, u64) {
    let guard = CONSOLE.lock();
    let Some(t) = guard.as_ref() else { return (0, backdrop.len() as u64) };
    let (bar0, surface) = (t.bar0.clone(), t.surface);
    drop(guard);

    use super::gsp_common::chan::MAX_BATCH;
    let (mut ok, mut refused) = (0u64, 0u64);
    // Each step timed, so a slow one says which it was (`SLOW_COMPOSE_US`)
    let sw = crate::kcore::time::Stopwatch::start();
    let mut at = [0u64; 5];

    // 0. anything the compute engine will read for the first time. Before the
    //    first submission: a mapping ends in a TLB invalidate that waits on the
    //    MMU, idle only while the engines are.
    //
    //    A window the pass has not mapped has pixels at an address resolving to nothing, and a blur launched over it faults inside the engine. The cursor batch below would then wait on a fence the compute channel never releases, and the next frame's wait times out
    let mapped = super::blackwell::boot::effects_map(&bar0, windows);
    at[0] = sw.elapsed_us();

    // 1. the backdrop. Past the batch limit the list is split and the chunks
    //    wait for each other; the fence that matters is the last
    let copies = to_copies(backdrop, &surface, &mut refused);
    let mut after = None;
    for chunk in copies.chunks(MAX_BATCH) {
        match super::blackwell::boot::ce_submit_batch_after(&bar0, chunk, None) {
            Some(f) => {
                ok += chunk.len() as u64;
                after = Some(f);
            }
            None => refused += chunk.len() as u64,
        }
    }
    at[1] = sw.elapsed_us();

    // 1b. and the same rectangles into the backdrop buffer, so the pass has
    // something to read that is not the surface it writes.
    //
    //     Ordered after step 1 and before the compose, on the same channel, so it needs no fence of its own.
    //
    //     What lies wholly under an effect window is in this list and not in step 1: `composite` writes every pixel of its grid over that area. What is only partly under stays in both: one wasted copy costs less than the rectangles a split would
    if let Some(dst) = super::blur::backdrop_surface() {
        let mut lost = 0u64;
        let into = to_copies(into_backdrop, &dst, &mut lost);
        for chunk in into.chunks(MAX_BATCH) {
            if let Some(f) = super::blackwell::boot::ce_submit_batch_after(&bar0, chunk, after) {
                after = Some(f);
            }
        }
        // Not added to `refused`: the rectangle is not missing from the screen,
        // and the compositor would redraw a correct frame
        BACKDROP_REFUSED.fetch_add(lost, Ordering::Relaxed);
    }
    at[2] = sw.elapsed_us();

    // 2. the blur and the blend, once the backdrop is down. The surface the
    //    pass writes into alternates with every flip, so it is pointed at the
    //    current one first.
    //
    //    A surface the pass was never given would leave it writing into the last one, which is on the screen, and nothing repaints that. So the frame goes out with the windows copied
    let composed = if mapped
        && super::blackwell::boot::effects_retarget(surface.vram, surface.pitch)
    {
        super::blackwell::boot::effects_submit(
            &bar0, (surface.width, surface.height), windows, after)
    } else {
        // The windows were kept out of the copy batch for this pass to draw,
        // and it will not, so they count as refused: the caller redraws the
        // whole frame and stops routing anything through the compute engine
        refused += windows.len() as u64;
        None
    };
    at[3] = sw.elapsed_us();

    // 3. the cursor, once the blend is done. If the compose was refused there
    //    is no fence to wait for and the cursor still goes on
    let cursor_copies = to_copies(cursor, &surface, &mut refused);
    for chunk in cursor_copies.chunks(MAX_BATCH) {
        match super::blackwell::boot::ce_submit_batch_after(&bar0, chunk, composed) {
            Some(_) => ok += chunk.len() as u64,
            None => refused += chunk.len() as u64,
        }
    }
    at[4] = sw.elapsed_us();
    if at[4] >= SLOW_COMPOSE_US {
        crate::kdebug!(
            "[nvidia] handing a frame with effects to the engines took {} us: mapping {}, backdrop {} \
             ({} copies), into the backdrop {} ({}), the pass {} ({} windows), cursor {}",
            at[4], at[0], at[1] - at[0], backdrop.len(), at[2] - at[1], into_backdrop.len(),
            at[3] - at[2], windows.len(), at[4] - at[3]);
    }
    (ok, refused)
}

/// Handing a frame to the engines is queueing, microseconds; past this the
/// driver says which step it stood in
const SLOW_COMPOSE_US: u64 = 2_000;

/// Turn screen blits into engine copies, refusing anything off the surface
fn to_copies(
    blits: &[ScreenBlit],
    surface: &Surface,
    refused: &mut u64,
) -> Vec<super::gsp_common::chan::Copy2d> {
    use super::gsp_common::chan::Copy2d;
    let mut copies = Vec::with_capacity(blits.len());
    for b in blits {
        if b.w == 0 || b.h == 0 || b.x + b.w > surface.width || b.y + b.h > surface.height {
            *refused += 1;
            continue;
        }
        copies.push(Copy2d {
            src: b.src_phys,
            dst: surface.vram + b.y as u64 * surface.pitch as u64 + b.x as u64 * 4,
            pitch_in: b.src_pitch,
            pitch_out: surface.pitch,
            line_len: b.w * 4,
            lines: b.h,
            src_sysmem: !b.src_vram,
            dst_sysmem: false,
        });
    }
    copies
}

// The compositor's copies of windows

// A window the card draws from its own memory instead of its client's
// (`dev/mayland-architecture.md`, "Window pixels the GPU reads live in video
// memory"; `dev/gpu-memory-compute.md`, 3.10). The compositor decides what goes
// into a copy and when; this is the memory and the engine work.

/// Who the allocator says a copy belongs to, in `nvidia vram`
const TEXTURE_OWNER: &str = "mayland window copy";

/// One rectangle of a client's buffer into the compositor's copy of it, for
/// `texture_upload`. The copy has the client's stride, so one offset and one
/// pitch serve both ends
#[derive(Clone, Copy)]
pub struct TextureCopy {
    /// The rectangle's first pixel in the client's buffer: system memory
    pub src_phys: u64,
    /// The copy it goes into: the start of its block, as `texture_alloc` gave
    /// it
    pub texture: u64,
    /// Where in that block the rectangle starts, in bytes
    pub offset: u64,
    /// Bytes from one row to the next, at both ends
    pub pitch: u32,
    /// In pixels and rows
    pub w: u32,
    pub h: u32,
}

/// One rectangle of a window in video memory, a client's own buffer there
/// (`dev/gpu-window-buffers.md`), into system memory the processor reads, for
/// `texture_download`
#[derive(Clone, Copy)]
pub struct TextureDownload {
    /// The rectangle's first pixel, in video memory
    pub src_vram: u64,
    /// Where it lands: physically contiguous system memory
    pub dst_phys: u64,
    /// Bytes from one row to the next, in the source and in the destination
    pub src_pitch: u32,
    pub dst_pitch: u32,
    /// In pixels and rows
    pub w: u32,
    pub h: u32,
}

/// Video memory for one window's copy. The allocator picks the page and the
/// padding (`vram::page_for`, 3.8), so a full-screen window is a block of
/// 2 MiB pages
pub fn texture_alloc(bytes: u64) -> Option<u64> {
    vram_scratch(bytes, TEXTURE_OWNER)
}

/// Give a copy back: out of the effects pass's address space first, then to
/// the allocator. The caller has waited for the frame that last read it
pub fn texture_free(texture: u64) {
    let Some(bytes) = super::gsp_common::vram::block_size(texture) else { return };
    if let Some(bar0) = console_bar0() {
        super::blackwell::boot::effects_unbind(&bar0, texture, bytes, super::gmmu::Aperture::Vram);
    }
    super::gsp_common::vram::free(texture);
}

/// Copy rectangles of clients' buffers into their copies, queued ahead of the
/// frame that reads them. Returns (copied, refused).
///
/// Not waited for: they go on the channel of the frame's own copies, which runs
/// in order, and the effects pass reads only after that channel's fence.
///
/// A rectangle landing outside its copy's block is refused. The engine takes an
/// address at its word, and the memory next to a copy is somebody else's: a
/// scanout surface, or GSP-RM's own
pub fn texture_upload(ups: &[TextureCopy]) -> (u64, u64) {
    use super::gsp_common::chan::{Copy2d, MAX_BATCH};
    let Some(bar0) = console_bar0() else { return (0, ups.len() as u64) };
    let mut copies: Vec<Copy2d> = Vec::with_capacity(ups.len());
    let mut refused = 0u64;
    for u in ups {
        let block = super::gsp_common::vram::block_size(u.texture).unwrap_or(0);
        let end = (u.h as u64)
            .checked_sub(1)
            .and_then(|rows| rows.checked_mul(u.pitch as u64))
            .and_then(|b| b.checked_add(u.w as u64 * 4))
            .and_then(|b| b.checked_add(u.offset));
        if u.w == 0 || u.h == 0 || (u.w as u64) * 4 > u.pitch as u64
            || !matches!(end, Some(e) if e <= block)
        {
            refused += 1;
            continue;
        }
        copies.push(Copy2d {
            src: u.src_phys,
            dst: u.texture + u.offset,
            pitch_in: u.pitch,
            pitch_out: u.pitch,
            line_len: u.w * 4,
            lines: u.h,
            src_sysmem: true,
            dst_sysmem: false,
        });
    }
    let mut ok = 0u64;
    for chunk in copies.chunks(MAX_BATCH) {
        if super::blackwell::boot::ce_submit_batch_after(&bar0, chunk, None).is_some() {
            ok += chunk.len() as u64;
        } else {
            refused += chunk.len() as u64;
        }
    }
    (ok, refused)
}

/// Copy rectangles of windows in video memory into system memory the processor
/// is about to read, and wait for them. Returns (copied, refused).
///
/// The other direction from `texture_upload`, and waited for: the processor
/// reads the destination when this returns (`dev/gpu-window-buffers.md`,
/// dma-buf's `begin_cpu_access`). A rectangle whose source is not inside one
/// block the allocator handed out is refused: the memory beside a window's
/// buffer is somebody else's
pub fn texture_download(downs: &[TextureDownload]) -> (u64, u64) {
    use super::gsp_common::chan::{Copy2d, MAX_BATCH};
    let Some(bar0) = console_bar0() else { return (0, downs.len() as u64) };
    let mut copies: Vec<Copy2d> = Vec::with_capacity(downs.len());
    let mut refused = 0u64;
    for d in downs {
        let row = d.w as u64 * 4;
        let end = (d.h as u64)
            .checked_sub(1)
            .and_then(|rows| rows.checked_mul(d.src_pitch as u64))
            .and_then(|b| b.checked_add(row))
            .and_then(|b| b.checked_add(d.src_vram));
        let inside = match (super::gsp_common::vram::block_containing(d.src_vram), end) {
            (Some((base, size)), Some(e)) => e <= base + size,
            _ => false,
        };
        if d.w == 0 || d.h == 0 || row > d.src_pitch as u64 || row > d.dst_pitch as u64 || !inside {
            refused += 1;
            continue;
        }
        copies.push(Copy2d {
            src: d.src_vram,
            dst: d.dst_phys,
            pitch_in: d.src_pitch,
            pitch_out: d.dst_pitch,
            line_len: d.w * 4,
            lines: d.h,
            src_sysmem: false,
            dst_sysmem: true,
        });
    }
    let mut ok = 0u64;
    for chunk in copies.chunks(MAX_BATCH) {
        if super::blackwell::boot::ce_copy_batch(&bar0, chunk) {
            ok += chunk.len() as u64;
        } else {
            refused += chunk.len() as u64;
        }
    }
    (ok, refused)
}

/// Wait out the frame the engine was last given.
///
/// `blit_batch_to_screen` returns while the engine reads, so this makes
/// touching a source buffer safe. Called at the top of the next frame: the
/// engine has had a whole raster period, and the wait should cost nothing.
///
/// Returns the microseconds it cost, and whether it finished. A number above
/// zero means the frame is not keeping up
pub fn wait_last_blit(timeout_us: u64) -> (u64, bool) {
    let sw = crate::kcore::time::Stopwatch::start();
    // Both engines: a frame with effects ends on the compute channel, and its
    // buffers are the ones about to be rewritten
    let copied = super::blackwell::boot::ce_wait_last(timeout_us);
    let composed = super::blackwell::boot::compute_wait_last(timeout_us);
    (sw.elapsed_us(), copied && composed)
}

/// The surface being drawn into: the back buffer when there are two
pub fn console_surface() -> Option<Surface> {
    CONSOLE.lock().as_ref().map(|t| t.surface)
}

pub fn console_bar0() -> Option<MmioRegion> {
    CONSOLE.lock().as_ref().map(|t| t.bar0.clone())
}

// The cursor plane

// The head scans the pointer out of its own small surface, composited over
// everything the windows produced. Moving the mouse costs no composition,
// copies or wait for the raster boundary, and the pointer is not a frame
// behind.
//
// Two independent halves. The image is video memory the head re-fetches every
// frame, so changing it is a write, no UPDATE, as for the window's own surface.
// The position is a PIO channel, so moving it is two stores into BAR0, at any
// point in a frame, from the input path, with no lock the compositor holds.

/// Where inside the image the pointer points.
///
/// The head's `HOT_SPOT` fields stay zero (see `arm_cursor`) and this is
/// subtracted here, keeping a client's `set_cursor` off the core channel
static CURSOR_HOT: spin::Mutex<(i32, i32)> = spin::Mutex::new((0, 0));

/// Is there a cursor plane to drive? False means the compositor draws its own
pub fn cursor_available() -> bool {
    super::blackwell::boot::live_cursor().is_some()
}

/// Put the pointer at `(x, y)` in screen coordinates.
///
/// Cheap enough for every motion event: no allocation, no lock the compositor
/// holds, no frame. False if there is no plane or the channel did not take the
/// method; the pointer then stays until the next event
pub fn cursor_move(x: i32, y: i32) -> bool {
    let Some(c) = super::blackwell::boot::live_cursor() else {
        return false;
    };
    let Some(bar0) = console_bar0() else {
        return false;
    };
    let (hx, hy) = *CURSOR_HOT.lock();
    super::gsp_common::modeset::move_to(&bar0, c.head, x - hx, y - hy)
}

/// Upload a new cursor image.
///
/// `px` is `w * h` pixels in the surface's own order, the 0xAARRGGBB the
/// console and the compositor use. Anything smaller than the plane's size is
/// padded with transparent pixels: the plane takes four sizes.
///
/// Costs one PRAMIN copy of the whole plane, so it belongs on a cursor change,
/// not a move
pub fn cursor_set_image(px: &[u32], w: u32, h: u32, hot_x: i32, hot_y: i32) -> bool {
    let Some(c) = super::blackwell::boot::live_cursor() else {
        return false;
    };
    let Some(bar0) = console_bar0() else {
        return false;
    };
    if w > c.size || h > c.size || px.len() < (w as usize) * (h as usize) {
        return false;
    }

    // Built whole and copied once. A partial copy leaves the previous pointer's
    // tail on the panel meanwhile, and the whole plane is 16 KiB
    let side = c.size as usize;
    let mut img = alloc::vec![0u8; side * side * 4];
    for y in 0..h as usize {
        let row = y * side * 4;
        for x in 0..w as usize {
            let p = px[y * w as usize + x].to_le_bytes();
            let o = row + x * 4;
            img[o..o + 4].copy_from_slice(&p);
        }
    }
    super::pramin::copy_to_vram(&bar0, c.vram, &img);
    let _ = super::pramin::flush_fb(&bar0);
    *CURSOR_HOT.lock() = (hot_x, hot_y);
    true
}

/// Take the pointer off the screen without tearing the plane down.
///
/// Every driver hides a cursor it owns by moving it past the raster; clearing
/// the enable bit takes a core update, a modeset-shaped risk for a null cursor
pub fn cursor_hide() -> bool {
    let Some(c) = super::blackwell::boot::live_cursor() else {
        return false;
    };
    let Some(bar0) = console_bar0() else {
        return false;
    };
    let off = -(c.size as i32) - 1;
    super::gsp_common::modeset::move_to(&bar0, c.head, off, off)
}

// The card, as an implementation of the display contract

/// Everything above, behind `drivers::gpu::scanout::Scanout`.
///
/// One-to-one: every method is the call it was, and this file's ordering rules
/// (`FLIP` before `CONSOLE`, the fence at the top of the frame) stand.
/// `mayland::render` names no vendor, and the processor's path answers the same
/// questions.
///
/// `bar0` is not in the contract. What needs it takes it from `console_bar0()`
pub struct NvScanout;

impl crate::drivers::gpu::scanout::Scanout for NvScanout {
    fn name(&self) -> &'static str { "the card" }

    fn output(&self, out: usize) -> Option<crate::drivers::gpu::scanout::OutputInfo> {
        if out != 0 { return None; }
        let t = console_target()?;
        Some(crate::drivers::gpu::scanout::OutputInfo {
            width: t.0,
            height: t.1,
            refresh_mhz: refresh_mhz(),
        })
    }

    fn wait_next_frame(&self, _out: usize, timeout_us: u64) -> bool {
        match console_bar0() {
            Some(bar0) => wait_next_frame(&bar0, timeout_us),
            None => false,
        }
    }

    fn raster_pos(&self, _out: usize) -> Option<(u16, u16)> {
        raster_pos(&console_bar0()?)
    }

    fn raster_timing(&self, _out: usize)
        -> Option<crate::drivers::gpu::scanout::RasterTiming>
    {
        raster_timing(&console_bar0()?)
    }

    fn draw_slot(&self, _out: usize) -> Option<usize> { draw_slot() }

    fn blit_batch(&self, _out: usize, blits: &[ScreenBlit]) -> (u64, u64) {
        if blits.is_empty() { return (0, 0); }
        blit_batch_to_screen(blits)
    }

    fn fence_wait(&self, timeout_us: u64) -> (u64, bool) { wait_last_blit(timeout_us) }

    fn unstick(&self) { super::blackwell::boot::unstick_engines(); }

    fn compute_generation(&self) -> Option<u32> {
        if super::blackwell::boot::compute_ready() { super::blackwell::boot::compute_epoch() } else { None }
    }

    fn unbind_range(&self, phys: u64, bytes: u64) -> usize {
        // A client's pool, system memory; a copy in video memory is unbound by
        // `texture_free`
        super::with_gb20x(|d| super::blackwell::boot::effects_unbind(
            &d.bar0, phys, bytes, super::gmmu::Aperture::Sysmem))
            .unwrap_or(0)
    }

    fn texture_alloc(&self, bytes: u64) -> Option<u64> { texture_alloc(bytes) }
    fn texture_free(&self, texture: u64) { texture_free(texture) }
    fn texture_upload(&self, ups: &[TextureCopy]) -> (u64, u64) { texture_upload(ups) }
    fn texture_download(&self, downs: &[TextureDownload]) -> (u64, u64) { texture_download(downs) }

    fn flipping(&self, _out: usize) -> bool { flipping() }
    fn flip(&self, _out: usize) -> bool { flip() }
    fn should_flip(&self, _out: usize) -> bool { should_flip() }
    fn enable_flip(&self, _out: usize) -> Result<(), &'static str> { enable_flip() }
    fn disable_flip(&self, _out: usize) { disable_flip() }
    fn flip_counts(&self, _out: usize) -> (u64, u64) { flip_counts() }
    fn refusals(&self) -> alloc::string::String { refusals() }

    fn flip_semaphore_active(&self) -> bool { flip_semaphore_active() }
    fn give_up_flip_semaphore(&self) { give_up_flip_semaphore() }
    fn disable_flip_semaphore(&self) { disable_flip_semaphore() }

    fn composes(&self) -> bool { super::blur::ready() }

    fn cursor_available(&self) -> bool { cursor_available() }
    fn cursor_move(&self, x: i32, y: i32) -> bool { cursor_move(x, y) }
    fn cursor_set_image(&self, px: &[u32], w: u32, h: u32, hx: i32, hy: i32) -> bool {
        cursor_set_image(px, w, h, hx, hy)
    }
    fn cursor_hide(&self) -> bool { cursor_hide() }
    fn cursor_release(&self) { super::blackwell::boot::clear_live_cursor(); }

    fn suspend_console_flush(&self) { suspend_console_flush(); }
    fn resume_console_flush(&self) -> bool { resume_console_flush() }
    fn force_console_flush_hook(&self) { force_console_flush_hook(); }
    fn force_front_surface(&self) { force_front_surface(); }
}

/// The live scanout's size, for `output` above
fn console_target() -> Option<(u32, u32)> {
    let c = CONSOLE.lock();
    let t = c.as_ref()?;
    Some((t.surface.width, t.surface.height))
}
