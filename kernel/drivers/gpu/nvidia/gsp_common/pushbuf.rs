// Display channel pushbuffer: the method stream a modeset is written into
//
// The driver writes a stream of 32-bit words into a buffer the GPU reads, then
// advances a PUT pointer in the channel's control region; the GPU's GET pointer
// chases it. Everything a modeset does (head timings, which output resource
// drives which head, the scanout surface) is methods in that stream, committed
// by a final UPDATE.
//
// Packet format (`NVC97B_DMA_*`, identical on the Blackwell classes):
//
//     bits 31:29  opcode, 0 = METHOD (address auto-increments per word)
//     bits 27:18  how many data words follow
//     bits 15:2   method address >> 2
//
// one header word followed by `count` data words. Method addresses are byte
// offsets into the class's method space, always 4-byte aligned, hence stored
// shifted.
//
// The control region lives in BAR0, at a fixed offset per channel type; for the
// core channel that is 0x680000. PUT and GET sit at +0x0 and +0x4 and hold a
// byte offset into the pushbuffer.

use crate::drivers::gpu::nvidia::mmio::MmioRegion;

/// BAR0 offset of the core channel's control region, and its size
/// (`r535_chan_user`, class low byte 0x7d)
pub const CORE_CHAN_USER: u32 = 0x0068_0000;
#[allow(dead_code)] // channel-user map: core, window and wimm are driven, cursor is not
pub const CORE_CHAN_USER_SIZE: u32 = 0x1_0000;

/// Window, window-immediate and cursor channels. There are several of each, so
/// the base is indexed by channel instance, for windows the window number, not
/// the head (nouveau's `head` field carries the instance here)
pub const WNDW_CHAN_USER: u32 = 0x0069_0000;
pub const WIMM_CHAN_USER: u32 = 0x006b_0000;
pub const CURS_CHAN_USER: u32 = 0x006d_8000;
pub const PER_CHAN_STRIDE: u32 = 0x1000;

/// Control region of window channel `instance`
#[inline]
pub fn wndw_chan_user(instance: u32) -> u32 {
    WNDW_CHAN_USER + instance * PER_CHAN_STRIDE
}

/// PUT and GET within a channel's control region
const REG_PUT: u32 = 0x0;
const REG_GET: u32 = 0x4;

/// Where a channel's GET stands, without a writer over its buffer.
///
/// For picking up a ring somebody else wrote: on an idle channel GET has caught
/// PUT, so it is the last writer's cursor. A new writer at zero would set PUT
/// behind GET, and the hardware would run the rest of the ring as methods
pub fn channel_get(bar0: &MmioRegion, user: u32) -> usize {
    (bar0.read32(user + REG_GET) >> 2) as usize
}

const OPCODE_METHOD: u32 = 0 << 29;

/// Build a method packet header
#[inline]
fn method_header(method: u32, count: u32) -> u32 {
    OPCODE_METHOD | ((count & 0x3ff) << 18) | (((method >> 2) & 0x3fff) << 2)
}

/// A writer over a display channel's pushbuffer. A dword cursor: PUT counts
/// dwords, and mixing the two runs garbage on the GPU
pub struct PushBuf<'a> {
    words: &'a mut [u32],
    /// Next free dword
    cur: usize,
    /// What PUT was last set to
    put: usize,
    /// BAR0 offset of this channel's control region
    user: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushError {
    /// The stream would run past the end of the buffer
    Full,
    /// GET never caught up with PUT
    Timeout,
}

impl<'a> PushBuf<'a> {
    /// Wrap a buffer. `bytes` must be the mapping of the same memory whose
    /// physical address was given to `set_channel_pushbuf`
    pub fn new(bytes: &'a mut [u8], user: u32) -> Self {
        // The buffer is a DmaBuffer, page-aligned, so the cast is sound; the
        // length rounds down if not a dword multiple
        let n = bytes.len() / 4;
        let words = unsafe {
            core::slice::from_raw_parts_mut(bytes.as_mut_ptr() as *mut u32, n)
        };
        PushBuf { words, cur: 0, put: 0, user }
    }

    /// How much of the pushbuffer PUT can address.
    ///
    /// PUT holds a byte offset in bits 11:2, so it cannot express 4 KiB or
    /// past, however large the registered buffer (ours is 64 KiB). A stream
    /// running past this does not fail: the write truncates, PUT lands earlier
    /// in the ring, GET catches it at once and the channel reports success
    /// having run a fraction of the methods. 1536 dwords is 6144 bytes; bits
    /// 11:2 keep 0x800, so PUT reads as 512 dwords
    const PUT_LIMIT_DWORDS: usize = 0x1000 / 4;

    /// Dwords still free before the buffer wraps
    #[inline]
    pub fn room(&self) -> usize {
        // `MAX_DWORDS`, not `PUT_LIMIT_DWORDS`: the last addressable dword is
        // reserved for a wind's jump. A stream filling it makes `free` compute
        // a length from a cursor past its limit, which underflows and says the
        // ring is empty when full
        let cap = Self::MAX_DWORDS.min(self.words.len());
        cap.saturating_sub(self.cur)
    }

    /// Dwords written so far, for reporting how close a stream is to the limit
    #[inline]
    /// How much of the pushbuffer this stream has filled, for a caller sizing
    /// the next one
    #[allow(dead_code)]
    pub fn used(&self) -> usize {
        self.cur
    }

    /// Emit one method with its data words. `data` may be empty for methods
    /// taking no argument
    pub fn method(&mut self, method: u32, data: &[u32]) -> Result<(), PushError> {
        if self.room() < data.len() + 1 {
            return Err(PushError::Full);
        }
        self.words[self.cur] = method_header(method, data.len() as u32);
        self.cur += 1;
        for &d in data {
            self.words[self.cur] = d;
            self.cur += 1;
        }
        Ok(())
    }

    /// Convenience for the common single-argument case
    #[inline]
    pub fn mthd1(&mut self, method: u32, value: u32) -> Result<(), PushError> {
        self.method(method, &[value])
    }

    /// Publish everything written so far by advancing PUT.
    ///
    /// The buffer is in coherent system memory, so no flush to VRAM (nouveau's
    /// `nv50_dmac_kick` needs one only for pushbuffers in video memory). A
    /// store fence makes the words visible before the GPU is told to read them
    pub fn kick(&mut self, bar0: &MmioRegion) {
        if self.cur == self.put {
            return;
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        // PUT holds a byte offset; the field is 11:2, so a dword index shifted
        // left by two lands in place
        bar0.write32(self.user + REG_PUT, (self.cur as u32) << 2);
        self.put = self.cur;
    }

    /// What PUT was last advanced to, for reporting a stall against GET
    #[inline]
    pub fn put_index(&self) -> usize {
        self.put
    }

    /// Where the GPU has read up to, as a dword index
    pub fn get(&self, bar0: &MmioRegion) -> usize {
        (bar0.read32(self.user + REG_GET) >> 2) as usize
    }

    /// Spin until the GPU has consumed everything published.
    ///
    /// After a modeset, to know the methods were executed. A timeout means the
    /// channel is not running, unlike a method being rejected
    pub fn wait_idle(&self, bar0: &MmioRegion, timeout_ns: u64) -> Result<(), PushError> {
        // TSC, not PTIMER: PRI lockdown windows stall BAR0 timer reads,
        // freezing the timeout
        let khz = crate::kcore::time::tsc_khz().max(1);
        let t0 = crate::kcore::time::rdtsc();
        loop {
            if self.get(bar0) == self.put {
                return Ok(());
            }
            let ns = crate::kcore::time::rdtsc()
                .wrapping_sub(t0)
                .saturating_mul(1_000_000)
                / khz;
            if ns >= timeout_ns {
                return Err(PushError::Timeout);
            }
            core::hint::spin_loop();
        }
    }

    /// Pick a channel's ring back up where the last writer left it.
    ///
    /// The cursor belongs to the channel. A writer starting from zero would set
    /// PUT behind GET, and the hardware would run the whole ring between them
    /// as methods
    pub fn resume(bytes: &'a mut [u8], user: u32, cur: usize) -> Self {
        let mut p = Self::new(bytes, user);
        p.cur = cur.min(Self::MAX_DWORDS);
        p.put = p.cur;
        p
    }

    /// Where to resume from next time
    #[inline]
    pub fn cursor(&self) -> usize {
        self.cur
    }

    /// The ring as PUT can address it, one dword short: nouveau's `dmac->max`.
    /// The last dword is left for a wind's jump
    const MAX_DWORDS: usize = Self::PUT_LIMIT_DWORDS - 1;

    /// A jump back to the start of the ring: opcode 1 in bits 31:29, offset
    /// zero. `NV507C_DMA_OPCODE_JUMP`, unchanged since Tesla
    const DMA_JUMP_TO_ZERO: u32 = 1 << 29;

    /// Dwords the channel can be given before the ring winds.
    ///
    /// `nv50_dmac_free`: while GET is ahead of the cursor the free space is the
    /// gap between them, kept five dwords short as NVIDIA's driver stays five
    /// from GET. Otherwise what is left before the end
    fn free(&self, bar0: &MmioRegion) -> usize {
        let get = self.get(bar0).min(Self::MAX_DWORDS);
        if get > self.cur {
            get.saturating_sub(self.cur + 5)
        } else {
            // Saturating: a cursor past the limit must read as "no room".
            // Unsigned subtraction would wrap to four billion dwords, `reserve`
            // would return at once, and the write would land past what PUT can
            // address (see `PUT_LIMIT_DWORDS`)
            Self::MAX_DWORDS.saturating_sub(self.cur)
        }
    }

    /// Make room for a batch of `dwords`, winding the ring back to the start if
    /// they will not fit before the end.
    ///
    /// A page flip happens sixty times a second, and 1023 dwords is a few
    /// seconds of them. Running off the end is not reported: PUT's field is
    /// 11:2, so the write truncates and the channel runs an earlier part of the
    /// ring.
    ///
    /// `nv50_dmac_wind`: the jump word goes at the cursor uncounted, the cursor
    /// goes to zero, and PUT follows. The channel runs from GET into the jump
    /// and lands on zero, where PUT waits. GET has to have left zero first,
    /// since PUT == GET means the ring is empty
    pub fn reserve(
        &mut self,
        bar0: &MmioRegion,
        dwords: usize,
        timeout_ns: u64,
    ) -> Result<(), PushError> {
        if dwords >= Self::MAX_DWORDS {
            return Err(PushError::Full);
        }
        let khz = crate::kcore::time::tsc_khz().max(1);
        let t0 = crate::kcore::time::rdtsc();
        let elapsed = |t0: u64| {
            crate::kcore::time::rdtsc().wrapping_sub(t0).saturating_mul(1_000_000) / khz
        };

        if self.cur + dwords >= Self::MAX_DWORDS {
            if self.get(bar0) == 0 {
                // The ring is idle at zero with work already written into it
                if self.put == 0 {
                    self.kick(bar0);
                }
                while self.get(bar0) == 0 {
                    if elapsed(t0) >= timeout_ns {
                        return Err(PushError::Timeout);
                    }
                    core::hint::spin_loop();
                }
            }
            self.words[self.cur] = Self::DMA_JUMP_TO_ZERO;
            self.cur = 0;
            self.kick(bar0);
        }

        while self.free(bar0) < dwords {
            if elapsed(t0) >= timeout_ns {
                return Err(PushError::Timeout);
            }
            core::hint::spin_loop();
        }
        Ok(())
    }
}

/// Control region of window-immediate channel `instance`
#[inline]
pub fn wimm_chan_user(instance: u32) -> u32 {
    WIMM_CHAN_USER + instance * PER_CHAN_STRIDE
}

/// Control region of the cursor-immediate channel for `head`.
///
/// The class is PIO, so the region is the method space, not a PUT/GET pair.
/// Only `modeset::cursor` writes it
#[inline]
pub fn curs_chan_user(head: u32) -> u32 {
    CURS_CHAN_USER + head * PER_CHAN_STRIDE
}
