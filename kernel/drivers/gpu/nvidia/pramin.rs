// PRAMIN: CPU access to video memory that survives a BAR1 remap
//
// The boot framebuffer lives in this GPU's BAR1 aperture. GSP-RM switches BAR1
// from physical to virtual mode during its display init, and CPU stores through
// the old BAR1 window then stop landing on the surface the display scans.
//
// BAR0 carries a 1 MiB window (at 0x700000) that can be pointed anywhere in
// VRAM by writing the base, in 64 KiB units, to NV_PBUS_PRAMIN_WINDOW (0x1700).
// It goes through BAR0, so GSP-RM's remap does not touch it.
//
// The same window reads the VBIOS shadow out of VRAM on this chip (see
// vbios.rs).

use super::mmio::MmioRegion;
use super::pci::GpuDevice;
use crate::io::framebuffer::font;

/// Window base, in 64 KiB units.
///
/// `NV_PBUS_BAR0_WINDOW` is the register on every chip up to Ada. Hopper moved
/// it to `NV_XAL_EP_BAR0_WINDOW`, and Blackwell inherits that: nouveau gives
/// GB202/GB203 `gh100_instmem_new`, whose `set_bar0_window_addr` writes
/// 0x10fd40.
///
/// Writing the legacy register on a chip that has moved on does not fault. The
/// window stays where it was, and accesses read back consistently at the wrong
/// address
const PBUS_PRAMIN_WINDOW: u32 = 0x0000_1700;
const XAL_EP_BAR0_WINDOW: u32 = 0x0010_FD40;
/// BASE field, 21:0
const XAL_EP_BAR0_WINDOW_BASE: u32 = 0x003F_FFFF;

/// Which of the two this GPU honours, decided once by writing a base and
/// reading it back. A register that is not there answers with the PRI poison
/// pattern, so the check cannot false-positive
static WINDOW_REG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn window_reg(bar0: &MmioRegion) -> u32 {
    use core::sync::atomic::Ordering;
    let cached = WINDOW_REG.load(Ordering::Acquire);
    if cached != 0 {
        return cached;
    }
    // The probe value must fit BASE (21:0), or the readback fails on bits the
    // field cannot hold. Flip one bit of what is there, then put it back
    let orig = bar0.read32(XAL_EP_BAR0_WINDOW);
    let probe = (orig ^ 1) & XAL_EP_BAR0_WINDOW_BASE;
    bar0.write32(XAL_EP_BAR0_WINDOW, probe);
    let honoured = bar0.read32(XAL_EP_BAR0_WINDOW) & XAL_EP_BAR0_WINDOW_BASE == probe;
    bar0.write32(XAL_EP_BAR0_WINDOW, orig);
    let reg = if honoured { XAL_EP_BAR0_WINDOW } else { PBUS_PRAMIN_WINDOW };
    WINDOW_REG.store(reg, Ordering::Release);
    reg
}

/// Which register the window probe settled on, for reporting
pub fn window_reg_in_use() -> u32 {
    WINDOW_REG.load(core::sync::atomic::Ordering::Acquire)
}
/// The 1 MiB aperture the window maps into BAR0
const PRAMIN_APERTURE: u32 = 0x0070_0000;
const PRAMIN_SIZE: u64 = 0x10_0000;

/// One window base register, and more than one thread wanting it.
///
/// The console flush runs on the keyboard thread while a command on the shell
/// thread may be mid-sweep. A re-base under someone else silently moves their
/// writes a megabyte or more away. Held for the life of a `Window`.
///
/// Every `Window` is created, used and dropped inside one function, and nothing
/// creates a second while holding the first, so a plain non-reentrant lock fits
static WINDOW_LOCK: spin::Mutex<()> = spin::Mutex::new(());

/// A borrowed PRAMIN window. The previous base is restored on drop: the VBIOS
/// reader and other users assume they own it
pub struct Window<'a> {
    _guard: spin::MutexGuard<'static, ()>,
    bar0: &'a MmioRegion,
    /// Currently mapped VRAM base, 1 MiB aligned so a whole aperture's worth
    /// of consecutive writes needs no reprogramming
    base: u64,
    mapped: bool,
    /// Register this chip's window base lives in
    reg: u32,
    saved: u32,
}

impl<'a> Window<'a> {
    /// Take the window. Blocks until whoever else has it is done with it
    pub fn new(bar0: &'a MmioRegion) -> Self {
        Self {
            _guard: WINDOW_LOCK.lock(),
            bar0,
            base: 0,
            mapped: false,
            reg: window_reg(bar0),
            saved: bar0.read32(window_reg(bar0)),
        }
    }

    fn ensure(&mut self, vram_off: u64) {
        let want = vram_off & !(PRAMIN_SIZE - 1);
        if self.mapped && want == self.base {
            return;
        }
        self.bar0.write32(self.reg, (want >> 16) as u32);
        self.base = want;
        self.mapped = true;
    }

    pub fn write32(&mut self, vram_off: u64, val: u32) {
        self.ensure(vram_off);
        self.bar0
            .write32(PRAMIN_APERTURE + (vram_off - self.base) as u32, val);
    }

    pub fn read32(&mut self, vram_off: u64) -> u32 {
        self.ensure(vram_off);
        self.bar0
            .read32(PRAMIN_APERTURE + (vram_off - self.base) as u32)
    }

    /// `count` consecutive dwords. The window base is re-checked only when the
    /// run crosses an aperture boundary
    pub fn fill32(&mut self, vram_off: u64, val: u32, count: usize) {
        for i in 0..count {
            self.write32(vram_off + (i as u64) * 4, val);
        }
    }
}

/// Can the CPU write video memory here?
///
/// An uninitialised memory controller reads as zero whether or not the window
/// works, so this is asked once GSP-RM is up. It decides where a scanout
/// surface can live.
///
/// Writes a pattern, reads it back, and restores what was there. `vram_off`
/// should point at memory carved out of a usable FB region
pub fn write_probe(bar0: &MmioRegion, vram_off: u64) -> bool {
    const PATTERN: [u32; 4] = [0x5254_5350, 0x554b_494d, 0xa5a5_5a5a, 0x0123_4567];
    let mut w = Window::new(bar0);
    let mut saved = [0u32; 4];
    for (i, s) in saved.iter_mut().enumerate() {
        *s = w.read32(vram_off + i as u64 * 4);
    }
    for (i, p) in PATTERN.iter().enumerate() {
        w.write32(vram_off + i as u64 * 4, *p);
    }
    // Re-map the window away and back, so a read cannot be answered from a
    // posted-write buffer
    let _ = w.read32(vram_off + PRAMIN_SIZE);
    let ok = PATTERN
        .iter()
        .enumerate()
        .all(|(i, p)| w.read32(vram_off + i as u64 * 4) == *p);
    for (i, s) in saved.iter().enumerate() {
        w.write32(vram_off + i as u64 * 4, *s);
    }
    ok
}

/// Copy a buffer into video memory a dword at a time.
///
/// Every store is an MMIO write through a 1 MiB window re-based each time the
/// copy crosses it; a framebuffer is written once here
pub fn copy_to_vram(bar0: &MmioRegion, vram_off: u64, src: &[u8]) {
    let mut w = Window::new(bar0);
    let mut i = 0;
    while i + 4 <= src.len() {
        let v = u32::from_le_bytes([src[i], src[i + 1], src[i + 2], src[i + 3]]);
        w.write32(vram_off + i as u64, v);
        i += 4;
    }
}

/// Push host writes out to where the GPU's own clients can see them.
///
/// Writes made through the PRAMIN window are flushed by reading the window
/// back: `kbusFlushSingle_GM107`, option
/// `BUS_FLUSH_VIDEO_MEMORY_VIA_PRAMIN_WINDOW`, "read the PRAMIN
/// pseudo-bar0-register window to flush writes in it to FB ... these
/// transactions are snooped at GPU before hitting BAR0 master". A read of any
/// address inside the window will do.
///
/// Then the old register: `g84_bar_flush` writes 1 to 0x070000 and waits for
/// bit 1 to clear. Modern RM does not use it on this path, and a register that
/// is not there answers with the PRI poison pattern, whose bit 1 is clear. Kept
/// for the chips where it is real, skipped where the readback says it is not
pub fn flush_fb(bar0: &MmioRegion) -> bool {
    // The one that covers page tables written a few instructions ago
    let _ = bar0.read32(PRAMIN_APERTURE);

    const REG_FLUSH: u32 = 0x0007_0000;
    if MmioRegion::is_pri_error(bar0.read32(REG_FLUSH)) {
        return true;
    }
    bar0.write32(REG_FLUSH, 0x0000_0001);
    let khz = crate::kcore::time::tsc_khz().max(1);
    let t0 = crate::kcore::time::rdtsc();
    loop {
        if bar0.read32(REG_FLUSH) & 0x0000_0002 == 0 {
            return true;
        }
        let ms = crate::kcore::time::rdtsc().wrapping_sub(t0) / khz;
        if ms >= 2000 {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// Clean the GPU's L2 down to video memory, as the chip's own registers do it
/// (`NV_XAL_EP_UFLUSH_L2_FLUSH_DIRTY`, Blackwell `pri_nv_xal_ep.h`).
///
/// Reading the PRAMIN window pushes writes out of the BAR0 window, where the
/// driver's `BUS_FLUSH_VIDEO_MEMORY_VIA_PRAMIN_WINDOW` stops. RM's own aperture
/// self-test (`kbusVerifyBar2_GH100`, and identically `_GB202`) writes through
/// one path, calls `kmemsysSendL2InvalidateEvict`, and only then reads back
/// through the other, since a readback without it would hit L2. Page tables
/// written through PRAMIN and read by the GPU's MMU are that pairing.
///
/// The protocol is a token: reading `_FLUSH_DIRTY` starts a clean and hands
/// back a ticket, and `_FLUSH_DIRTY_COMPLETED` counts up to it. Both are
/// read-only registers; the read is the request.
///
/// Not on the console's path: this runs once, where RM makes the call, with a
/// budget small enough that a wrong answer cannot wedge the machine. The
/// register can answer `0xbad0fb00`, which is not a token
pub fn l2_flush_dirty(bar0: &MmioRegion) -> bool {
    const FLUSH_DIRTY: u32 = 0x0010_f810;
    const FLUSH_DIRTY_COMPLETED: u32 = 0x0010_f814;
    const TOKEN: u32 = 0x7fff_ffff;
    const BUSY: u32 = 1 << 31;
    /// `NV_XAL_EP_MEMOP_MAX_OUTSTANDING`, the depth the driver's loop allows
    const MAX_OUTSTANDING: u32 = 256;

    let start = bar0.read32(FLUSH_DIRTY);
    if MmioRegion::is_pri_error(start) {
        return false;
    }
    let start = start & TOKEN;

    let khz = crate::kcore::time::tsc_khz().max(1);
    let t0 = crate::kcore::time::rdtsc();
    loop {
        let done = bar0.read32(FLUSH_DIRTY_COMPLETED);
        if MmioRegion::is_pri_error(done) {
            return false;
        }
        // `kmemsysReadL2InvalidateEvict_GH100`, both exits: not busy, or the
        // completed token has passed the one we were handed. The counter wraps,
        // so "passed" is the wrapped difference against the queue depth
        if done & BUSY == 0 || start.wrapping_sub(done & TOKEN) & TOKEN > MAX_OUTSTANDING {
            return true;
        }
        if crate::kcore::time::rdtsc().wrapping_sub(t0) / khz >= 20 {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// What the host side of the GPU thinks went wrong.
///
/// A CPU read of BAR1 returning `0xbad0ac00` is a PRI error code from the
/// `0xBAD0xx` family, host-side and not an MMU fault, and the MMU's fault
/// buffers are programmed by GSP-RM in its own address space.
/// `NV_XAL_EP_INTR_0` latches which timeout fired
pub fn xal_intr(bar0: &MmioRegion) -> u32 {
    bar0.read32(0x0010_f100)
}

/// Read `count` dwords back out of video memory at `vram_off`. A scanout
/// surface full of zeros displays perfectly and looks like a display engine
/// that never fetched anything
pub fn sample_vram(bar0: &MmioRegion, vram_off: u64, out: &mut [u32]) {
    let mut w = Window::new(bar0);
    for (i, o) in out.iter_mut().enumerate() {
        *o = w.read32(vram_off + i as u64 * 4);
    }
}

impl Drop for Window<'_> {
    fn drop(&mut self) {
        self.bar0.write32(self.reg, self.saved);
    }
}

/// What a PRAMIN read returns, so a failed search can be told from a window not
/// reading VRAM
pub struct Probe {
    pub vram_size_mb: u64,
    /// First dwords at a few VRAM offsets, one per aperture position
    pub dumps: [(u64, [u32; 4]); 4],
    /// Offset of the VBIOS image signature (0xAA55) if the window can see it.
    /// The shadow is in VRAM on a POSTed card, so finding it proves the read
    /// path
    pub vbios_at: Option<u64>,
}

/// Read-only check of the PRAMIN read path
pub fn probe_vram(bar0: &MmioRegion) -> Probe {
    let vram = (bar0.read32(FB_USABLE_SIZE_MB) as u64) << 20;
    let mut w = Window::new(bar0);
    let mut dumps = [(0u64, [0u32; 4]); 4];
    for (i, off) in [0u64, 0x10_0000, 0x100_0000, 0x1000_0000].iter().enumerate() {
        let mut vals = [0u32; 4];
        for (j, v) in vals.iter_mut().enumerate() {
            *v = w.read32(off + (j as u64) * 4);
        }
        dumps[i] = (*off, vals);
    }
    // The BIOS shadow is 64 KiB aligned; 64 MiB of sweep is plenty
    let limit = (64 << 20).min(vram);
    let mut vbios_at = None;
    let mut off = 0u64;
    while off < limit {
        if w.read32(off) & 0xFFFF == 0xAA55 {
            vbios_at = Some(off);
            break;
        }
        off += 64 << 10;
    }
    Probe { vram_size_mb: vram >> 20, dumps, vbios_at }
}

/// Text output straight into the scanout surface, bypassing BAR1.
///
/// Monochrome: the pixel value has equal R, G and B, so the surface's channel
/// order does not matter. A diagnostic channel, not a terminal
pub struct VramConsole {
    /// Byte offset of the scanout surface inside VRAM. In BAR1 physical mode a
    /// BAR1 offset is the VRAM offset, so this can be derived before GSP-RM
    /// changes anything
    vram_base: u64,
    pitch: u64,
    width: u32,
    /// First scanline of the band this writes into
    band_y: u32,
    band_rows: u32,
    row: u32,
}

/// Rows reserved at the bottom of the screen. The normal console output
/// above stays untouched and readable
const BAND_ROWS: u32 = 14;
const LINE_HEIGHT: u32 = font::CHAR_HEIGHT as u32 + 2;

/// Signature planted in the framebuffer to find it again from the VRAM side
const SIG: [u32; 4] = [0x4D49_4B55, 0x5052_414D, 0x4642_4C4F, 0x4341_5445];
/// NV_PGC6_AON_SECURE_SCRATCH: usable framebuffer size in MiB. The scan covers
/// all of it: the GOP surface need not be in the low 512 MiB
const FB_USABLE_SIZE_MB: u32 = 0x0011_83A4;

/// Two passes. Scanout surfaces are large and heavily aligned, so the 64 KiB
/// sweep almost always wins at a quarter of a second; the page sweep is the
/// fallback that cannot miss
const SCAN_STEP_COARSE: u64 = 64 << 10;
const SCAN_STEP_FINE: u64 = 4096;

impl VramConsole {
    /// Find the scanout surface in VRAM and open a text band at its bottom.
    ///
    /// The VRAM address cannot be derived from the BAR1 address: UEFI's GOP
    /// driver has put BAR1 in virtual mode, so a BAR1 offset is a translated
    /// address. The surface is located empirically: plant a signature through
    /// the BAR1 mapping while it works, then scan VRAM through PRAMIN for it.
    /// The signature goes into the last scanline and is restored afterwards
    pub fn locate(gpu: &GpuDevice, bar0: &MmioRegion) -> Option<Self> {
        let fbi = crate::arch::x86_64::grub::framebuffer()?;
        if fbi.bpp != 32 {
            return None;
        }
        // Must still be this GPU's aperture, or the signature lands in someone
        // else's framebuffer
        let loc = super::fb::find_in_bars(gpu, fbi.addr)?;
        if loc.bar_index != 1 {
            return None;
        }
        let band_h = BAND_ROWS * LINE_HEIGHT;
        if fbi.height <= band_h {
            return None;
        }

        let pitch = fbi.pitch as u64;
        let sig_off = pitch * (fbi.height as u64 - 1);
        let fb = crate::arch::x86_64::grub::phys_to_virt(fbi.addr) as *mut u32;
        let sig_idx = (sig_off / 4) as usize;

        let mut saved = [0u32; SIG.len()];
        unsafe {
            for (i, s) in saved.iter_mut().enumerate() {
                *s = fb.add(sig_idx + i).read_volatile();
            }
            for (i, v) in SIG.iter().enumerate() {
                fb.add(sig_idx + i).write_volatile(*v);
            }
            // The framebuffer is write-combining; read one word back to force
            // the store buffer out before looking from the VRAM side
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            let _ = fb.add(sig_idx).read_volatile();
        }

        // Distinguish "the signature is somewhere we did not look" from "the
        // store never landed"
        let readback = unsafe { fb.add(sig_idx).read_volatile() };
        let vram_size = (bar0.read32(FB_USABLE_SIZE_MB) as u64) << 20;
        crate::serial_println!(
            "[pramin] signature at fb+{:#x}, readback {:#010x} (want {:#010x}), vram {} MiB",
            sig_off, readback, SIG[0], vram_size >> 20
        );
        let found = {
            let mut w = Window::new(bar0);
            // Only the first word is read per candidate; the other three are
            // checked on a hit. One MMIO read per step makes scanning gigabytes
            // practical
            let mut scan = |step: u64| -> Option<u64> {
                let mut base = 0u64;
                while base + sig_off + 16 < vram_size {
                    if w.read32(base + sig_off) == SIG[0]
                        && w.read32(base + sig_off + 4) == SIG[1]
                        && w.read32(base + sig_off + 8) == SIG[2]
                        && w.read32(base + sig_off + 12) == SIG[3]
                    {
                        return Some(base);
                    }
                    base += step;
                }
                None
            };
            scan(SCAN_STEP_COARSE).or_else(|| scan(SCAN_STEP_FINE))
        };

        unsafe {
            for (i, s) in saved.iter().enumerate() {
                fb.add(sig_idx + i).write_volatile(*s);
            }
        }

        let Some(vram_base) = found else {
            crate::serial_println!(
                "[pramin] signature not found anywhere in {} MiB of VRAM", vram_size >> 20
            );
            return None;
        };
        crate::serial_println!(
            "[pramin] scanout surface found at VRAM {:#x} (bar1 offset {:#x})",
            vram_base, loc.offset
        );
        Some(Self {
            vram_base,
            pitch,
            width: fbi.width,
            band_y: fbi.height - band_h,
            band_rows: BAND_ROWS,
            row: 0,
        })
    }

    fn px(&self, x: u32, y: u32) -> u64 {
        self.vram_base + (y as u64) * self.pitch + (x as u64) * 4
    }

    /// Solid bar across the band: a visible "PRAMIN reaches the screen" probe
    /// that needs no font
    pub fn probe(&self, bar0: &MmioRegion) {
        let mut w = Window::new(bar0);
        for y in self.band_y..self.band_y + 4 {
            for x in 0..self.width {
                w.write32(self.px(x, y), 0x00FF_FFFF);
            }
        }
    }

    fn clear_row(&self, w: &mut Window, row: u32) {
        let y0 = self.band_y + row * LINE_HEIGHT;
        for y in y0..y0 + LINE_HEIGHT {
            for x in 0..self.width {
                w.write32(self.px(x, y), 0);
            }
        }
    }

    /// One line of text, wrapping around the band. Each line clears its own
    /// row first, so what is on screen is always the last BAND_ROWS lines
    pub fn line(&mut self, bar0: &MmioRegion, text: &str) {
        let mut w = Window::new(bar0);
        self.line_in(&mut w, text);
    }

    /// The same, into a window the caller already holds, so a caller with a
    /// `try_new` window need not let go and retake it
    fn line_in(&mut self, w: &mut Window, text: &str) {
        let row = self.row % self.band_rows;
        self.row = self.row.wrapping_add(1);
        self.clear_row(w, row);

        let y0 = self.band_y + row * LINE_HEIGHT;
        let cw = font::CHAR_WIDTH as u32;
        let ch = font::CHAR_HEIGHT as u32;
        let max_cols = self.width / cw;
        for (i, c) in text.chars().enumerate() {
            if i as u32 >= max_cols {
                break;
            }
            let Some((glyph, _)) = font::get_glyph(c) else {
                continue;
            };
            let x0 = i as u32 * cw;
            for gy in 0..ch {
                for gx in 0..cw {
                    let v = glyph[(gy * cw + gx) as usize];
                    // Most of a glyph is empty; skipping those cuts the MMIO
                    // traffic per line by roughly two thirds
                    if v == 0 {
                        continue;
                    }
                    let p = v as u32;
                    w.write32(self.px(x0 + gx, y0 + gy), (p << 16) | (p << 8) | p);
                }
            }
        }
    }
}
