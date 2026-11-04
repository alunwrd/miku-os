// The compositor's copy of every window it draws, in the card's own memory
//
// A client draws with the processor into a pool of system memory. Reading a
// window from there every frame takes it across the bus whole even when it only
// moves, fades or scales: 4-6 ms for a full-screen window at the GB206's idle
// link (Gen1 x8, about 2 GB/s) against a 4.17 ms frame.
//
// So each window the card draws has a copy in video memory, and the bus carries
// only what the client changed. Hyprland's `updateSynchronousTexture` does the
// same with a shm buffer. See `dev/mayland-architecture.md`, "Window pixels the
// GPU reads live in video memory".
//
// The table is the compositor's and the memory is the backend's: whether a copy
// is whole or needs only the damage is frame state, kept beside `LAST`; where
// the memory comes from and how the engine fills it is behind
// `Scanout::texture_*`. A backend the processor drives has none, and then
// nothing here does anything.
//
// A copy is either whole or damage:
//
//   whole   a copy that is new, or of another size or stride, or one that was
//           not brought up to date in the frame before this one.
//           `frame_composed` drops all the damage of a surface the frame did
//           not draw (one on another workspace), so a frame a copy sat out is
//           damage it will never see. A surface the frame did draw keeps what
//           it committed while the frame was composing
//   damage  otherwise, clipped to the buffer: what the client says changed
//           since its last commit
//
// The processor never reads a copy. `Presentable::phys` stays the client's
// memory for every path the processor draws (`compose`, the software cursor
// patch, the ghost); only the engine's paths take `Presentable::gpu`.
//
// A window whose buffer is already video memory (a `gpu_alloc` pool, our
// linux-dmabuf, `dev/gpu-window-buffers.md`) gets no copy: `gpu` is the
// client's buffer and the engines read it in place. The processor does not read
// the card's memory, since from write-combining memory every load is its own
// trip across PCIe. Where one of its paths needs such a window's pixels,
// `mirror` has the copy engine bring those rectangles into system memory first,
// and `phys` points there for the frame.
//
// A GSP-RM unload needs no hook here: it is refused while the compositor runs
// (`bringup/unload.rs`), and `stop` gives every copy back first.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

use crate::drivers::gpu::scanout::{TextureCopy, TextureDownload};
use crate::mm::dma::DmaBuffer;
use crate::mayland::server;

use super::damage::Rect;

/// The largest copy made, in bytes of the client's buffer. The effects pass
/// maps a copy whole into a window slot, and a slot is 16 MiB of address space
/// (`compute::SURFACE_STRIDE`); the allocator pads a copy to its page, 2 MiB at
/// this size, so a buffer up to 16 MiB still fits. A larger window (4K) is read
/// from its client's memory
pub const MAX_BYTES: u64 = 16 << 20;

/// More damaged rectangles than this in one frame go up as one rectangle around
/// them. Every copy is a launch of the engine
const MAX_PIECES: usize = 8;

struct Texture {
    /// The surface it is a copy of, keyed as `damage::LAST` keys it
    key: (u32, u32),
    /// The start of its block of video memory, as `texture_alloc` gave it
    vram: u64,
    /// The buffer it was made for. Another size or stride is another copy
    width: i32,
    height: i32,
    stride: usize,
    /// The frame it was last brought up to date in (`Table::frame`)
    frame: u64,
}

struct Table {
    live: Vec<Texture>,
    /// Copies whose surface left the frame, or changed size under them. Given
    /// back in `reap`, once the engine has finished every frame that read them
    retired: Vec<u64>,
    /// Counts the frames that composed anything. A copy that missed a frame
    /// missed its damage
    frame: u64,
    /// This frame's uploads, worked out by `sync` and sent by `submit`, and the
    /// copies they are for, so a refusal can make those whole again
    pending: Vec<TextureCopy>,
    touched: Vec<(u32, u32)>,
    /// System-memory mirrors of windows that live in video memory, for the
    /// processor's paths, holding only what a frame read
    mirrors: Vec<Mirror>,
}

/// One GPU window, laid out as its buffer is (same stride) in system memory the
/// processor may read. Only the rectangles `mirror` brought over this frame are
/// current
struct Mirror {
    key: (u32, u32),
    buf: DmaBuffer,
}

static TABLE: Mutex<Table> = Mutex::new(Table {
    live: Vec::new(), retired: Vec::new(), frame: 0, pending: Vec::new(), touched: Vec::new(),
    mirrors: Vec::new(),
});

/// What `mayland status` says about the copies
static LAST_PX: AtomicU64 = AtomicU64::new(0);
static TOTAL_PX: AtomicU64 = AtomicU64::new(0);
static WHOLE: AtomicU64 = AtomicU64::new(0);
/// Windows drawn from their client's memory because a copy could not be had:
/// too large for a slot, or no video memory left
static REFUSED: AtomicU64 = AtomicU64::new(0);
/// Rectangles the backend would not upload
static UPLOAD_REFUSED: AtomicU64 = AtomicU64::new(0);
/// Windows whose buffer is video memory, read in place in the last frame
static IN_PLACE: AtomicU64 = AtomicU64::new(0);
/// Pixels of such windows mirrored for the processor, last frame and in all
static MIRRORED_LAST_PX: AtomicU64 = AtomicU64::new(0);
static MIRRORED_TOTAL_PX: AtomicU64 = AtomicU64::new(0);
/// Times the processor had to leave such a window out: no backend to bring it
/// over, no system memory for a mirror, or a refused download
static LEFT_OUT: AtomicU64 = AtomicU64::new(0);

/// `notex` on the boot line, read once: 0 not asked yet, 1 copies on, 2 off
static MODE: AtomicU8 = AtomicU8::new(0);

/// Whether copies are made at all. `notex` reads every window from its client's
/// memory instead
pub fn enabled() -> bool {
    match MODE.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let on = !crate::arch::x86_64::grub::cmdline_flag("notex");
            MODE.store(if on { 1 } else { 2 }, Ordering::Relaxed);
            on
        }
    }
}

/// The copies, for `mayland status`
#[derive(Clone, Copy, Default)]
pub struct Counts {
    /// Copies there are now, and the bytes of client buffer they hold
    pub live: usize,
    pub bytes: u64,
    /// Pixels uploaded in the last frame that composed anything, and in all
    pub last_px: u64,
    pub total_px: u64,
    /// Uploads that were a whole window rather than its damage
    pub whole: u64,
    /// Windows read from their client's memory for want of a copy
    pub refused: u64,
    /// Rectangles the backend refused to upload
    pub upload_refused: u64,
    /// Windows whose buffer is video memory, read in place in the last frame
    pub in_place: u64,
    /// Pixels of them mirrored into system memory for the processor, in the
    /// last frame that mirrored anything and in all
    pub mirrored_last_px: u64,
    pub mirrored_total_px: u64,
    /// Times the processor left one out instead of reading video memory
    pub left_out: u64,
}

pub fn counts() -> Counts {
    let t = TABLE.lock();
    Counts {
        live: t.live.len(),
        bytes: t.live.iter().map(|x| x.stride as u64 * x.height as u64).sum(),
        last_px: LAST_PX.load(Ordering::Relaxed),
        total_px: TOTAL_PX.load(Ordering::Relaxed),
        whole: WHOLE.load(Ordering::Relaxed),
        refused: REFUSED.load(Ordering::Relaxed),
        upload_refused: UPLOAD_REFUSED.load(Ordering::Relaxed),
        in_place: IN_PLACE.load(Ordering::Relaxed),
        mirrored_last_px: MIRRORED_LAST_PX.load(Ordering::Relaxed),
        mirrored_total_px: MIRRORED_TOTAL_PX.load(Ordering::Relaxed),
        left_out: LEFT_OUT.load(Ordering::Relaxed),
    }
}

/// A processor path met a window it may not read and has no mirror of: left
/// out of what it composes, and counted
pub(super) fn left_out() {
    LEFT_OUT.fetch_add(1, Ordering::Relaxed);
}

/// A frame that cleared the damage without composing (`present` returning
/// before it reached `sync`). Every copy has now missed a frame, and the next
/// `sync` makes them whole again
pub(super) fn missed_frame() {
    let mut t = TABLE.lock();
    t.frame = t.frame.wrapping_add(1);
}

/// Works out what every window's copy needs from what its client committed, and
/// points the frame's GPU paths at the copies. Called once a frame, after the
/// engines have finished the last one and before anything is composed. Nothing
/// is sent from here, see `submit`
///
/// A window with no copy keeps `gpu` as `None` and is drawn from its client's
/// memory. A window whose buffer is video memory keeps `gpu` at that buffer and
/// gets no copy
pub(super) fn sync(list: &mut [server::Presentable]) {
    // The GPU windows on this frame, and their mirrors, kept only while they
    // are on it and still in video memory: a mirror is read in the frame that
    // filled it and never again
    //
    // The shell below the windows is the exception: it is folded into the list
    // only after this runs (`base`) and is on every frame while it exists, so
    // its mirror is kept until `base` says it is gone (`forget_mirror`)
    {
        let mut t = TABLE.lock();
        t.mirrors.retain(|m| m.key == super::base::KEY
                             || list.iter().any(|p| (p.client, p.surface) == m.key && p.vram.is_some()));
        IN_PLACE.store(list.iter().filter(|p| p.vram.is_some()).count() as u64, Ordering::Relaxed);
    }
    // The processor composes every frame on that path, so there is no card
    // memory to put a copy in, and asking every frame would only count refusals
    if !enabled() || super::software() {
        return;
    }
    // Counted before anything can give up: `frame_composed` clears the damage
    // whether or not a copy saw it, so a frame returning uncounted would leave
    // every copy looking current
    let mut t = TABLE.lock();
    t.frame = t.frame.wrapping_add(1);
    let frame = t.frame;
    t.pending.clear();
    t.touched.clear();
    let Some(backend) = super::backend() else { return };

    // A ghost takes the copy of the window it is the afterimage of. Its pixels
    // are that window's last frame, which the copy holds, apart from damage
    // committed since then, which the ghost carries as its own
    // (`Server::push_ghost`). Only once the window has left the frame: until
    // then the copy is still the window's
    for p in list.iter() {
        let Some(from) = p.from else { continue };
        let key = (p.client, p.surface);
        if t.live.iter().any(|x| x.key == key) {
            continue;
        }
        if list.iter().any(|q| (q.client, q.surface) == from) {
            continue;
        }
        if let Some(x) = t.live.iter_mut().find(|x| x.key == from) {
            x.key = key;
        }
    }

    // Copies of surfaces this frame does not draw. A copy that sat a frame out
    // is stale and would be made whole again when its surface came back, so it
    // is not kept
    let mut i = 0;
    while i < t.live.len() {
        let key = t.live[i].key;
        if list.iter().any(|p| (p.client, p.surface) == key) {
            i += 1;
        } else {
            let gone = t.live.swap_remove(i);
            t.retired.push(gone.vram);
        }
    }

    let mut ups: Vec<TextureCopy> = Vec::new();
    let mut touched: Vec<(u32, u32)> = Vec::new();
    let mut px = 0u64;
    let mut pieces: Vec<Rect> = Vec::with_capacity(MAX_PIECES + 1);
    for p in list.iter_mut() {
        p.gpu = p.vram;
        if p.vram.is_some() {
            // Already the card's: nothing to upload, and a copy left from when
            // this surface's buffer was system memory holds other pixels
            let key = (p.client, p.surface);
            if let Some(i) = t.live.iter().position(|x| x.key == key) {
                let old = t.live.swap_remove(i);
                t.retired.push(old.vram);
            }
            continue;
        }
        if p.width <= 0 || p.height <= 0 || p.phys == 0 || (p.width as usize) * 4 > p.stride {
            continue;
        }
        let bytes = p.stride as u64 * p.height as u64;
        if bytes > MAX_BYTES {
            REFUSED.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let key = (p.client, p.surface);

        // The copy this window has, if it is still the right shape
        let mut at = t.live.iter().position(|x| x.key == key);
        if let Some(i) = at {
            let x = &t.live[i];
            if x.width != p.width || x.height != p.height || x.stride != p.stride {
                let old = t.live.swap_remove(i);
                t.retired.push(old.vram);
                at = None;
            }
        }
        let i = match at {
            Some(i) => i,
            None => {
                let Some(vram) = backend.texture_alloc(bytes) else {
                    REFUSED.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                t.live.push(Texture {
                    key, vram,
                    width: p.width, height: p.height, stride: p.stride,
                    // Never brought up to date, so this frame makes it whole
                    frame: 0,
                });
                t.live.len() - 1
            }
        };
        let x = &mut t.live[i];

        // Whole, or only what changed
        let buffer = Rect::new(0, 0, p.width, p.height);
        pieces.clear();
        if x.frame.wrapping_add(1) != frame {
            pieces.push(buffer);
            WHOLE.fetch_add(1, Ordering::Relaxed);
        } else {
            for (dx, dy, dw, dh) in &p.damage {
                if let Some(d) = Rect::new(*dx, *dy, *dw, *dh).intersect(&buffer) {
                    pieces.push(d);
                }
            }
            if pieces.len() > MAX_PIECES {
                let around = pieces.iter().skip(1).fold(pieces[0], |a, r| a.union(r));
                pieces.clear();
                pieces.push(around);
            }
        }
        for r in pieces.iter() {
            let offset = r.y0 as u64 * p.stride as u64 + r.x0 as u64 * 4;
            ups.push(TextureCopy {
                src_phys: p.phys + offset,
                texture: x.vram,
                offset,
                pitch: p.stride as u32,
                w: (r.x1 - r.x0) as u32,
                h: (r.y1 - r.y0) as u32,
            });
            px += r.area();
        }
        if !pieces.is_empty() {
            touched.push(key);
        }
        x.frame = frame;
        p.gpu = Some(x.vram);
    }

    t.pending = ups;
    t.touched = touched;
    LAST_PX.store(px, Ordering::Relaxed);
    TOTAL_PX.fetch_add(px, Ordering::Relaxed);
}

/// Sends this frame's uploads, queued on the copy engine ahead of everything
/// else the frame submits
///
/// They go on the channel the frame's own copies go on, which runs its work in
/// order, and the effects pass waits for that channel's fence before it reads.
/// So nothing that reads a copy can start before the copy is current, and
/// nothing waits for the uploads on the processor.
///
/// Separate from `sync` because a copy the effects pass has not seen is mapped
/// for it before anything of the frame is queued
/// (`scanout::map_effect_windows`). A mapping ends in a TLB invalidate:
/// microseconds with the engines idle, hundreds of milliseconds with them busy,
/// and the uploads are what would make them busy.
///
/// A refusal means the frame was built on copies now older than their buffers.
/// It goes out as it is, and the next frame makes those copies whole and
/// redraws everything from them. Returns false for the one caller that keeps
/// something built from the copies: the shell below the windows has to start
/// again from the whole screen (`base`)
pub(super) fn submit() -> bool {
    let (ups, touched) = {
        let mut t = TABLE.lock();
        (core::mem::take(&mut t.pending), core::mem::take(&mut t.touched))
    };
    if ups.is_empty() {
        return true;
    }
    let Some(backend) = super::backend() else { return true };
    let (_, refused) = backend.texture_upload(&ups);
    if refused > 0 {
        UPLOAD_REFUSED.fetch_add(refused, Ordering::Relaxed);
        let mut t = TABLE.lock();
        for key in &touched {
            if let Some(x) = t.live.iter_mut().find(|x| x.key == *key) {
                x.frame = 0;
            }
        }
        drop(t);
        super::damage::force_full_frame();
    }
    refused == 0
}

/// Brings the processor what it is about to read of every window in video
/// memory: for each such window under `areas` (screen coordinates), the
/// rectangles of its buffer `compose` will read there, scaled as `compose`
/// scales them while a curve shrinks the window, copied by the engine into the
/// window's mirror and waited for. `phys` then points at the mirror for the
/// rest of the frame. A window this cannot serve keeps `phys = 0`, and the
/// processor leaves it out (`left_out`)
///
/// dma-buf's `begin_cpu_access`, done by its only reader
/// (`dev/gpu-window-buffers.md`). Waits because the processor reads the mirror
/// as soon as this returns, and before the frame's own copies are queued the
/// engine has nothing else to do
pub(super) fn mirror(list: &mut [server::Presentable], areas: &[Rect]) {
    if areas.is_empty() || !list.iter().any(|p| p.vram.is_some()) {
        return;
    }
    let backend = super::backend();
    let mut pieces: Vec<Rect> = Vec::with_capacity(MAX_PIECES + 1);
    let mut frame_px = 0u64;
    for p in list.iter_mut() {
        let Some(vram) = p.vram else { continue };
        let r = super::compose::placed(p);
        let (dw, dh) = (r.x1 - r.x0, r.y1 - r.y0);
        if dw <= 0 || dh <= 0 || p.width <= 0 || p.height <= 0 {
            continue;
        }
        // What `compose` reads for each area it composes over the window, in
        // the buffer's own coordinates: nearest neighbour, the same arithmetic
        pieces.clear();
        for a in areas {
            let Some(hit) = r.intersect(a) else { continue };
            let sx0 = (hit.x0 - r.x0) * p.width / dw;
            let sx1 = (hit.x1 - r.x0 - 1) * p.width / dw + 1;
            let sy0 = (hit.y0 - r.y0) * p.height / dh;
            let sy1 = (hit.y1 - r.y0 - 1) * p.height / dh + 1;
            if let Some(s) = Rect::new(sx0, sy0, sx1 - sx0, sy1 - sy0)
                .intersect(&Rect::new(0, 0, p.width, p.height))
            {
                pieces.push(s);
            }
        }
        if pieces.is_empty() {
            continue;
        }
        if pieces.len() > MAX_PIECES {
            let around = pieces.iter().skip(1).fold(pieces[0], |a, r| a.union(r));
            pieces.clear();
            pieces.push(around);
        }
        let Some(backend) = backend.as_ref() else {
            left_out();
            continue;
        };

        // The window's mirror, made or kept: the buffer's whole size at its
        // stride, so a pixel is at the same offset in both
        let key = (p.client, p.surface);
        let bytes = p.stride * p.height as usize;
        let pages = bytes.div_ceil(4096);
        let mut t = TABLE.lock();
        let at = match t.mirrors.iter().position(|m| m.key == key) {
            Some(i) if t.mirrors[i].buf.pages() >= pages => Some(i),
            other => {
                if let Some(i) = other {
                    t.mirrors.swap_remove(i);
                }
                match DmaBuffer::alloc(pages) {
                    Ok(buf) => {
                        t.mirrors.push(Mirror { key, buf });
                        Some(t.mirrors.len() - 1)
                    }
                    Err(_) => None,
                }
            }
        };
        let Some(i) = at else {
            drop(t);
            left_out();
            continue;
        };
        let dst = t.mirrors[i].buf.phys();
        drop(t);

        let downs: Vec<TextureDownload> = pieces.iter().map(|s| {
            let offset = s.y0 as u64 * p.stride as u64 + s.x0 as u64 * 4;
            TextureDownload {
                src_vram: vram + offset,
                dst_phys: dst + offset,
                src_pitch: p.stride as u32,
                dst_pitch: p.stride as u32,
                w: (s.x1 - s.x0) as u32,
                h: (s.y1 - s.y0) as u32,
            }
        }).collect();
        let (_, refused) = backend.texture_download(&downs);
        if refused != 0 {
            p.phys = 0;
            left_out();
            continue;
        }
        p.phys = dst;
        frame_px += pieces.iter().map(|s| s.area()).sum::<u64>();
    }
    if frame_px != 0 {
        MIRRORED_LAST_PX.store(frame_px, Ordering::Relaxed);
        MIRRORED_TOTAL_PX.fetch_add(frame_px, Ordering::Relaxed);
    }
}

/// Lets go of one surface's mirror (the shell below the windows', when it stops
/// being folded)
pub(super) fn forget_mirror(key: (u32, u32)) {
    TABLE.lock().mirrors.retain(|m| m.key != key);
}

/// Copies a GPU window's rectangle into system memory, for the readers outside
/// the frame with no mirror to read: the afterimage of a closing window
/// (`shell::ghost`) and `may_desktop.capture`. Waits, as `mirror` does. False
/// with no backend, or when it refuses
pub(in crate::mayland) fn download(src_vram: u64, src_pitch: usize, dst_phys: u64, dst_pitch: usize,
                                   w: i32, h: i32) -> bool {
    if w <= 0 || h <= 0 {
        return false;
    }
    let Some(backend) = super::backend() else { return false };
    let down = TextureDownload {
        src_vram,
        dst_phys,
        src_pitch: src_pitch as u32,
        dst_pitch: dst_pitch as u32,
        w: w as u32,
        h: h as u32,
    };
    backend.texture_download(&[down]).1 == 0
}

/// Gives back the copies retired before the frame the engines just finished.
/// Called from `wait_for_engines`, beside `shm::reap`: the fence it waited for
/// proves nothing still reads them
pub(super) fn reap() {
    let gone: Vec<u64> = core::mem::take(&mut TABLE.lock().retired);
    if gone.is_empty() {
        return;
    }
    let Some(backend) = super::backend() else { return };
    for vram in gone {
        backend.texture_free(vram);
    }
}

/// Every copy back to the backend. The compositor has stopped and the engines
/// are waited for first
pub(super) fn release_all() {
    let (live, retired) = {
        let mut t = TABLE.lock();
        // Mirrors are system memory the engines are not reading: the
        // processor read them in the frame that filled them
        t.mirrors.clear();
        (core::mem::take(&mut t.live), core::mem::take(&mut t.retired))
    };
    if live.is_empty() && retired.is_empty() {
        return;
    }
    let Some(backend) = super::backend() else { return };
    let _ = backend.fence_wait(200_000);
    for vram in live.iter().map(|x| x.vram).chain(retired) {
        backend.texture_free(vram);
    }
}
