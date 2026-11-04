// The effects pass
//
// Windows drawn as Hyprland draws them with `xray` on: the shell below the
// windows (the wallpaper and the BACKGROUND and BOTTOM layers) kept as one
// image (`base`), blurred once by dual kawase whenever it changes, and each
// window composed over that with alpha, rounded corners and a shadow. One
// dispatch a window; the blur is three halvings and three doublings, run only
// when what is below the windows changed (`dev/mayland-below-windows.md`).
//
// The kernels are in `shaders/blur.cu`, `shaders/composite.cu` and
// `shaders/over.cu`. This file is bookkeeping: which buffer each level lives
// in, where in the surface the region sits, and what order the launches go in.
//
// The levels live as long as the surface does. They are sized for the surface
// at bring-up, allocated out of video memory and mapped into the compute
// channel's address space then, and given back only when a surface of another
// size asks for its own.
//
// Three rules:
//
//   - The blur lands in a buffer of its own, never back in the surface. The
//     region it covers is wider than the window, because the window's edge has
//     to have something to sample, and only what the window covers reaches the
//     screen.
//   - The pass reads a backdrop kept apart from the surface it writes: a blur
//     fed its own output dissolves the window.
//   - The chain waits on the copy engine's fence before its first dispatch. The
//     two channels are on separate runlists and run at once.

use alloc::vec::Vec;
use spin::Mutex;

use super::gmmu;
use super::vaspace;
use super::gsp_common::chan::Channel;
use super::gsp_common::compute::{self, Programs};
use super::mmio::MmioRegion;

/// How many times the image is halved before it is doubled back. Hyprland's
/// default. Each level doubles the effective radius and quarters the work
pub const PASSES: usize = 3;
/// How many effect windows one frame carries.
///
/// A window is one launch, the blur being computed once for the whole screen.
/// The bound is the effects surfaces' reservation in the compute address space:
/// two mapping slots of 16 MiB a window, under the channel windows at 1 GiB
/// (`chanmem::layout`)
pub const MAX_WINDOWS: usize = 12;
const _: () = assert!(MAX_WINDOWS <= compute::MAX_SLOTS - compute::CLIENT_SLOTS);

/// The most dispatches one update of `base` takes: a clear, a layer for each
/// surface folded into it, and the blur
pub const MAX_BASE_LAYERS: usize = 8;
const _: () = assert!(1 + MAX_BASE_LAYERS + PASSES * 2 <= compute::MAX_SLOTS - compute::CLIENT_SLOTS);

/// The surface a level lives in
#[derive(Clone, Copy)]
struct Level {
    va: u64,
    /// Pixels per row of the allocation, what the kernels index by
    stride: u32,
}

/// One window to be drawn with effects, in screen coordinates
pub struct Window {
    /// Where its top-left is on the screen. Signed: a window sliding in with a
    /// workspace is part way past an edge, and the pass draws the part on the
    /// screen
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    /// The client's pixels: physical address of its top-left and its stride in
    /// bytes. Mapped for the engine by `map_windows`
    pub src_phys: u64,
    pub src_stride: u32,
    /// Whether `src_phys` is video memory (the compositor's own copy of the
    /// window, `mayland::render::texture`, the start of a block the allocator
    /// handed out) or the client's system memory
    pub src_vram: bool,
    /// Whether the fourth byte of a pixel is an alpha channel or padding.
    /// XRGB8888 buffers carry nothing there and the compose must not read it
    pub src_has_alpha: bool,
    pub alpha: f32,
    pub corner: f32,
    pub shadow: f32,
    /// How far the shadow reaches beyond the window, in pixels: the whole of
    /// the window's margin
    pub margin: u32,
    /// Whether it is laid over the blurred shell below the windows or over the
    /// sharp backdrop. Off for a window that only fades: a chain with a radius
    /// of zero still softens the picture
    pub blur: bool,
    /// The part of the region this frame has to redo, in screen pixels:
    /// `(x, y, w, h)`, already grown by the blur's reach so the taps at its
    /// edge land outside anything visible.
    ///
    /// `(0, 0, 0, 0)` means the whole region. The chain, `blurred` and the
    /// compose all cover this one rectangle, which lets `blurred` stay indexed
    /// from its own origin
    pub clip: (u32, u32, u32, u32),
    /// What fraction of its rectangle the window is drawn at.
    ///
    /// 1 is the plain one-to-one read; below it the whole window is drawn into
    /// a box that fraction of the size around the same centre, with its corner
    /// and shadow, which a `popin` animation moves. The region stays the whole
    /// rectangle, so neither the damage nor the blur region changes
    pub scale: f32,
    /// `rounding_power`: the corner is `corner · power / 2` pixels and a
    /// superellipse of this power, Hyprland's (`libmay::shape`)
    pub power: f32,
    /// How much the window's own colour is darkened: 0 for the active window
    pub dim: f32,
    /// The border, drawn over the window in its box grown by `border`: the
    /// colour with its alpha (`0xAARRGGBB`), and how wide. 0 wide is none.
    /// `margin` reaches at least this far
    pub border_colour: u32,
    pub border: f32,
    /// Over another window of this pass: composed over what the frame holds
    /// once the windows below it are drawn, not over the backdrop. Its clip is
    /// laid from the backdrop at the head of the chain, and the windows below
    /// it recompose where it meets them (`dev/mayland-layers-above.md`, section
    /// 3)
    pub stacked: bool,
}

/// A surface the pass can be pointed at, mapped once and kept
#[derive(Clone, Copy, Default)]
struct Mapped {
    phys: u64,
    va: u64,
    /// Bytes covered. Only client buffers use it: they are mapped for the size
    /// the window was when first seen, and a window can grow
    len: u64,
    /// Video memory, not system memory. Only client buffers use it, and a hit
    /// has to match it: the same number in the other memory is somebody else's
    /// pages
    vram: bool,
    /// The frame this mapping was last wanted on.
    ///
    /// The table fills, and the least recently wanted slot is the one to go:
    /// remapping a slot in a frame (`gmmu::map_at`, page tables written through
    /// PRAMIN a word at a time, a TLB invalidate) costs hundreds of
    /// milliseconds while the engines are busy
    seen: u64,
}

pub struct Blur {
    /// The surface everything below was sized for: width, height and stride in
    /// bytes. A surface that differs in any of them gets levels of its own
    sized_for: (u32, u32, u32),
    /// The video memory behind the levels, `blurred` and the backdrop
    owned: Owned,
    levels: [Level; PASSES],
    /// The region, blurred, at full resolution and on its own.
    ///
    /// Doubled back into the surface, the margin the blur has to cover would
    /// end up a square of smeared pixels with a hard edge where the region
    /// stops. Kept apart, the surface stays sharp and the compose picks which
    /// of the two each pixel blends over
    blurred: Level,
    /// What is under the effect windows, kept apart from the screen.
    ///
    /// The pass reads the backdrop and writes the scanout surface, so the
    /// backdrop is rewritten only where something below a window changed, and
    /// for a static wallpaper never.
    ///
    /// `None` when `nobackdrop` is on the boot line or video memory could not
    /// be had: every reader falls back to laying the region down with the copy
    /// engine
    backdrop: Option<Backdrop>,
    /// The shell below the windows, as one image: the wallpaper and every
    /// BACKGROUND and BOTTOM layer laid over it (`submit_base`). `None` when
    /// video memory would not give up another screen, or no `over` program was
    /// built: nothing is folded then
    base: Option<Backdrop>,
    /// Whether `blurred` holds `base`, blurred, for the whole screen. Only
    /// then does a window that asks for blur get it
    blurred_valid: bool,
    /// Every surface a frame can be composed into, mapped before the first
    /// frame. Both when there are two
    targets: [Mapped; 2],
    /// Which of them the pass is currently writing, and its stride in pixels
    target_va: u64,
    target_stride: u32,
    /// Client buffers, mapped when first seen and kept.
    ///
    /// Page tables are written through PRAMIN a word at a time and every change
    /// ends in a TLB invalidate that waits on the MMU, so nothing maps here
    /// once the compositor is drawing. Between frames a client can only change
    /// which buffer it attached, and there are few of those
    windows: [Mapped; MAX_WINDOWS * 2],
    /// Counts frames, so `windows` can tell which mapping has gone longest
    /// without being wanted
    frame: u64,
    down: usize,
    up: usize,
    comp: usize,
    over: Option<usize>,
}

/// The backdrop buffer: what the engine reads it as, and what the copy engine
/// writes it as.
///
/// Two descriptions of one allocation. The compute chain needs a virtual
/// address and a stride in pixels; the copy engine takes a physical address and
/// a pitch in bytes, and `to_copies` speaks `Surface`
#[derive(Clone, Copy)]
struct Backdrop {
    level: Level,
    surface: super::surface::Surface,
}

/// How much of the video memory at `vram` to map for a surface of `bytes`: the
/// whole block the allocator handed out, so the mapping takes the block's own
/// page (`vram::page_for`, 3.8). Memory the allocator did not hand out (the
/// firmware's console surface is GSP-RM's) is mapped no further than `bytes`
fn whole_block(vram: u64, bytes: u64) -> u64 {
    match super::gsp_common::vram::block_size(vram) {
        Some(whole) if whole >= bytes => whole,
        _ => bytes,
    }
}

/// Video memory the pass allocated for itself, and where it is mapped. Kept to
/// give back on a mode change or a failure halfway through bring-up
#[derive(Default)]
struct Owned(Vec<Block>);

struct Block {
    va: u64,
    vram: u64,
    bytes: u64,
}

impl Owned {
    /// Allocate `bytes` of video memory and map it at `va`.
    ///
    /// `Ok(None)` when there is no memory, which only the backdrop treats as
    /// anything but a failure
    fn map_new(
        &mut self,
        bar0: &MmioRegion,
        pdb: u64,
        va: u64,
        bytes: u64,
        owner: &'static str,
    ) -> Result<Option<u64>, &'static str> {
        let Some(vram) = super::scanout::vram_scratch(bytes, owner) else {
            return Ok(None);
        };
        // The whole block, padding included, so it maps with the block's own
        // page: 2 MiB for a full-screen surface, 64 KiB for a level (3.8)
        let bytes = whole_block(vram, bytes);
        // Recorded before the mapping, so a mapping that fails still hands the
        // block back
        self.0.push(Block { va, vram, bytes });
        vaspace::map(bar0, pdb, va, vram, bytes, gmmu::Aperture::Vram)?;
        Ok(Some(vram))
    }

    /// Unmap every block and return it to the allocator. Only once the engine
    /// has stopped reading them
    fn give_back(&self, bar0: &MmioRegion, pdb: u64) -> u64 {
        let mut bytes = 0;
        for b in &self.0 {
            // A mapping that failed partway has already taken its own entries
            // back (`vaspace::map`), so what is recorded is all there is
            vaspace::unmap(bar0, pdb, b.va);
            if super::gsp_common::vram::free(b.vram) {
                bytes += b.bytes;
            }
        }
        bytes
    }
}

impl Blur {
    /// Take the whole pass out of the engine's address space and give its
    /// memory back. Returns how many bytes of video memory went back
    fn release(self, bar0: &MmioRegion, pdb: u64) -> u64 {
        for m in self.targets.iter().chain(self.windows.iter()).filter(|m| m.phys != 0) {
            vaspace::unmap(bar0, pdb, m.va);
        }
        self.owned.give_back(bar0, pdb)
    }
}

static BLUR: Mutex<Option<Blur>> = Mutex::new(None);

/// GSP-RM was unloaded: the levels' video memory and the address space went
/// with it. Dropped without `release`, whose unbinds and frees would concern
/// memory that is no longer anyone's
pub fn forget() {
    BLUR.lock().take();
}

/// The surfaces first, then the levels, then the blurred region, then the
/// backdrop and `base`, then one slot per client buffer.
///
/// Slots: 0-1 the two scanout surfaces, 2-4 the three levels, 5 `blurred`, 6
/// the backdrop, 7 `base`, 8 upward the client buffers, `MAX_WINDOWS * 2` of
/// them, ending at 800 MiB. `SURFACE_VA` is the last region in `compute.rs`'s
/// map; above this list are the channel windows at 1 GiB
/// (`chanmem::CHAN_WINDOW_BASE`), and `nvidia selftest` checks the list stays
/// under them
const VA_TARGET: u64 = compute::SURFACE_VA;
const VA_LEVEL: u64 = compute::SURFACE_VA + 2 * compute::SURFACE_STRIDE;
const VA_BLURRED: u64 = compute::SURFACE_VA + (PASSES as u64 + 2) * compute::SURFACE_STRIDE;
const VA_BACKDROP: u64 = compute::SURFACE_VA + (PASSES as u64 + 3) * compute::SURFACE_STRIDE;
const VA_BASE: u64 = compute::SURFACE_VA + (PASSES as u64 + 4) * compute::SURFACE_STRIDE;
const VA_WINDOW: u64 = compute::SURFACE_VA + (PASSES as u64 + 5) * compute::SURFACE_STRIDE;
/// The end of the list: the effects surfaces' reservation (`chanmem::layout`)
pub const VA_END: u64 = VA_WINDOW + (MAX_WINDOWS * 2) as u64 * compute::SURFACE_STRIDE;

/// Reserve and map everything an effects pass needs.
///
/// Called after the compute channel is up and the display is running, every
/// time the compositor starts. A failure is not fatal: the compositor then
/// draws without effects.
///
/// `idle` waits out whatever the compute engine was last given, asked only when
/// levels are about to be given back: the last frame's chain may still be
/// reading them
pub fn init(
    bar0: &MmioRegion,
    pdb: u64,
    programs: &Programs,
    width: u32,
    height: u32,
    stride_bytes: u32,
    idle: &dyn Fn() -> bool,
    say: &dyn Fn(&str),
) -> Result<(), &'static str> {
    let down = programs.index("blur_down").ok_or("no blur_down program was built")?;
    let up = programs.index("blur_up").ok_or("no blur_up program was built")?;
    let comp = programs.index("composite").ok_or("no composite program was built")?;
    // Optional: without it the shell below the windows is not folded
    let over = programs.index("over");

    // Every surface a frame can land in: two once the compositor has turned
    // flipping on. The pass may be set up before that (`nvidia blur` runs on a
    // bare console), so this can run twice and maps only what is new
    let live = super::scanout::flip_surfaces()
        .or_else(|| super::scanout::console_surface().map(|s| [s.vram, s.vram]))
        .ok_or("nothing is being drawn into")?;
    let bytes = stride_bytes as u64 * height as u64;
    let size = (width, height, stride_bytes);

    // A surface of another size needs levels of another size. The mode changes
    // size only with the compositor stopped (`control::set_mode`), and this
    // runs when it starts again
    let stale = {
        let mut g = BLUR.lock();
        if g.as_ref().is_some_and(|b| b.sized_for != size) { g.take() } else { None }
    };
    if let Some(old) = stale {
        if !idle() {
            *BLUR.lock() = Some(old);
            return Err("the compute engine is still busy with the old levels");
        }
        let (w, h, _) = old.sized_for;
        let back = old.release(bar0, pdb);
        say(&alloc::format!(
            "blur: the surface went from {}x{} to {}x{} - {} KiB of levels given back",
            w, h, width, height, back / 1024));
    }

    if let Some(b) = BLUR.lock().as_mut() {
        // Read again: `chan-cycle` may have rebuilt the programs, and an index
        // is true of one set
        b.down = down;
        b.up = up;
        b.comp = comp;
        b.over = over;
        for phys in live.iter() {
            if b.targets.iter().any(|m| m.phys == *phys) {
                continue;
            }
            // The slot to take is one whose surface is no longer drawn into.
            // Taking slot `i` for `live[i]` puts a new surface over the other
            // live one when the pair comes back in the other order
            let i = b.targets.iter().position(|m| !live.contains(&m.phys)).unwrap_or(0);
            let va = VA_TARGET + i as u64 * compute::SURFACE_STRIDE;
            let len = whole_block(*phys, bytes);
            vaspace::map(bar0, pdb, va, *phys, len, gmmu::Aperture::Vram)
                .map_err(|_| "mapping a surface for the effects pass failed")?;
            b.targets[i] = Mapped { phys: *phys, va, len, seen: 0, vram: true };
            say(&alloc::format!("blur: surface {} at vram {:#x}, va {:#x}", i, phys, va));
        }
        return Ok(());
    }

    let mut owned = Owned::default();
    let built = allocate(bar0, pdb, &mut owned, width, height, stride_bytes, &live, say);
    let (levels, blurred, backdrop, base, targets) = match built {
        Ok(parts) => parts,
        Err(e) => {
            // What was had before the failure goes back now: nothing else knows
            // it was allocated
            for i in 0..2 {
                vaspace::unmap(bar0, pdb, VA_TARGET + i * compute::SURFACE_STRIDE);
            }
            owned.give_back(bar0, pdb);
            return Err(e);
        }
    };

    *BLUR.lock() = Some(Blur {
        sized_for: size,
        owned,
        levels,
        blurred,
        backdrop,
        base,
        blurred_valid: false,
        targets,
        target_va: targets[0].va,
        target_stride: stride_bytes / 4,
        windows: [Mapped::default(); MAX_WINDOWS * 2],
        frame: 0,
        down,
        up,
        comp,
        over,
    });
    say(&alloc::format!("blur: up to {} window(s) a frame", MAX_WINDOWS));
    Ok(())
}

/// The levels, the blurred region, the backdrop, `base` and the surfaces, each
/// allocated into `owned` as it is had
fn allocate(
    bar0: &MmioRegion,
    pdb: u64,
    owned: &mut Owned,
    width: u32,
    height: u32,
    stride_bytes: u32,
    live: &[u64; 2],
    say: &dyn Fn(&str),
) -> Result<([Level; PASSES], Level, Option<Backdrop>, Option<Backdrop>, [Mapped; 2]), &'static str> {
    let mut levels = [Level { va: 0, stride: 0 }; PASSES];
    let (mut w, mut h) = (width, height);
    for (i, level) in levels.iter_mut().enumerate() {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        let stride = (w + 15) & !15;
        let bytes = stride as u64 * h as u64 * 4;
        let va = VA_LEVEL + i as u64 * compute::SURFACE_STRIDE;
        let vram = owned.map_new(bar0, pdb, va, bytes, "blur level")
            .map_err(|_| "mapping a blur level failed")?
            .ok_or("no video memory for the blur levels")?;
        *level = Level { va, stride };
        say(&alloc::format!(
            "blur: level {} is {}x{} at vram {:#x}, va {:#x}", i + 1, w, h, vram, va));
    }

    // The region at full resolution, sized for the whole screen since a window
    // may be nearly that big
    let blurred = {
        let stride = (width + 15) & !15;
        let bytes = stride as u64 * height as u64 * 4;
        let vram = owned.map_new(bar0, pdb, VA_BLURRED, bytes, "blurred region")
            .map_err(|_| "mapping the blurred region failed")?
            .ok_or("no video memory for the blurred region")?;
        say(&alloc::format!(
            "blur: the blurred region is {}x{} at vram {:#x}, va {:#x}",
            width, height, vram, VA_BLURRED));
        Level { va: VA_BLURRED, stride }
    };

    // What is under the windows, kept where the pass can read it without
    // reading the screen. Full-screen for the same reason as `blurred`.
    //
    // Best effort: `nobackdrop` on the boot line, or a machine that cannot
    // spare eight megabytes, goes without, and every reader branches on the
    // `Option`
    let backdrop = if crate::arch::x86_64::grub::cmdline_flag("nobackdrop") {
        say("blur: nobackdrop on the command line - the pass reads the screen");
        None
    } else {
        let stride = (width + 15) & !15;
        let bytes = stride as u64 * height as u64 * 4;
        match owned.map_new(bar0, pdb, VA_BACKDROP, bytes, "effects backdrop")
            .map_err(|_| "mapping the backdrop failed")?
        {
            None => {
                say("blur: no video memory for the backdrop - the pass reads the screen");
                None
            }
            Some(vram) => {
                say(&alloc::format!(
                    "blur: the backdrop is {}x{} at vram {:#x}, va {:#x}",
                    width, height, vram, VA_BACKDROP));
                Some(Backdrop {
                    level: Level { va: VA_BACKDROP, stride },
                    surface: super::surface::Surface {
                        vram, pitch: stride * 4, width, height,
                    },
                })
            }
        }
    };

    // The shell below the windows, folded into one image. Best effort, as the
    // backdrop is: without it the frame composes those layers directly
    let base = {
        let stride = (width + 15) & !15;
        let bytes = stride as u64 * height as u64 * 4;
        match owned.map_new(bar0, pdb, VA_BASE, bytes, "shell below the windows")
            .map_err(|_| "mapping the shell below the windows failed")?
        {
            None => {
                say("blur: no video memory for the shell below the windows - its layers are composed as before");
                None
            }
            Some(vram) => {
                say(&alloc::format!(
                    "blur: the shell below the windows is {}x{} at vram {:#x}, va {:#x}",
                    width, height, vram, VA_BASE));
                Some(Backdrop {
                    level: Level { va: VA_BASE, stride },
                    surface: super::surface::Surface {
                        vram, pitch: stride * 4, width, height,
                    },
                })
            }
        }
    };

    // Mapped now: which surface is drawn into alternates with every flip, and
    // remapping on each would put two thousand page-table writes and a TLB
    // invalidate mid-frame
    let bytes = stride_bytes as u64 * height as u64;
    let mut targets = [Mapped::default(); 2];
    for (i, phys) in live.iter().enumerate() {
        let va = VA_TARGET + i as u64 * compute::SURFACE_STRIDE;
        let len = whole_block(*phys, bytes);
        vaspace::map(bar0, pdb, va, *phys, len, gmmu::Aperture::Vram)
            .map_err(|_| "mapping a surface for the effects pass failed")?;
        targets[i] = Mapped { phys: *phys, va, len, seen: 0, vram: true };
        say(&alloc::format!("blur: surface {} at vram {:#x}, va {:#x}", i, phys, va));
    }
    Ok((levels, blurred, backdrop, base, targets))
}

pub fn ready() -> bool {
    BLUR.lock().is_some()
}

/// The backdrop as a copy-engine destination, or `None` when there is none.
///
/// The compositor lays the same rectangles into this as under the effect
/// windows, so the pass has something to read that is not the screen
pub fn backdrop_surface() -> Option<super::surface::Surface> {
    BLUR.lock().as_ref()?.backdrop.map(|b| b.surface)
}

/// The shell below the windows as the compositor names it, where it is in video
/// memory and its pitch in bytes, or `None` when there is none to fold into
/// (`dev/mayland-below-windows.md`)
pub fn base_surface() -> Option<super::surface::Surface> {
    let guard = BLUR.lock();
    let b = guard.as_ref()?;
    b.over?;
    b.base.map(|d| d.surface)
}

/// Frames the pass was pointed at a surface it had never mapped. If this moves,
/// the pass is writing into a surface other than the one the frame is composed
/// into
static RETARGET_MISSES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// What the pass is pointed at, for `mayland status`
pub struct State {
    /// Physical address and virtual address of each surface it can write
    pub targets: [(u64, u64); 2],
    /// The one this frame goes into, and its stride in pixels
    pub target_va: u64,
    pub target_stride: u32,
    /// Client buffers mapped so far
    pub windows: usize,
    pub misses: u64,
    /// The backdrop the pass reads, if it has one: (vram, va, stride in pixels)
    pub backdrop: Option<(u64, u64, u32)>,
    /// The shell below the windows, the same three numbers, and whether
    /// `blurred` holds it blurred
    pub base: Option<(u64, u64, u32)>,
    pub blurred_valid: bool,
}

pub fn state() -> Option<State> {
    let guard = BLUR.lock();
    let b = guard.as_ref()?;
    Some(State {
        targets: [(b.targets[0].phys, b.targets[0].va), (b.targets[1].phys, b.targets[1].va)],
        target_va: b.target_va,
        target_stride: b.target_stride,
        windows: b.windows.iter().filter(|m| m.phys != 0).count(),
        misses: RETARGET_MISSES.load(core::sync::atomic::Ordering::Relaxed),
        backdrop: b.backdrop.map(|d| (d.surface.vram, d.level.va, d.level.stride)),
        base: b.base.map(|d| (d.surface.vram, d.level.va, d.level.stride)),
        blurred_valid: b.blurred_valid,
    })
}

/// Point the pass at the surface this frame goes into.
///
/// A lookup, not a mapping: both surfaces were mapped at bring-up, so this can
/// run every frame
pub fn retarget(vram: u64, stride_bytes: u32) -> Result<(), &'static str> {
    let mut guard = BLUR.lock();
    let b = guard.as_mut().ok_or("the effects pass is not set up")?;
    let Some(m) = b.targets.iter().find(|m| m.phys == vram) else {
        RETARGET_MISSES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        return Err("the surface being drawn into was not mapped at bring-up");
    };
    b.target_va = m.va;
    b.target_stride = stride_bytes / 4;
    Ok(())
}

/// Where a window's pixels are in the engine's address space, mapping them the
/// first time they are seen.
///
/// The client's buffer is in system memory at whatever offset in its pool it
/// asked for, so the page is mapped and the offset kept. Remembered: a client
/// drawing into one buffer costs nothing after the first frame, and one
/// alternating two costs two mappings.
///
/// The compositor's copy of a window is a block of video memory of its own,
/// mapped whole so it takes the block's page: 2 MiB for a full-screen window
/// (3.8). Mapped once per window
fn window_va(
    b: &mut Blur,
    bar0: &MmioRegion,
    pdb: u64,
    win: &Window,
) -> Result<u64, &'static str> {
    let mut page = win.src_phys & !0xfff;
    let mut inside = win.src_phys - page;
    let mut bytes = (inside + win.src_stride as u64 * win.h as u64 + 0xfff) & !0xfff;
    if win.src_vram {
        // A client's window buffer can start part way into its pool, and both
        // halves of a double-buffered one should share a slot: the whole block
        // it lies in, when that fits a slot (`dev/gpu-window-buffers.md`). The
        // compositor's own copies start their block, and come out the same
        let end = win.src_phys + win.src_stride as u64 * win.h as u64;
        match super::gsp_common::vram::block_containing(win.src_phys) {
            Some((base, size)) if size <= compute::SURFACE_STRIDE && end <= base + size => {
                page = base;
                inside = win.src_phys - base;
                bytes = size;
            }
            _ => bytes = whole_block(page, bytes),
        }
    }
    let aper = if win.src_vram { gmmu::Aperture::Vram } else { gmmu::Aperture::Sysmem };
    // A slot is `SURFACE_STRIDE` of address space, and a mapping past it takes
    // out the next slot's. The compositor makes no copy that large
    // (`texture::MAX_BYTES`), so this is somebody else's
    if win.src_vram && bytes > compute::SURFACE_STRIDE {
        return Err("a window's copy in video memory is larger than a slot");
    }

    // A hit has to cover this frame's window as well as the one that made it.
    // Every console buffer is the same shadow at the same page, so it always
    // hits, and a tile that grows when a window closes would read past a
    // mapping made at half the height: a fault in the engine
    let now = b.frame;
    // A hit has to be true of the page tables too. A slot can name a mapping
    // the GMMU has since taken out (a client's pool being destroyed unbinds
    // every range inside it), and an address nothing translates is a fault in
    // the engine. `vaspace::lookup` is the authority; this array is a cache
    let slot = match b.windows.iter().position(|m| m.phys == page && m.vram == win.src_vram) {
        Some(i)
            if b.windows[i].len >= bytes
                && vaspace::lookup(pdb, b.windows[i].va) == Some((page, b.windows[i].len)) =>
        {
            b.windows[i].seen = now;
            return Ok(b.windows[i].va + inside);
        }
        Some(i) => i,
        // A free slot, or failing that the one nobody has asked for in the
        // longest time
        None => b
            .windows
            .iter()
            .position(|m| m.phys == 0)
            .unwrap_or_else(|| {
                let mut oldest = 0usize;
                for (i, m) in b.windows.iter().enumerate() {
                    if m.seen < b.windows[oldest].seen {
                        oldest = i;
                    }
                }
                oldest
            }),
    };
    let va = VA_WINDOW + slot as u64 * compute::SURFACE_STRIDE;
    // `vaspace::map`, not `map_at`: it takes out whatever was at this address
    // first. Otherwise a slot re-used for a shorter window leaves the tail of
    // the previous mapping valid, pointing at that client's pages
    vaspace::map(bar0, pdb, va, page, bytes, aper)
        .map_err(|_| "mapping a window's pixels failed")?;
    b.windows[slot] = Mapped {
        phys: page, va, len: bytes.next_multiple_of(4096), seen: now, vram: win.src_vram,
    };
    Ok(va + inside)
}

/// Map whatever this frame is about to read, before the frame starts.
///
/// Every mapping in the pass happens here or at bring-up, never between the
/// submissions of a frame. Page tables are written through PRAMIN a word at a
/// time and every change ends in a TLB invalidate that waits on the MMU:
/// microseconds while the engines are idle, hundreds of milliseconds while they
/// are not. This runs before the backdrop is queued
pub fn map_windows(
    bar0: &MmioRegion,
    pdb: u64,
    windows: &[Window],
) -> Result<(), &'static str> {
    let mut guard = BLUR.lock();
    let b = guard.as_mut().ok_or("the effects pass is not set up")?;
    // One tick per frame, which is all the ordering the eviction below needs
    b.frame = b.frame.wrapping_add(1);
    for win in windows.iter().take(MAX_WINDOWS) {
        window_va(b, bar0, pdb, win)?;
    }
    Ok(())
}

/// Blur arguments, as the compiled kernel expects to find them
fn blur_params(
    dst: u64, src: u64, dstride: u32, sstride: u32,
    dw: u32, dh: u32, sw: u32, sh: u32, radius: f32,
) -> Vec<u8> {
    let mut p = Vec::with_capacity(44);
    p.extend_from_slice(&dst.to_le_bytes());
    p.extend_from_slice(&src.to_le_bytes());
    for v in [dstride, sstride, dw, dh, sw, sh] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    p.extend_from_slice(&radius.to_le_bytes());
    p
}

/// Threads per block. 16x16 is 256 threads, which at 40 registers apiece fits
/// six blocks on a multiprocessor, enough to hide the memory latency these
/// kernels are made of
const BLOCK: (u32, u32, u32) = (16, 16, 1);

fn grid(w: u32, h: u32) -> (u32, u32, u32) {
    ((w + BLOCK.0 - 1) / BLOCK.0, (h + BLOCK.1 - 1) / BLOCK.1, 1)
}

/// Where a rectangle of the target surface begins, as an address
fn at(base: u64, stride_px: u32, x: u32, y: u32) -> u64 {
    base + (y as u64 * stride_px as u64 + x as u64) * 4
}

/// Down the chain and back up: `w` by `h` pixels of `src` blurred into `dst`,
/// both pitch-linear in the compute address space with strides in pixels. Six
/// launches, pushed onto `chain` in order.
///
/// The sizes are recomputed at each step: a region halves and halves again, and
/// rounding the wrong way reads a row never written
fn push_kawase(
    chain: &mut Vec<u64>,
    b: &Blur,
    programs: &Programs,
    sass_version: u32,
    (src_va, src_stride): (u64, u32),
    (dst_va, dst_stride): (u64, u32),
    (w, h): (u32, u32),
    radius: f32,
) -> Result<(), &'static str> {
    let mut sizes = [(0u32, 0u32); PASSES];
    let (mut sw, mut sh) = (w, h);
    let (mut from_va, mut from_stride) = (src_va, src_stride);
    for (i, level) in b.levels.iter().enumerate() {
        let dw = (sw / 2).max(1);
        let dh = (sh / 2).max(1);
        chain.push(programs.prepare(
            chain.len(), b.down,
            &blur_params(level.va, from_va, level.stride, from_stride, dw, dh, sw, sh, radius),
            BLOCK, grid(dw, dh), sass_version)?);
        sizes[i] = (dw, dh);
        from_va = level.va;
        from_stride = level.stride;
        sw = dw;
        sh = dh;
    }
    for i in (0..PASSES).rev() {
        let (to_va, to_stride, dw, dh) = if i == 0 {
            (dst_va, dst_stride, w, h)
        } else {
            (b.levels[i - 1].va, b.levels[i - 1].stride, sizes[i - 1].0, sizes[i - 1].1)
        };
        let level = b.levels[i];
        chain.push(programs.prepare(
            chain.len(), b.up,
            &blur_params(to_va, level.va, to_stride, level.stride, dw, dh, sw, sh, radius),
            BLOCK, grid(dw, dh), sass_version)?);
        sw = dw;
        sh = dh;
    }
    Ok(())
}

/// One step of bringing the shell below the windows up to date, in screen
/// coordinates: `w` by `h` pixels at `x`, `y` of `base`
#[derive(Clone, Copy)]
pub struct BaseOp {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Which surface of `submit_base`'s list is laid there, or `None` to clear
    /// the rectangle to black, what no surface covers
    pub layer: Option<usize>,
    /// The alpha it is drawn with, multiplied by its own alpha channel when it
    /// has one (`Window::src_has_alpha`)
    pub alpha: f32,
}

/// `over.cu`'s modes
const OVER_ALPHA: u32 = 0;
const OVER_OPAQUE: u32 = 1;
const OVER_CLEAR: u32 = 2;

/// Bring the shell below the windows up to date and, when `blur` names a
/// radius, blur all of it into `blurred`: Hyprland's `preBlurForCurrentMonitor`
/// (`dev/mayland-below-windows.md`).
///
/// `layers` are the surfaces folded into it, bottom first, described as windows
/// (where each one's top-left is on the screen and where its pixels are) and
/// mapped by `map_windows` beforehand. `ops` say what to lay where, in order.
/// Returns the chain's fence; nothing here waits
pub fn submit_base(
    chan: &mut Channel,
    bar0: &MmioRegion,
    pdb: u64,
    programs: &Programs,
    sass_version: u32,
    layers: &[Window],
    ops: &[BaseOp],
    blur: Option<f32>,
    after: Option<(u64, u32)>,
) -> Result<u32, &'static str> {
    let mut guard = BLUR.lock();
    let b = guard.as_mut().ok_or("the effects pass is not set up")?;
    let over = b.over.ok_or("no over program was built")?;
    let base = b.base.ok_or("there is no shell below the windows to draw into")?;
    if ops.len() > 1 + MAX_BASE_LAYERS {
        return Err("more layers below the windows than one update takes");
    }
    // Whatever happens below, `blurred` no longer holds the shell below the
    // windows, until the chain that blurs it again has run
    b.blurred_valid = false;

    let mut chain: Vec<u64> = Vec::with_capacity(ops.len() + PASSES * 2);
    for op in ops {
        let (sw, sh) = (base.surface.width, base.surface.height);
        if op.w == 0 || op.h == 0 || op.x + op.w > sw || op.y + op.h > sh {
            return Err("a layer below the windows reaches past the screen");
        }
        let dst = at(base.level.va, base.level.stride, op.x, op.y);
        let (src, sstride, mode) = match op.layer {
            None => (0u64, 0u32, OVER_CLEAR),
            Some(i) => {
                let l = layers.get(i).ok_or("a layer below the windows that was not passed")?;
                // The rectangle has to lie inside the layer: the source is read
                // one-to-one, and outside it is somebody else's memory
                let (lx, ly) = (op.x as i64 - l.x as i64, op.y as i64 - l.y as i64);
                if lx < 0 || ly < 0 || lx + op.w as i64 > l.w as i64 || ly + op.h as i64 > l.h as i64 {
                    return Err("a layer below the windows does not cover what it is asked to draw");
                }
                let va = window_va(b, bar0, pdb, l)?;
                let src = va + ly as u64 * l.src_stride as u64 + lx as u64 * 4;
                (src, l.src_stride / 4, if l.src_has_alpha { OVER_ALPHA } else { OVER_OPAQUE })
            }
        };
        let mut params = Vec::with_capacity(40);
        params.extend_from_slice(&dst.to_le_bytes());
        params.extend_from_slice(&src.to_le_bytes());
        for v in [base.level.stride, sstride, op.w, op.h] {
            params.extend_from_slice(&v.to_le_bytes());
        }
        params.extend_from_slice(&op.alpha.to_le_bytes());
        params.extend_from_slice(&mode.to_le_bytes());
        chain.push(programs.prepare(
            chain.len(), over, &params, BLOCK, grid(op.w, op.h), sass_version)?);
    }
    // The whole screen, once: what changed below the windows reaches every
    // window's blur, and a blur clamped to a damaged rectangle puts a seam at
    // its edge. Hyprland blurs the whole monitor here too, only when something
    // below the windows changed
    if let Some(radius) = blur {
        push_kawase(&mut chain, b, programs, sass_version,
                    (base.level.va, base.level.stride),
                    (b.blurred.va, b.blurred.stride),
                    (base.surface.width, base.surface.height), radius)?;
    }
    if chain.is_empty() {
        return Err("nothing to draw below the windows");
    }
    crate::mm::dma::DmaBuffer::write_barrier();
    let fence = chan.submit_compute_chain(bar0, &chain, after)
        .map_err(|_| "the chain below the windows was refused by the ring")?;
    b.blurred_valid = blur.is_some();
    Ok(fence)
}

/// `blurred` no longer holds the shell below the windows: a chain that was to
/// blur it did not finish, or nothing is folded any more
pub fn forget_blurred() {
    if let Some(b) = BLUR.lock().as_mut() {
        b.blurred_valid = false;
    }
}

/// Draw every effect window, as one submission the copy engine has to finish
/// before it starts.
///
/// One launch a window: the window composed over the blurred shell below the
/// windows, which `submit_base` keeps, or over the sharp backdrop when it asks
/// for no blur or there is none. Returns the fence of the whole chain. Nothing
/// here waits
pub fn submit(
    chan: &mut Channel,
    bar0: &MmioRegion,
    pdb: u64,
    programs: &Programs,
    sass_version: u32,
    surface: (u32, u32),
    windows: &[Window],
    after: Option<(u64, u32)>,
) -> Result<u32, &'static str> {
    let mut guard = BLUR.lock();
    let b = guard.as_mut().ok_or("the effects pass is not set up")?;
    if b.target_stride == 0 {
        return Err("no surface has been given to the effects pass");
    }
    if windows.is_empty() {
        return Err("nothing to draw");
    }

    let mut chain: Vec<u64> = Vec::with_capacity(windows.len() * 2);
    // What each window's launch covers, worked out before any launch is made: a
    // stacked window's clip is laid from the backdrop at the head of the chain,
    // before the windows below it write theirs
    let mut grids: Vec<(&Window, u32, u32, u32, u32, u32)> = Vec::with_capacity(windows.len());

    for win in windows.iter().take(MAX_WINDOWS) {
        // The region is the window and its shadow. The shadow shrinks instead
        // of clipping: the same on all four sides, cut down to what fits
        // between the window and the screen edges and the effect windows below
        // it (`effects::margin_for`). The kernel places the window at (margin,
        // margin) inside the region, with no room for four different margins,
        // so a window against an edge gets a narrower shadow, and one past an
        // edge none.
        //
        // Past an edge is a window sliding in with a workspace. The region is
        // cut to the screen and the window's corner lies outside it, which the
        // compose takes: its offsets are signed for a clip starting inside the
        // window
        let (sw, sh) = (surface.0 as i32, surface.1 as i32);
        let (wx, wy, ww, wh) = (win.x, win.y, win.w as i32, win.h as i32);
        if wx + ww <= 0 || wy + wh <= 0 || wx >= sw || wy >= sh {
            continue;
        }
        let margin = (win.margin as i32)
            .min(wx)
            .min(wy)
            .min(sw - (wx + ww))
            .min(sh - (wy + wh))
            .max(0);
        let fx0 = (wx - margin).max(0);
        let fy0 = (wy - margin).max(0);
        let frw = ((wx + ww + margin).min(sw) - fx0) as u32;
        let frh = ((wy + wh + margin).min(sh) - fy0) as u32;
        let (fx0, fy0) = (fx0 as u32, fy0 as u32);
        let margin = margin as u32;
        // The whole region is what the window occupies; the clip is what this
        // frame has to do about it. Composing the rest would rewrite the pixels
        // it holds
        let (cx, cy, cw, ch) = win.clip;
        let (x0, y0, rw, rh) = if cw == 0 || ch == 0 {
            (fx0, fy0, frw, frh)
        } else {
            let ax0 = fx0.max(cx);
            let ay0 = fy0.max(cy);
            let ax1 = (fx0 + frw).min(cx + cw);
            let ay1 = (fy0 + frh).min(cy + ch);
            if ax1 <= ax0 || ay1 <= ay0 {
                continue;
            }
            (ax0, ay0, ax1 - ax0, ay1 - ay0)
        };
        if rw == 0 || rh == 0 {
            continue;
        }
        grids.push((win, x0, y0, rw, rh, margin));
    }

    // The frame under each stacked window, put back to the backdrop: what the
    // windows below it then compose over. Under `nobackdrop` the copy batch has
    // laid it already, and the backdrop is the frame itself
    if let Some(d) = b.backdrop {
        let over = b.over.ok_or("no over program was built")?;
        for &(_, x0, y0, rw, rh, _) in grids.iter().filter(|g| g.0.stacked) {
            let mut params = Vec::with_capacity(40);
            params.extend_from_slice(&at(b.target_va, b.target_stride, x0, y0).to_le_bytes());
            params.extend_from_slice(&at(d.level.va, d.level.stride, x0, y0).to_le_bytes());
            for v in [b.target_stride, d.level.stride, rw, rh] {
                params.extend_from_slice(&v.to_le_bytes());
            }
            params.extend_from_slice(&1.0f32.to_le_bytes());
            params.extend_from_slice(&OVER_OPAQUE.to_le_bytes());
            chain.push(programs.prepare(chain.len(), over, &params, BLOCK, grid(rw, rh), sass_version)?);
        }
    }

    for &(win, x0, y0, rw, rh, margin) in &grids {
        // What the compose blends over, sharp: the backdrop buffer when there
        // is one, and the scanout surface when not (`nobackdrop` on the boot
        // line, or video memory that would not give up eight megabytes). The
        // same pixels either way, but the scanout surface is also what this
        // pass writes, so reading it means laying the region down before every
        // pass.
        //
        // A stacked window reads the frame: the windows below it are in it by
        // the time it runs, over its clip laid from the backdrop
        let (under_va, under_stride) = match b.backdrop {
            Some(d) if !win.stacked => (d.level.va, d.level.stride),
            _ => (b.target_va, b.target_stride),
        };
        // And soft: the shell below the windows, blurred, read at the same
        // place on the screen. Windows below are not in it (`xray`), and a
        // window asking for no blur blends over the sharp backdrop
        let (soft_va, soft_stride) = if win.blur && b.blurred_valid {
            (at(b.blurred.va, b.blurred.stride, x0, y0), b.blurred.stride)
        } else {
            (at(under_va, under_stride, x0, y0), under_stride)
        };

        // The window's own pixels, where the client drew them. Mapped by
        // `map_windows` before the frame, so this is a lookup
        let src = window_va(b, bar0, pdb, win)?;

        // The compose covers the window and its margin in one grid
        let mut params = Vec::with_capacity(112);
        params.extend_from_slice(&at(b.target_va, b.target_stride, x0, y0).to_le_bytes());
        params.extend_from_slice(&at(under_va, under_stride, x0, y0).to_le_bytes());
        params.extend_from_slice(&soft_va.to_le_bytes());
        params.extend_from_slice(&src.to_le_bytes());
        params.extend_from_slice(&b.target_stride.to_le_bytes());
        params.extend_from_slice(&under_stride.to_le_bytes());
        params.extend_from_slice(&soft_stride.to_le_bytes());
        params.extend_from_slice(&(win.src_stride / 4).to_le_bytes());
        params.extend_from_slice(&win.w.to_le_bytes());
        params.extend_from_slice(&win.h.to_le_bytes());
        // The grid, and where the window's own top-left sits inside it. Three
        // numbers (grid size, window placement, shadow reach), since a clip
        // makes the offsets negative when the rectangle starts inside the
        // window
        params.extend_from_slice(&rw.to_le_bytes());
        params.extend_from_slice(&rh.to_le_bytes());
        params.extend_from_slice(&(win.x - x0 as i32).to_le_bytes());
        params.extend_from_slice(&(win.y - y0 as i32).to_le_bytes());
        // Clipped to the region: a shadow cannot be drawn where there are no
        // pixels for it
        let falloff = (win.margin.min(margin) as f32).max(1.0);
        for v in [win.alpha, win.corner, win.shadow, falloff] {
            params.extend_from_slice(&v.to_le_bytes());
        }
        params.extend_from_slice(&(win.src_has_alpha as u32).to_le_bytes());
        params.extend_from_slice(&win.scale.to_le_bytes());
        params.extend_from_slice(&win.power.to_le_bytes());
        params.extend_from_slice(&win.dim.to_le_bytes());
        params.extend_from_slice(&win.border_colour.to_le_bytes());
        params.extend_from_slice(&win.border.to_le_bytes());
        chain.push(programs.prepare(
            chain.len(), b.comp, &params, BLOCK, grid(rw, rh), sass_version)?);
    }

    if chain.is_empty() {
        return Err("every window was too small to draw");
    }
    crate::mm::dma::DmaBuffer::write_barrier();
    chan.submit_compute_chain(bar0, &chain, after)
        .map_err(|_| "the effects chain was refused by the ring")
}

/// Blur a whole surface, with nothing composed over it. `nvidia blur` runs
/// this, to test the blur with no window or blending over it
pub fn submit_whole(
    chan: &mut Channel,
    bar0: &MmioRegion,
    programs: &Programs,
    sass_version: u32,
    width: u32,
    height: u32,
    radius: f32,
) -> Result<u32, &'static str> {
    let guard = BLUR.lock();
    let b = guard.as_ref().ok_or("the effects pass is not set up")?;
    if b.target_stride == 0 {
        return Err("no surface has been given to the effects pass");
    }

    let mut chain: Vec<u64> = Vec::with_capacity(PASSES * 2);
    push_kawase(&mut chain, b, programs, sass_version,
                (b.target_va, b.target_stride), (b.target_va, b.target_stride),
                (width, height), radius)?;

    crate::mm::dma::DmaBuffer::write_barrier();
    chan.submit_compute_chain(bar0, &chain, None)
        .map_err(|_| "the blur chain was refused by the ring")
}
