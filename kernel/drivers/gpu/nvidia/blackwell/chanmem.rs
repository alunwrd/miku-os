// Video-memory carves and channel virtual addresses, shared by `channel.rs` and
// `compute_chan.rs`, so those two do not import each other (see the cycle note
// in `mod.rs`).
//
// Who takes these
//
// All are written once, during `boot_opts`, on the shell thread, and read
// afterwards.
//
// They are not merged into one lock: `unstick_engines` runs on the compositor
// thread (`mayland/render/present.rs`) while the shell thread may hold
// something else, and a coarser lock there is a deadlock.





/// `bar1PdeBase` from the GSP static config, and where BAR1 lives.
///
/// One page of video memory for the root page directory of the channel's
/// address space. Carved when the FB layout is known, used when the RM client
/// exists
pub static CHAN_PDB: spin::Mutex<u64> = spin::Mutex::new(0);

/// Video memory for the channel's instance block and USERD, carved alongside
/// the page directory
pub static CHAN_INST: spin::Mutex<u64> = spin::Mutex::new(0);

/// Instance block and USERD for the compute channel, its own: two channels
/// sharing one share a USERD slot, and one channel's state runs into the
/// other's
pub static CHAN_INST_COMPUTE: spin::Mutex<u64> = spin::Mutex::new(0);

/// Two pages of video memory nothing else uses, for the copy engine's first
/// job: a source and a destination
pub static CHAN_SCRATCH: spin::Mutex<u64> = spin::Mutex::new(0);

/// GSP-RM's own client, device and subdevice handles. The method buffer size is
/// a question about the chip's copy engines, and `r535_fifo_runl_ctor` asks it
/// on the internal subdevice
pub static CHAN_INTERNAL: spin::Mutex<Option<(u32, u32, u32)>> = spin::Mutex::new(None);

/// The same triple as an `RmClient`, for anything that wants to issue an
/// NV2080_CTRL_CMD_INTERNAL_* control after the boot has returned. `None`
/// until `enumerate_displays` has read the static config
pub fn internal_client() -> Option<crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient> {
    CHAN_INTERNAL.lock().map(|(c, d, s)| {
        crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient {
            h_client: c,
            h_device: d,
            h_subdevice: s,
        }
    })
}

/// This driver's own client, device and subdevice handles. A display
/// reconfigured after boot (a mode change, a monitor going to sleep, a cable
/// arriving) needs an RM client. A second client chain per command would leak
/// one on every failure
pub static RM_CLIENT: spin::Mutex<Option<(u32, u32, u32)>> = spin::Mutex::new(None);

/// The driver's client, for post-boot RM calls. `None` until the boot has
/// reached `RmClient::bootstrap`
pub fn rm_client() -> Option<crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient> {
    RM_CLIENT.lock().map(|(c, d, s)| {
        crate::drivers::gpu::nvidia::gsp_common::rmapi::RmClient {
            h_client: c,
            h_device: d,
            h_subdevice: s,
        }
    })
}

/// The `GB202_DISP` root object every display channel is allocated under.
///
/// Lighting a second monitor after boot allocates its window and
/// window-immediate channels under the root created once at boot
pub static DISP_ROOT: spin::Mutex<Option<u32>> = spin::Mutex::new(None);

/// GSP-RM was unloaded: every handle here was one of its objects, and every
/// address was carved from a layout it gave. The next boot carves and allocates
/// again (`bringup::rm`)
pub(super) fn forget() {
    *CHAN_PDB.lock() = 0;
    *CHAN_INST.lock() = 0;
    *CHAN_INST_COMPUTE.lock() = 0;
    *CHAN_SCRATCH.lock() = 0;
    CHAN_INTERNAL.lock().take();
    RM_CLIENT.lock().take();
    DISP_ROOT.lock().take();
}

/// One window per channel for its ring, pushbuffer and semaphore.
///
/// The channel id picks the window, so an allocated channel gets its addresses,
/// and two channels cannot overlap.
///
/// At 1 GiB: everything else in this address space (the graphics contexts, the
/// programs, their constant banks, the surfaces the effects pass reads) runs
/// from 16 MiB to 800 MiB. These mappings need more than one index in the
/// levels above the page table
pub const CHAN_WINDOW_BASE: u64 = 0x4000_0000;
pub const CHAN_WINDOW_STRIDE: u64 = 0x0100_0000;

/// Ring, pushbuffer and semaphore for `chid`, far enough apart that neither
/// grows into the next
pub fn gpfifo_va(chid: u32) -> u64 {
    CHAN_WINDOW_BASE + chid as u64 * CHAN_WINDOW_STRIDE
}
pub fn pushbuf_va(chid: u32) -> u64 {
    gpfifo_va(chid) + 0x0010_0000
}
/// One page for the completion semaphore, in system memory: the CPU can read it
/// without PRAMIN
pub fn sema_va(chid: u32) -> u64 {
    gpfifo_va(chid) + 0x0020_0000
}
pub const GPFIFO_BYTES: u64 = 0x1000;
pub const PUSHBUF_BYTES: u64 = 0x1_0000;
pub const SEMA_BYTES: u64 = 0x1000;

/// Channel ids the chid allocator hands out (`chan::alloc_chid`)
const CHANNEL_IDS: u64 = 64;

/// Every address this driver maps anything at, as the named reservations of an
/// address space (`vaspace::create`, `dev/gpu-memory-compute.md` 3.2).
///
/// A new user of the space reserves around these, and two landing on one
/// address is a refusal at creation
pub fn layout() -> [(&'static str, u64, u64); 8] {
    use crate::drivers::gpu::nvidia::blur;
    use crate::drivers::gpu::nvidia::gsp_common::compute as c;
    [
        ("graphics context", c::CTX_BASE_VA, c::CTX_LIMIT_VA - c::CTX_BASE_VA),
        ("programs", c::PROGRAM_VA, c::CONST0_VA - c::PROGRAM_VA),
        ("constant banks", c::CONST0_VA, c::QMD_VA - c::CONST0_VA),
        ("launch descriptors", c::QMD_VA, c::SCRATCH_VA - c::QMD_VA),
        ("scratch", c::SCRATCH_VA, 0x1000),
        ("effects surfaces", c::SURFACE_VA, blur::VA_END - c::SURFACE_VA),
        ("channel windows", CHAN_WINDOW_BASE, CHANNEL_IDS * CHAN_WINDOW_STRIDE),
        ("client binds", c::CLIENT_VA_BASE, c::MAX_CLIENT_BINDS as u64 * c::CLIENT_VA_STRIDE),
    ]
}
