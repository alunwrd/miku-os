// The cursor-immediate channel (NVCA7A).
//
// A PIO channel, unlike the DMA channels: the class is
// `NVCA7A_CURSOR_IMM_CHANNEL_PIO` (0xCA7A) and its control region is the method
// space (`Free` at +0x008, `Update` at +0x200, `SetCursorHotSpotPointOut[0]` at
// +0x208), so moving the pointer is two 32-bit stores into BAR0.
//
// A software cursor costs a composition, two copies and a wait for the raster
// boundary. Here the pointer moves when the mouse does, in any part of any
// frame, and the compositor does not learn of it.
//
// The image and the hot spot are core-channel state on the head
// (`core::head_set_cursor`), changing only with the picture.
//
// Reference: `MoveCursorC3` in nvidia-open's
// `src/nvidia-modeset/src/nvkms-cursor3.c`, what `nvEvoCursorCA` binds to on
// GB20x.

use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use super::super::pushbuf::curs_chan_user;

/// `NVCA7A_CURSOR_IMM_CHANNEL_PIO`, the class RM allocates per head
pub const GB202_DISP_CURSOR_IMM_CHANNEL_PIO: u32 = 0xca7a;

/// Free method slots, `NVCA7A_FREE_COUNT` at 5:0
const R_FREE: u32 = 0x008;
/// `NVCA7A_UPDATE`: RELEASE_ELV 0:0, FLIP_LOCK_PIN 8:4
const R_UPDATE: u32 = 0x200;
/// `NVCA7A_SET_INTERLOCK_FLAGS`: INTERLOCK_WITH_CURSOR0..7 at 7:0 and
/// INTERLOCK_WITH_CORE at 16:16
const R_INTERLOCK: u32 = 0x204;
/// `NVCA7A_SET_CURSOR_HOT_SPOT_POINT_OUT(0)`: X 15:0, Y 31:16
const R_POINT_OUT: u32 = 0x208;
/// `NVCA7A_SET_WINDOW_INTERLOCK_FLAGS`, one bit per window
const R_WINDOW_INTERLOCK: u32 = 0x210;

/// `FLIP_LOCK_PIN_LOCK_PIN_NONE`. Nothing here raster-locks, and a channel
/// inheriting a pin waits for an edge that never comes
const UPDATE_NO_LOCK: u32 = 0;
/// `UPDATE_RELEASE_ELV_TRUE` at 0:0
const UPDATE_RELEASE_ELV: u32 = 1;

/// Nothing this channel does is atomic with anything else.
///
/// Both interlock words are written: NVKMS allocates this channel through RM,
/// which zeroes it, while here it starts from what GSP-RM and the firmware
/// left. An update interlocked with a channel nobody updates never completes,
/// like `head_set_mode`'s inherited stall lock
fn clear_interlocks(bar0: &MmioRegion, base: u32) {
    bar0.write32(base + R_INTERLOCK, 0);
    bar0.write32(base + R_WINDOW_INTERLOCK, 0);
}

/// Put the channel where an `Update` waits only for this channel's own work.
/// Called once, when the plane is armed
pub fn cursor_init(bar0: &MmioRegion, head: u32) {
    clear_interlocks(bar0, curs_chan_user(head));
}

/// How many methods the channel accepts now.
///
/// Zero is not an error or a full queue: the display front-end is still
/// processing the previous method. Also the cheapest proof the channel was
/// allocated and the control region decodes: a region nobody owns reads zero
/// forever, so the caller checks at arm time
pub fn free_slots(bar0: &MmioRegion, head: u32) -> u32 {
    bar0.read32(curs_chan_user(head) + R_FREE) & 0x3f
}

/// Spin until the channel has room, at most `timeout_us`.
///
/// NVKMS waits 0.1 s and yields; this runs on an interrupt-time input path with
/// no yield, waiting for one method's front-end work. False on timeout, and the
/// caller drops the move
fn wait_free(bar0: &MmioRegion, head: u32, timeout_us: u64) -> bool {
    if free_slots(bar0, head) != 0 {
        return true;
    }
    let sw = crate::kcore::time::Stopwatch::start();
    while sw.elapsed_us() < timeout_us {
        if free_slots(bar0, head) != 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Put the cursor's hot spot at `(x, y)` on the head and commit.
///
/// The coordinates are where the hot spot lands: the offset to the image's
/// corner is armed on the head. Both fields are signed 16-bit, so a cursor can
/// hang off the left and top edges, and the plane clips.
///
/// False if the channel never made room; the pointer stays for this event and
/// the next one moves it
pub fn move_to(bar0: &MmioRegion, head: u32, x: i32, y: i32) -> bool {
    let base = curs_chan_user(head);
    // Two methods, with a wait before each. One wait is one method: `Free` is a
    // slot count, and a method in a channel with no room makes it stop
    // retiring, which takes the window channel down. `MoveCursorC3` in
    // `nvkms-cursor3.c` writes two, each behind its own `WaitForFreeSpace`. The
    // interlocks are head state, set once by `cursor_init`.
    //
    // 100 us, not NVKMS's 100 ms: on the input path a pointer that stutters
    // beats a machine that stops
    if !wait_free(bar0, head, 100) {
        return false;
    }
    let point = ((x as i16 as u16) as u32) | (((y as i16 as u16) as u32) << 16);
    bar0.write32(base + R_POINT_OUT, point);
    if !wait_free(bar0, head, 100) {
        return false;
    }
    // Under variable refresh the head scans a frame only when released, so a
    // move that does not release waits for the next flip or the panel's minimum
    // refresh. NVKMS releases 33 ms later if no flip came
    // (`nvTriggerVrrUnstallMoveCursor`); on a 48 Hz floor that timer never
    // wins. Released on the move itself (`dev/display-nvkms.md`, decision 6)
    let update = if super::vrr::active_on(head) { UPDATE_NO_LOCK | UPDATE_RELEASE_ELV } else { UPDATE_NO_LOCK };
    bar0.write32(base + R_UPDATE, update);
    true
}

/// Release the ELV block this channel can hold on the head.
///
/// The class has it and `ReleaseElvC3` is paired with every `MoveCursorC3` in a
/// modeset. A move under variable refresh releases on its own UPDATE
/// (`move_to`), so this is for an interlocked flip path only
#[allow(dead_code)] // the interlocked-flip case, which no path here takes yet
pub fn release_elv(bar0: &MmioRegion, head: u32) -> bool {
    if !wait_free(bar0, head, 100) {
        return false;
    }
    bar0.write32(
        curs_chan_user(head) + R_UPDATE,
        UPDATE_NO_LOCK | UPDATE_RELEASE_ELV,
    );
    true
}
