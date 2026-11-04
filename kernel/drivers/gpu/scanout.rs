// What a compositor needs of a display, and nothing about who provides it
//
// A frame reaches the screen two ways. The card scans out of video memory, its
// copy engine moves the rectangles and its raster says when; without a card the
// processor writes the rows into the framebuffer firmware left. Both implement
// this contract. It lives beside `display.rs`, `edid.rs` and `bochs.rs`,
// outside `nvidia/`, since a driver implements it.
//
// This is not DRM: MikuOS ships one compositor, in the same address space, so
// the seam is a trait with no ioctls, serialisation or versioning. From DRM
// comes the model: planes, a scanout buffer, damage clips, an explicit fence, a
// page flip at a frame boundary.
//
// The shape of a frame
//
// One loop, whoever is presenting:
//
//      wait_next_frame     when should this frame start
//      fence_wait          is the last one finished being read
//      ...compose...
//      blit_batch          put the rectangles down
//      flip                show them, at a frame boundary
//
// A backend with no raster answers `wait_next_frame` by sleeping until the
// deadline; one with no second buffer says `flipping` is false and the loop
// composes into what is being scanned. The degenerate answers are defaults
// here.

use alloc::string::String;

pub use super::nvidia::gsp_common::modeset::RasterTiming;
pub use super::nvidia::scanout::{ScreenBlit, TextureCopy, TextureDownload};

/// One output's geometry, as the compositor sees it. Per output: the protocol
/// advertises one today, and the head this is read from has a second
#[derive(Clone, Copy, Debug)]
pub struct OutputInfo {
    pub width: u32,
    pub height: u32,
    /// Millihertz, which is what `may_output.mode` carries
    pub refresh_mhz: u32,
}

pub trait Scanout: Send + Sync {
    /// For `mayland status` and the boot log
    fn name(&self) -> &'static str;

    fn output_count(&self) -> usize { 1 }
    fn output(&self, out: usize) -> Option<OutputInfo>;

    // the clock

    /// Block until the next frame should start. False means the wait gave up,
    /// and the caller skips the frame rather than composing into the beam
    fn wait_next_frame(&self, out: usize, timeout_us: u64) -> bool;

    /// Where the beam is, if this backend has one
    fn raster_pos(&self, out: usize) -> Option<(u16, u16)> { let _ = out; None }
    fn raster_timing(&self, out: usize) -> Option<RasterTiming> { let _ = out; None }

    // putting a frame down

    /// Which of the two surfaces is being drawn into, when there are two
    fn draw_slot(&self, out: usize) -> Option<usize> { let _ = out; None }

    /// The rectangles, in order, back to front. Returns (copied, refused)
    fn blit_batch(&self, out: usize, blits: &[ScreenBlit]) -> (u64, u64);

    /// Wait for the last batch to have been read. `(microseconds, finished)`.
    ///
    /// The default is for a backend whose copies are finished when the call
    /// returns, every backend the processor drives
    fn fence_wait(&self, timeout_us: u64) -> (u64, bool) { let _ = timeout_us; (0, true) }

    /// Free two engines that are each waiting for the other
    fn unstick(&self) {}

    /// Which build of the engine the effects run on, while it is usable. A
    /// backend that rebuilds a failed engine (`dev/gpu-memory-compute.md` 3.6)
    /// hands out a new number each time, telling the compositor that an engine
    /// it gave up on is worth trying again
    fn compute_generation(&self) -> Option<u32> { None }

    /// Take this physical range out of the GPU's own address space.
    ///
    /// Called when memory stops belonging to whoever showed it: a client pool
    /// destroyed, a surface replaced. A backend reaching memory by physical
    /// address has nothing to do; one walking page tables does, since a
    /// translation left behind is a writable pointer into pages the allocator
    /// may hand to anything.
    ///
    /// Returns how many mappings were dropped, for `mayland status`
    fn unbind_range(&self, phys: u64, bytes: u64) -> usize {
        let _ = (phys, bytes);
        0
    }

    // The compositor's copies of windows, in the display's own memory
    //
    // `dev/mayland-architecture.md`, "Window pixels the GPU reads live in video
    // memory". A backend the processor drives has no memory of its own and
    // keeps the defaults: the compositor reads every window where its client
    // drew it.

    /// Memory for one window's copy, `bytes` of it: where it starts in the
    /// display's memory. `None` when there is none to be had
    fn texture_alloc(&self, bytes: u64) -> Option<u64> { let _ = bytes; None }
    /// Give a copy back. Only once nothing in flight still reads it
    fn texture_free(&self, texture: u64) { let _ = texture; }
    /// Bring copies up to date: queued ahead of the frame that reads them, on
    /// the engine that frame's copies go on. Returns (copied, refused)
    fn texture_upload(&self, ups: &[TextureCopy]) -> (u64, u64) { (0, ups.len() as u64) }
    /// The other way: rectangles of a window in video memory, a client's own
    /// buffer there, into system memory, waited for, since the processor reads
    /// them when this returns (`dev/gpu-window-buffers.md`). Returns (copied,
    /// refused). A backend with no memory of its own refuses
    fn texture_download(&self, downs: &[TextureDownload]) -> (u64, u64) { (0, downs.len() as u64) }

    // showing it

    fn flipping(&self, out: usize) -> bool { let _ = out; false }
    fn flip(&self, out: usize) -> bool { let _ = out; false }
    fn should_flip(&self, out: usize) -> bool { let _ = out; false }
    fn enable_flip(&self, out: usize) -> Result<(), &'static str> {
        let _ = out;
        Err("this display has one surface and nothing to flip to")
    }
    fn disable_flip(&self, out: usize) { let _ = out; }
    fn flip_counts(&self, out: usize) -> (u64, u64) { let _ = out; (0, 0) }
    /// Why flips were refused, in words, for `mayland status`
    fn refusals(&self) -> String { String::new() }

    /// The acquire semaphore that holds a flip back until the copies land.
    /// Absent means the caller waits on the fence itself
    fn flip_semaphore_active(&self) -> bool { false }
    fn give_up_flip_semaphore(&self) {}
    fn disable_flip_semaphore(&self) {}

    // the compute compositor, where there is one

    /// Blur, rounded corners and shadows in one pass. `None` means the
    /// processor does it (`mayland::render::compose`)
    fn composes(&self) -> bool { false }

    // the head's own cursor plane

    fn cursor_available(&self) -> bool { false }
    fn cursor_move(&self, x: i32, y: i32) -> bool { let _ = (x, y); false }
    fn cursor_set_image(&self, px: &[u32], w: u32, h: u32, hot_x: i32, hot_y: i32) -> bool {
        let _ = (px, w, h, hot_x, hot_y);
        false
    }
    fn cursor_hide(&self) -> bool { false }
    /// Give the plane back. The compositor draws the pointer again from the
    /// next frame
    fn cursor_release(&self) {}

    // Handing the screen back and forth with the console
    //
    // The console paints until the first frame lands and again when compositing
    // stops, and on the card those two moments are where a flush through PRAMIN
    // would land in the surface about to be shown. A backend with no such
    // hazard leaves all four alone.

    fn suspend_console_flush(&self) {}
    fn resume_console_flush(&self) -> bool { false }
    fn force_console_flush_hook(&self) {}
    fn force_front_surface(&self) {}
}
