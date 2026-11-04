// The compositor
//
// The frame loop and everything it needs to put pixels on a screen. The only
// layer touching the card: below it are surfaces and rectangles, above it a
// display channel, a copy engine and a panel.
//
// A frame is the same five steps on either path:
//
//      1  wait for last frame's engines, and hand the buffers back
//      2  drain input, tick the animations, pump the console
//      3  ask the server for the scene, flattened
//      4  work out what changed and copy exactly that
//      5  say the frame happened
//
//      damage   what changed, and what therefore has to be copied
//      effects  how a window is drawn: borders, corners, opacity, blur
//      outline  a window's corner and border on the processor, as tables
//      cursor   the one thing drawn over a finished frame, composed by hand
//      compose  a damage rectangle blended by the processor, for the pixels
//               a see-through surface covers
//      present  the frame itself, submitted to the copy engine
//      texture  every window's copy in the card's memory, sent only what the
//               client changed; the GPU reads windows from there
//
// Two paths: with a card, frames are paced by the raster and copied by the
// engine. With no card, they are paced by the scheduler tick and copied by the
// processor into the firmware's framebuffer. `bring_up` is the shared tail of
// both starts.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;
use crate::mayland::server;
use crate::drivers::gpu::nvidia::scanout;

pub mod base;
pub mod compose;
pub mod cursor;
pub mod damage;
pub mod effects;
pub mod outline;
pub mod phases;
pub mod present;
pub mod texture;

// What the rest of the system asks of the compositor
pub use damage::{force_full_frame, Rect};
pub use effects::{border, effects, set_border, set_param, Border, Effects, Fx, BORDER, EFFECTS};

use effects::effect_windows;
use present::{present, wait_for_engines};

static RUNNING: AtomicBool = AtomicBool::new(false);

static SPAWNED: AtomicBool = AtomicBool::new(false);

/// Whether a frame has been put on the screen since compositing started
///
/// The console stops painting when this becomes true, not when the frame loop
/// is spawned: otherwise a machine with a black screen has a running, invisible
/// console
static PRESENTED: AtomicBool = AtomicBool::new(false);

/// Milliseconds, from the clock of the compositor's own animations.
///
/// `layout::now_us` reads the TSC. A timer tick is 4 ms: at 240 Hz the frame is
/// 4166 us, so frames counted in ticks would carry equal timestamps and then
/// jump, and a client animating off frame-callback differences would stutter on
/// even frames
fn frame_time_ms() -> u32 {
    (crate::mayland::shell::layout::now_us() / 1000) as u32
}

/// How long a frame waits for its own copies before arming the flip.
///
/// A backstop for an engine that stopped answering, not a frame period. Two
/// frames at 240 Hz, half a frame at 60: short enough that a wedged engine does
/// not visibly freeze the desktop, long enough that a full-screen batch (about
/// a millisecond) never reaches it. On a timeout the flip is armed anyway: a
/// torn frame is better than a still picture
const FENCE_BUDGET_US: u64 = 8_000;

/// Someone asked compositing to stop, as opposed to it having fallen over
///
/// `compositor_thread` watches for a stopped desktop and restarts it. Without
/// this flag the watch would undo `mayland stop` within a second, removing the
/// way out of a wedged desktop
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

/// A stop asked for from inside the frame loop, carried out elsewhere
///
/// `stop` waits for the frame in flight, so the frame loop cannot call it. The
/// watcher in `mayland::compositor_thread`, a different thread, does it
static STOP_WANTED: AtomicBool = AtomicBool::new(false);

static THREAD_PID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Default)]
pub struct Stats {
    /// Completed frames.
    pub frames: u64,
    /// Total surface copies.
    pub blits: u64,
    /// Failed or out-of-bounds copies.
    pub dropped: u64,
    /// Raster wait timeouts.
    pub stalls: u64,
    /// Last and worst frame copy time in microseconds.
    pub last_us: u64,
    pub worst_us: u64,
    /// Time spent waiting for previous frame copies, in microseconds.
    pub wait_us: u64,
    pub wait_worst_us: u64,
    /// The handover alone (fence wait, flip push and kick), in microseconds
    pub flip_us: u64,
    pub flip_worst_us: u64,
    /// Frames where previous copies exceeded the frame budget.
    pub fence_timeouts: u64,
    /// Time between frame boundaries in microseconds.
    pub period_us: u64,
    /// Damaged rectangles redrawn in the last frame.
    pub last_rects: usize,
    /// Pixels copied in the last frame and total.
    pub last_px: u64,
    /// Pixels copied into the backdrop buffer, not onto the screen
    ///
    /// Counted apart as a different errand and cost: in `last_px` they would
    /// hide that what lies under an effect window is no longer copied to the
    /// screen
    pub last_backdrop_px: u64,
    pub backdrop_pixels: u64,
    pub pixels: u64,
    /// Raster line when frame copies started and ended.
    pub line_in: u16,
    pub line_out: u16,
    /// Worst observed raster end line.
    pub line_out_worst: u16,
    /// Worst observed raster start line.
    pub line_in_worst: u16,
    /// Frames that missed the blanking interval.
    pub late: u64,
    /// Frames that exceeded the blanking interval and may tear.
    pub overran: u64,
}

static STATS: Mutex<Stats> = Mutex::new(Stats {
    frames: 0, blits: 0, dropped: 0, stalls: 0, last_us: 0, worst_us: 0,
    wait_us: 0, wait_worst_us: 0, flip_us: 0, flip_worst_us: 0,
    fence_timeouts: 0, period_us: 0,
    last_rects: 0, last_px: 0, pixels: 0,
    last_backdrop_px: 0, backdrop_pixels: 0,
    line_in: 0, line_out: 0, line_out_worst: 0, line_in_worst: 0, late: 0,
    overran: 0,
});

/// Cached raster timing information.
static RASTER: Mutex<crate::drivers::gpu::scanout::RasterTiming> =
    Mutex::new(crate::drivers::gpu::scanout::RasterTiming {
        vtotal: 0, htotal: 0, vblanke: 0, vblanks: 0, pixel_clock_hz: 0,
    });

pub fn raster() -> crate::drivers::gpu::scanout::RasterTiming {
    *RASTER.lock()
}

pub fn stats() -> Stats { *STATS.lock() }

pub fn running() -> bool { RUNNING.load(Ordering::Relaxed) }

/// Someone typed `mayland stop`, as against a watchdog giving up
///
/// A watchdog stop means nothing was drawing the screen, so the console gets it
/// back, and mikuD then restarts the greeter, needing a compositor. An
/// operator's stop holds until asked otherwise. The two are separate flags so
/// the watcher can start a compositor after the first
static STOP_BY_OPERATOR: AtomicBool = AtomicBool::new(false);

/// Whether compositing was stopped by a person, not given up on
pub fn stopped_by_request() -> bool { STOP_BY_OPERATOR.load(Ordering::Relaxed) }

/// Stop, and hold: the screen stays with the console until asked otherwise. The
/// shell's `mayland stop` only
pub fn stop_by_operator() {
    STOP_BY_OPERATOR.store(true, Ordering::SeqCst);
    stop();
}

/// Ask, from inside the frame loop, for compositing to be stopped
///
/// The screen is left with nobody drawing it and the console cannot be reached:
/// the one way out needing no typing into an unpainted screen
pub fn request_stop() { STOP_WANTED.store(true, Ordering::SeqCst); }

/// Whether a stop has been asked for from the frame loop, clearing the request
pub fn take_stop_request() -> bool { STOP_WANTED.swap(false, Ordering::SeqCst) }

/// The screen changes hands, once there is something on it to change to
///
/// Called from the frame loop with what the frame copied. Zero means the
/// compositor drew nothing (no wallpaper, no clients, a card refusing a blit),
/// and blanking the console for that would lose the machine's only interface
fn took_screen(blits: u64) {
    if blits == 0 || PRESENTED.swap(true, Ordering::SeqCst) {
        return;
    }
    // From here the screen belongs to the compositor; two writers of the same
    // pixels would overwrite each other.
    //
    // The text carries on: `set_headless` stops the painting and not the grid,
    // so `/bin/mayterm` can take the console through `may_console` and show it
    // in a window. The kernel keeps the text, and a program keeps the pixels
    crate::io::console::set_headless(true);
    scanout::suspend_console_flush();

    // And repaint everything: until now there were two writers of the same
    // video memory.
    //
    // Boot messages reach the panel until a frame replaces them, so the frame
    // triggering this is composed with the console still live. On the card that
    // frame's copies are submitted, not waited for, and the console flushes
    // from the keyboard thread through PRAMIN: a line printed in that window
    // lands on the composed screen, in the surface about to flip, and nothing
    // damages those pixels again. On the card only: with no card there is no
    // flush hook.
    //
    // `FORCE_FULL` means something outside the compositor has drawn on the
    // screen. It repaints both surfaces: the forced frame pays one in full and
    // hands the whole screen to the other as debt
    force_full_frame();
    crate::serial_println!("[mayland] a frame is on the screen - the console stops painting");
}

/// True when no card is scanning out and the processor writes frames into the
/// firmware's framebuffer
static SOFTWARE: AtomicBool = AtomicBool::new(false);

/// Who is putting frames on the screen
///
/// Chosen once, at `start`, fixed while compositing runs. Both answers
/// implement `drivers::gpu::scanout::Scanout`, so everything below asks its
/// questions without naming a vendor.
///
/// The two frame loops below are still two. The hardware loop's ordering (where
/// the fence is waited for, that a flip needs `redrew` and not `blits`, that a
/// frame starting behind the beam is given up) can be exercised only on the
/// card
static BACKEND: Mutex<Option<alloc::sync::Arc<dyn crate::drivers::gpu::scanout::Scanout>>> =
    Mutex::new(None);

/// The presenter, or `None` before `start` has chosen one
pub fn backend() -> Option<alloc::sync::Arc<dyn crate::drivers::gpu::scanout::Scanout>> {
    BACKEND.lock().clone()
}

/// Flips the window channel would not take, since the last it did.
///
/// The run of consecutive refusals (`scanout::flip_counts` keeps the total):
/// one is a frame nobody sees, four in a row a screen that will never update
static FLIPS_REFUSED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// A frame was composed into the back surface and its flip was held back
///
/// `should_flip` delays a flip whose copies have not landed, for the next frame
/// to show, which holds only for a next frame redrawing something: the flip is
/// gated on `redrew > 0`. A burst ending on a held-back frame (`clear`, a
/// screenful and then silence) would leave the finished picture in the surface
/// nobody scans. So a held-back flip is owed, and a later frame arms it whether
/// or not it drew
static FLIP_OWED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Consecutive frames whose flip was held because the copies had not landed
static UNFINISHED_HOLDS: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// Consecutive frames the gate would not let hand anything over
///
/// `should_flip` holds a flip back while the copy engine works, and lets one
/// through every `max_skipped_flips` frames, 16 at this panel's refresh. With
/// blur on the engine is busy every frame (the backdrop under an effect window
/// is repainted regardless), so the gate would let one frame in seventeen reach
/// the monitor: thirteen pictures a second on a 240 Hz panel.
///
/// The semaphore exists to save the processor a fence wait, not worth that
/// cost, so it is given up on
static FLIPS_HELD: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);


/// A frame, in scheduler ticks
///
/// The timer runs at 250 Hz (`apic::TIMER_HZ_DEFAULT`), so a tick is 4 ms and
/// four are the 16 ms of a 60 Hz frame. The hardware path waits for the raster;
/// the software path keeps the time itself
const FRAME_TICKS: u64 = 4;

pub fn software() -> bool {
    SOFTWARE.load(Ordering::Relaxed)
}

pub fn start() -> Result<(), &'static str> {
    // Two ways to the screen. The card, when brought up: its copy engine moves
    // the frame and its raster says when. The plain framebuffer otherwise: the
    // processor writes the rows, with nothing to synchronise against
    let sw = scanout::console_bar0().is_none();
    {
        use alloc::sync::Arc;
        use crate::drivers::gpu::scanout::Scanout;
        let b: Arc<dyn Scanout> = if sw {
            Arc::new(crate::drivers::gpu::softscan::SoftScanout)
        } else {
            Arc::new(scanout::NvScanout)
        };
        crate::serial_println!("[mayland] presenting with {}", b.name());
        *BACKEND.lock() = Some(b);
    }
    if sw && crate::io::console::fb_size().is_none() {
        return Err("no screen at all, neither a scanning card nor a framebuffer");
    }
    SOFTWARE.store(sw, Ordering::SeqCst);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Err("the compositor is already running");
    }
    // What stopped it last is no longer what is asked for
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    STOP_WANTED.store(false, Ordering::SeqCst);
    STOP_BY_OPERATOR.store(false, Ordering::SeqCst);
    PRESENTED.store(false, Ordering::SeqCst);
    if sw {
        crate::serial_println!(
            "[mayland] no card is scanning out, composing into the framebuffer \
             with the processor");
        // The screen itself, not the console's shadow of it, which reaches the
        // screen only when the console draws (`console::panel`)
        match crate::io::console::panel() {
            Some((ptr, len, cfg)) => {
                crate::drivers::gpu::softscan::set_panel(
                    Some(crate::io::console::Surface { ptr, len, cfg }));
            }
            None => {
                RUNNING.store(false, Ordering::SeqCst);
                return Err("there is no framebuffer to compose into");
            }
        }
        claim_screen();
        bring_up();
        return Ok(());
    }
    // Take the screen before drawing on it. The scanout surface holds what the
    // last owner (firmware, another operating system, an earlier boot) left,
    // and the console repaints only the rectangle it dirtied
    claim_screen();
    // A second surface to compose into, if the framebuffer carve has room. With
    // one, a frame has to be written into the blanking interval and a
    // full-screen repaint never fits
    match scanout::enable_flip() {
        Ok(()) => {
            crate::serial_println!("[mayland] composing into a second surface");
            // The hardware holds the flip back until the copies have landed, as
            // the shipping driver does, so the head does not latch a
            // half-written surface. `noflipsema` on the boot line puts the wait
            // on the processor
            if crate::arch::x86_64::grub::cmdline_flag("noflipsema") {
                scanout::disable_flip_semaphore();
                crate::serial_println!(
                    "[mayland] 'noflipsema' - the frame loop waits for the copy engine itself");
            } else {
                crate::serial_println!(
                    "[mayland] the window's acquire semaphore gates the flip");
            }
        }
        Err(e) => crate::serial_println!(
            "[mayland] one surface only ({}), frames must fit the blanking interval", e),
    }
    // The blur levels, reserved once against the surface's size. A failure
    // costs the effects only: `effect_windows` asks whether the pass is ready
    // before picking
    if let (Some(surface), Some(bar0)) = (scanout::console_surface(), scanout::console_bar0()) {
        crate::drivers::gpu::nvidia::blackwell::boot::blur_init(
            &bar0, surface.width, surface.height, surface.pitch,
            &|line| crate::serial_println!("[mayland] {}", line));
    }
    bring_up();
    Ok(())
}

/// The end of `start`, on either path: the thread, the backdrop, the console
/// and the pointer
fn bring_up() {
    if !SPAWNED.swap(true, Ordering::SeqCst) {
        let pid = crate::sched::spawn_named(compositor_main, "mayland", 1);
        THREAD_PID.store(pid, Ordering::Relaxed);
        crate::serial_println!("[mayland] compositor thread pid={}", pid);
    }
    // Something has to be at the back, or a moving window leaves a trail of
    // itself across the screen
    if let Err(e) = crate::mayland::shell::wallpaper::start_for_output() {
        crate::serial_println!("[mayland] no wallpaper: {}", e);
    }
    // The console keeps painting until there is a frame to replace it; see
    // `took_screen`, which the frame loop calls with what it copied
    if let Some((w, h)) = server::output_size() {
        crate::mayland::input::pointer::start(w as i32, h as i32);
    }
}

pub fn stop() {
    // Before the flag, so the watcher cannot see a stopped compositor and
    // restart it between the two
    STOP_REQUESTED.store(true, Ordering::SeqCst);
    STOP_WANTED.store(false, Ordering::SeqCst);
    PRESENTED.store(false, Ordering::SeqCst);
    RUNNING.store(false, Ordering::SeqCst);
    // Let the frame in flight finish before repainting over it. Two frames at
    // 60 Hz is 33 ms; the flag is checked at the top of every loop, so by then
    // the thread is parked
    crate::sched::sleep(9);
    // Back to drawing into what is being scanned, before anything else paints;
    // otherwise the repaint below lands in a surface nobody sees
    crate::mayland::input::pointer::stop();
    // A plane the head draws does not stop with the compositor, so the last
    // pointer would stay on the console until something moved it
    cursor::hw_hide();
    scanout::disable_flip();
    crate::mayland::shell::wallpaper::stop();
    // The console reaches the panel by itself again, and repaints all of it, or
    // the last composed frame stays for ever. `set_headless(false)` is the
    // repaint
    scanout::resume_console_flush();
    crate::io::console::set_headless(false);
    claim_screen();
    // The thread is parked and will not reap again. What the teardown destroyed
    // waits for an engine with nothing left to finish, and starting twice would
    // leak every pool of the first run
    crate::mayland::server::shm::reap();
    // The same for the windows' copies in video memory, all of them: nothing
    // draws from any now. A GSP-RM unload is refused until this has run
    // (`bringup/unload.rs`)
    texture::release_all();
}

/// Blank the screen and put the console back on it, whole
///
/// One full-screen copy-engine push, about four milliseconds, the path a scroll
/// takes
pub fn claim_screen() {
    // Someone other than the compositor is about to draw on the screen, so the
    // next frame assumes nothing
    force_full_frame();
    if crate::io::console::repaint() {
        crate::io::console::flush();
    }
}

/// Put the console back on the screen without touching anything that could be
/// wedged, for the panic path
///
/// The console is text the kernel keeps and `/bin/mayterm` shows, so this is
/// the three things that put a panic message in front of a person.
///
/// Best effort: a panic can happen with any lock held, so a step that cannot be
/// taken is skipped, not waited for
pub fn emergency() {
    // First and unconditionally: the compositor may be composing into a second
    // surface, and everything drawn from here, the message included, would land
    // in a surface nobody scans
    scanout::force_front_surface();
    if !RUNNING.load(Ordering::Relaxed) {
        return;
    }
    // The frame loop is asked to stop, not waited for: it runs on another
    // processor and may never be scheduled again
    RUNNING.store(false, Ordering::Relaxed);
    STOP_REQUESTED.store(true, Ordering::Relaxed);
    PRESENTED.store(false, Ordering::Relaxed);
    // Reinstall the hook without asking whether there is a target: that takes
    // the target lock, and a lock held by a halted thread is never released
    scanout::force_console_flush_hook();
    // Let the console paint again. This repaints the grid, a screenful of work
    // in a panic, and the right one: the panel holds the compositor's last
    // frame.
    //
    // Try-locked: a console whose lock is held by the thread that died sends
    // the message to the serial port and not the screen, with no hang
    if !crate::io::console::try_set_headless(false) {
        crate::serial_println!("[mayland] panic: the console lock was busy, screen not restored");
    }
}

/// Convert a TSC delta to microseconds
///
/// A delta: dividing the raw TSC by kHz and multiplying by 1000 rounds to the
/// millisecond before it scales
#[inline]
fn tsc_to_us(delta: u64) -> u64 {
    let mhz = (crate::kcore::time::tsc_khz().max(1) / 1000).max(1);
    delta / mhz
}

fn compositor_main() -> ! {
    crate::serial_println!("[mayland] compositor started");
    let mut last_frame_us = 0u64;

    loop {
        if !RUNNING.load(Ordering::Relaxed) {
            crate::sched::sleep(25);
            continue;
        }
        // With no card there is no raster or engine to wait for: the frame is
        // written by the processor and finished on return. So the loop is plain
        // (sleep, take what the clients drew, write it), and what follows about
        // beams and blanking does not apply
        if SOFTWARE.load(Ordering::Relaxed) {
            // The backend keeps the time. With no raster it is a deadline and a
            // carried remainder (`softscan::wait_next_frame`); with one it is
            // the beam
            if let Some(b) = backend() { b.wait_next_frame(0, 0); }
            else { crate::sched::sleep(FRAME_TICKS); }
            // What the kernel's console has to say, offered to whoever draws
            // it: what `may_console` delivers, the grid and which rows moved
            crate::mayland::server::console_pump();
            crate::mayland::shell::wallpaper::pump();
            // Curves before the frame is composed, as `present` snapshots where
            // every window is. On this path it warps, not animates (see
            // `layout::animating`), with the call here so one place walks a
            // curve
            crate::mayland::shell::layout::tick(crate::mayland::shell::layout::now_us());
            let boundary = crate::kcore::time::rdtsc();
            let (blits, dropped, _redrew) = present();
            took_screen(blits);
            // The damage this frame read has been copied by the processor,
            // here, with nothing in flight. Answered on this path too, not by
            // `frame_done`: both paths retire damage in the same place
            server::frame_composed();
            let done = crate::kcore::time::rdtsc();
            // The processor wrote this frame, so the copies are finished when
            // `present` returns and any pool destroyed meanwhile goes back to
            // the allocator
            crate::mayland::server::shm::reap();
            let mut st = STATS.lock();
            st.frames += 1;
            st.blits += blits;
            st.dropped += dropped;
            st.last_us = tsc_to_us(done.saturating_sub(boundary));
            st.worst_us = st.worst_us.max(st.last_us);
            drop(st);

            // The same end to a frame as the hardware path: the keys and mouse
            // the interrupts collected, then the buffers handed back for the
            // clients' next frame
            let now_ms = frame_time_ms();
            pump_input(now_ms);
            // The head's cursor plane, if any, is fed here, not from a frame:
            // an iteration composing nothing still has to notice a client
            // committing a new pointer
            cursor::hw_sync();
            server::frame_done(now_ms);
            server::lock_watchdog(now_ms);
            // Everything this frame had to say is queued; the clients waiting
            // for it are asleep
            server::wake_readers();
            continue;
        }

        let Some(bar0) = scanout::console_bar0() else {
            pump_input(frame_time_ms());
            crate::sched::sleep(25);
            continue;
        };
        // The raster the head is scanning, from its own registers. It changes
        // only with a modeset, so it is read once
        let timing = *RASTER.lock();
        let timing = if timing.valid() {
            timing
        } else {
            let Some(t) = scanout::raster_timing(&bar0) else {
                pump_input(frame_time_ms());
                crate::sched::sleep(25);
                continue;
            };
            *RASTER.lock() = t;
            let mhz = t.refresh_mhz().max(1) as u64;
            crate::serial_println!(
                "[mayland] raster {} lines total, visible {}..{}, {} blanked at the top - {} us to write in at {}.{:03} Hz",
                t.vtotal, t.vblanke + 1, t.vblanks - 1, t.vblanke + 1,
                // From the head's own refresh rate: the number read to decide
                // whether a frame fits in the blanking interval
                (t.vblanke as u64 + 1) * 1_000_000_000 / (t.vtotal.max(1) as u64 * mhz),
                mhz / 1000, mhz % 1000,
            );
            t
        };

        // 50 ms is three frames at 60 Hz and twelve at 240. Missing that many
        // means the head stopped
        if !scanout::wait_next_frame(&bar0, 50_000) {
            STATS.lock().stalls += 1;
            // The path a stalled head takes repeatedly: without this the
            // machine answers nothing
            pump_input(frame_time_ms());
            crate::sched::sleep(1);
            continue;
        }
        let line_in = scanout::raster_pos(&bar0).map(|(v, _)| v).unwrap_or(0);

        // Woke up behind the beam
        //
        // The frame counter ticks at line zero of the raster, inside the
        // vertical blanking interval; the first visible row is `vblanke` lines
        // later. A frame starting on time has that whole interval to write in,
        // one starting late has what is left. Past the last blanked line every
        // copy would land in front of the beam. Nothing has been written yet,
        // so giving the frame up costs nothing.
        //
        // Not with two surfaces: the frame is composed into one nobody scans,
        // and handed over whole
        let flipping = scanout::flipping();
        if !flipping && line_in > timing.vblanke {
            STATS.lock().late += 1;
            pump_input(frame_time_ms());
            continue;
        }
        let boundary = crate::kcore::time::rdtsc();
        // Every step from here is timed into `phases`, so a late frame can say
        // which step
        phases::begin();
        let step = crate::kcore::time::Stopwatch::start();

        // Last frame's copies first: from here the frame reads and writes the
        // memory they were reading
        wait_for_engines();
        phases::add(phases::Phase::Engines, step.elapsed_us());
        let step = crate::kcore::time::Stopwatch::start();

        // The engine has released them, so last frame's buffers go back to
        // their clients and their callbacks fire. Not at the end of the frame
        // composing them: `present` only queues the copies, and a client told
        // its buffer was free then would draw into memory the engine is
        // reading. The damage was answered then, by `frame_composed`
        let now_ms = frame_time_ms();
        server::frame_done(now_ms);
        server::lock_watchdog(now_ms);
        phases::add(phases::Phase::Handback, step.elapsed_us());
        let step = crate::kcore::time::Stopwatch::start();

        // What the console has said since the last frame belongs in this frame,
        // so it is offered before anything is copied
        crate::mayland::server::console_pump();
        crate::mayland::shell::wallpaper::pump();
        phases::add(phases::Phase::Pumps, step.elapsed_us());
        let step = crate::kcore::time::Stopwatch::start();
        // After the engines, as from here the frame writes the memory they
        // read, and after the bail above, which skips a frame when the raster
        // has passed. An animation in wall-clock time survives that
        crate::mayland::shell::layout::tick(crate::mayland::shell::layout::now_us());
        phases::add(phases::Phase::Curves, step.elapsed_us());
        let step = crate::kcore::time::Stopwatch::start();

        let (blits, dropped, redrew) = present();
        // `present` timed its own steps; what is left of it is the damage and
        // the frame's lists
        let in_present = step.elapsed_us();
        let timed: u64 = [phases::Phase::Fences, phases::Phase::Scene, phases::Phase::Copies,
                          phases::Phase::Blend, phases::Phase::Submit]
            .iter().map(|p| phases::get(*p)).sum();
        phases::add(phases::Phase::Compose, in_present.saturating_sub(timed));
        took_screen(blits);
        // The damage this frame read is in the batch the engine is working
        // through, so it is answered. Immediately: between here and the end of
        // the loop (the flip, the input drain) a client can commit, and a
        // rectangle declared then belongs to the next frame. The buffers go
        // back at the top of the next frame, once the batch has finished
        server::frame_composed();
        // Where the raster was when composing ended, read before the handover
        let line_out = scanout::raster_pos(&bar0).map(|(v, _)| v).unwrap_or(0);
        let composed_at = crate::kcore::time::rdtsc();

        // Hand the finished frame to the display engine. It arms at the next
        // frame boundary, so this neither blocks nor needs timing.
        //
        // Only when this frame composed something. A flip puts the other
        // surface on the screen, and a frame that drew nothing has nothing of
        // its own there: that surface holds the frame before last.
        //
        // "Composed something" is rectangles redrawn, not copies submitted: the
        // cursor patch goes on top of every frame, so the copy count is never
        // zero while the pointer is up.
        //
        // With an animating client the two surfaces are never identical: the
        // buffer-age rule repairs the older one by re-reading the client's
        // buffer as it is now. So they hold consecutive frames of the
        // animation, and flipping between them with nothing new composed shows
        // a window flickering between two images.
        //
        // The copies have to land before the flip is armed:
        //
        // `present` submits the frame's copies and returns; the engine is still
        // reading its sources and writing the back surface when control gets
        // here. `flip` arms SET_SURFACE_ADDRESS with MIN_PRESENT_INTERVAL 1,
        // and the hardware swaps at the next frame boundary, with no regard for
        // the copies. At 240 Hz the period is 4166 us, about a full-screen
        // batch, so the head would latch a surface the engine is halfway
        // through, showing stripes of stale content with the new text on top.
        //
        // So the fence is waited for here, before arming, unless the hardware
        // does the waiting: with the window's acquire semaphore armed, the flip
        // is gated on the engine's fence, as in the reference driver.
        //
        // What this frame has to put on the screen: what it drew, or what an
        // earlier frame drew and could not hand over
        let owing = redrew > 0 || FLIP_OWED.load(Ordering::Relaxed);
        // Did the copies land? Asked only when the processor waits: with the
        // acquire semaphore armed the hardware holds the flip on the fence
        let mut unfinished = false;
        if flipping && owing && !scanout::flip_semaphore_active() {
            let (us, done) = scanout::wait_last_blit(FENCE_BUDGET_US);
            let mut st = STATS.lock();
            st.wait_us = us;
            st.wait_worst_us = st.wait_worst_us.max(us);
            if !done {
                st.fence_timeouts += 1;
                unfinished = true;
            }
        }
        // `should_flip` keeps the window channel's ring from backing up: with
        // the acquire semaphore armed, a flip pushed while the copy engine
        // works retires at best every second frame, and the compositor pushes
        // one every frame. A frame not flipping keeps the surface it composed
        // and shows it on the next one.
        //
        // A frame whose copies have not landed is not handed over.
        //
        // The budget above is eight milliseconds and a screenful through the
        // copy engine about four, so this is reached only by a very large frame
        // (`clear` repainting the whole console). Arming the flip anyway would
        // tear the picture, while the front surface holds a whole one: keeping
        // it one more frame costs latency, and `FLIP_OWED` sends the frame once
        // the copies land.
        //
        // Bounded, like every gate here: a wedged engine is
        // `wait_for_engines`'s problem
        let stalled = if unfinished {
            UNFINISHED_HOLDS.fetch_add(1, Ordering::Relaxed) + 1 < 4
        } else {
            UNFINISHED_HOLDS.store(0, Ordering::Relaxed);
            false
        };
        let held = flipping && owing && (stalled || !scanout::should_flip());
        let flipped = flipping && owing && !held && scanout::flip();
        // Held back or refused, not lost: both leave the picture in the surface
        // nobody scans, and both are cleared by the flip that finally goes out
        FLIP_OWED.store(flipping && owing && !flipped, Ordering::Relaxed);

        // A gate holding most frames back costs more than the stall it saves;
        // see `FLIPS_HELD`. Two in a row, as on the refusal path below
        if held {
            if FLIPS_HELD.fetch_add(1, Ordering::Relaxed) + 1 >= 2
                && scanout::flip_semaphore_active()
            {
                scanout::give_up_flip_semaphore();
                FLIPS_HELD.store(0, Ordering::Relaxed);
                force_full_frame();
            }
        } else {
            FLIPS_HELD.store(0, Ordering::Relaxed);
        }

        // A refused flip.
        //
        // `flip` returning false leaves `front` where it was, so the frame
        // composed is in the surface nobody scans and the next one goes to the
        // same place. The machine runs on (drains input, runs clients,
        // composes) and the monitor holds one still image for ever, which looks
        // like a hang.
        //
        // The window channel's ring refuses: PUT addresses 4 KiB and a flip
        // writes about twenty dwords, so the ring winds roughly once a second,
        // and a wind not coming back inside `reserve`'s hundred milliseconds
        // ends the screen.
        //
        // So a few in a row and the second surface goes. One surface is a
        // working mode, as on any machine whose channel will not take a flip,
        // and the picture comes back on the next frame
        if flipping && owing && !flipped && !held {
            let missed = FLIPS_REFUSED.fetch_add(1, Ordering::Relaxed) + 1;
            // The acquire semaphore goes before the second surface. A window
            // not retiring a flip looks like a semaphore waiting on a value
            // nothing wrote, and giving that up costs one frame of latency;
            // giving up the second surface costs composing outside the blanking
            // interval, at 240 Hz only 152 us
            if missed >= 2 && scanout::flip_semaphore_active() {
                scanout::give_up_flip_semaphore();
                FLIPS_REFUSED.store(0, Ordering::Relaxed);
                force_full_frame();
            } else if missed >= 4 {
                scanout::disable_flip();
                FLIPS_REFUSED.store(0, Ordering::Relaxed);
                force_full_frame();
                crate::serial_println!(
                    "[mayland] the window channel refused {} flips - back to one \
                     surface, composing into the blanking interval", missed);
            }
        } else if flipped {
            FLIPS_REFUSED.store(0, Ordering::Relaxed);
        }

        // Stamped after the flip, so the measurement includes the fence wait
        // and `scanout::flip`, the expensive parts of handing a frame over
        let done = crate::kcore::time::rdtsc();
        {
            let mut st = STATS.lock();
            st.frames += 1;
            st.blits += blits;
            st.dropped += dropped;
            st.last_us = tsc_to_us(done.saturating_sub(boundary));
            st.worst_us = st.worst_us.max(st.last_us);
            // The handover on its own, telling a frame slow to compose from one
            // slow to reach the monitor
            st.flip_us = tsc_to_us(done.saturating_sub(composed_at));
            st.flip_worst_us = st.flip_worst_us.max(st.flip_us);
            st.line_in = line_in;
            st.line_out = line_out;
            st.line_in_worst = st.line_in_worst.max(line_in);
            st.line_out_worst = st.line_out_worst.max(line_out);
            // Ran past the last blanked line: the beam was on the visible area
            // before this finished, and what was being written could be seen
            // half-done. Counted only with one surface: with two a long frame
            // costs only latency
            if !flipped && redrew > 0 && (line_out > timing.vblanke || line_out < line_in) {
                st.overran += 1;
            }
            if last_frame_us != 0 {
                st.period_us = tsc_to_us(boundary.saturating_sub(last_frame_us));
            }
        }
        // Where this frame's time went, and whether the screen went too long
        // without one before it
        phases::add(phases::Phase::Handover, tsc_to_us(done.saturating_sub(composed_at)));
        {
            let st = *STATS.lock();
            phases::end(st.last_us);
            if last_frame_us != 0 {
                let raster_us = 1_000_000_000 / timing.refresh_mhz().max(1) as u64;
                phases::between(st.period_us, raster_us, phases::Counters {
                    stalls: st.stalls,
                    late: st.late,
                    held: scanout::skipped_flips(),
                    refused: scanout::flip_counts().1,
                });
            }
        }
        last_frame_us = boundary;

        let now_ms = frame_time_ms();

        // Keys and mouse movement the interrupts collected since the last
        // frame. Delivered here as this thread may take the server lock; see
        // the note at the top of `input`. Once a frame suits the mouse: a
        // hundred reports a second at sixty frames
        pump_input(now_ms);
        cursor::hw_sync();

        // And the clients those events belong to are asleep in `may_recv`
        server::wake_readers();
    }
}

/// Keys and pointer movement the interrupts collected, passed on
///
/// A function of its own: the frame loop has four ways out of an iteration and
/// each owes the machine this. A machine ignoring the keyboard and mouse looks
/// hung
fn pump_input(now_ms: u32) {
    crate::mayland::input::drain(now_ms);
    if let Some((w, h)) = server::output_size() {
        crate::mayland::input::pointer::drain(w as i32, h as i32, now_ms);
    }
}

/// One line of the stack, for `mayland status`
///
/// What the compositor sees, in drawing order. Which window gets effects is a
/// rule applied to this list, so a wrong screen is read here first: whether
/// there is a backdrop, whether the thing blurred is the thing meant to be
pub struct StackItem {
    pub client: u32,
    pub surface: u32,
    pub rect: (i32, i32, i32, i32),
    pub phys: u64,
    pub stride: usize,
    /// Composed by the GPU this frame
    pub effects: bool,
    /// Over another window the GPU composes, reading what that one wrote
    /// (`dev/mayland-layers-above.md`, section 3)
    pub stacked: bool,
    /// Asked for effects, whether or not it got them. They differ for a window
    /// the compositor refuses (the backdrop, a fullscreen window, one hanging
    /// off an edge): the answer to "I set it and nothing happened"
    pub wants_effects: bool,
    /// What it would be drawn with: its own numbers over the desktop's
    pub alpha: f32,
    pub corner: f32,
}

pub fn stack_dump() -> Vec<StackItem> {
    let Some((w, h)) = server::output_size() else { return Vec::new() };
    let screen = Rect::new(0, 0, w as i32, h as i32);
    // Folded as the last frame folded it, so the stack says what the frame did
    let mut list = server::presentable();
    base::fold_for_status(&mut list, &screen);
    let fx = effect_windows(&list, &screen, &compose::blending(&list));
    list.iter().enumerate().map(|(i, p)| StackItem {
        client: p.client,
        surface: p.surface,
        rect: (p.offset.0, p.offset.1, p.width, p.height),
        phys: p.phys,
        stride: p.stride,
        effects: fx.iter().any(|e| e.index == i),
        stacked: fx.iter().any(|e| e.index == i && e.window.stacked),
        wants_effects: p.fx.on,
        alpha: p.fx.alpha,
        corner: p.fx.corner,
    }).collect()
}

/// Frames that went out before a client's own GPU work had finished, for
/// `mayland status`: non-zero is a client asking for more work than a frame has
/// room for
pub fn late_fences() -> u64 {
    present::LATE_FENCES.load(core::sync::atomic::Ordering::Relaxed)
}
