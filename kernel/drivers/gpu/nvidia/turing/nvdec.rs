// NVDEC VRAM scrubber first-contact for TU116
//
// Engage the NVDEC Falcon with the signed 'nvdec/scrubber' HS image and observe
// the result. The scrubber is the first firmware consumer in the bring-up order
// (tu116_fw): it zeroes the VRAM region SEC2 ACR will lock as WPR2, so the ACR
// sequence starts clean and stale bytes do not fool the WPR signature check.
//
// The scrubber HS image reads its descriptor (base address + size of the
// region, plus a completion handshake word) from DMEM; nouveau stages that via
// the ACR. Here the region goes through MAILBOX0/MAILBOX1, the HS-ucode
// argument convention of 'gsp::attempt_boot_with_arg' and 'sec2::attempt_acr',
// and the scrubber halts. The halt status (MAILBOX0/MAILBOX1, CPUCTL.HALTED)
// confirms the NVDEC Falcon engages the upload and runs the HS signature path.
//
// Safety:
//     Halt poll is timeout-bounded; on timeout a soft reset and 'NvdecError::Timeout'.
//     'pmc::mask_all_interrupts' runs first, so NVDEC-side interrupts stay gated.
//     PMC_ENABLE / NV_PMC_DEVICE_ENABLE are not written. On Turing the NVDEC enable bit lives in NV_PMC_DEVICE_ENABLE (BAR0+0x88c), with a chip-specific layout. If NVDEC reads as gated, abort with 'EngineGated'.
//
// Sources: nouveau (drivers/gpu/drm/nouveau/nvkm/subdev/acr, engine/nvdec) and
// open-gpu-kernel-modules (the 'nvdec_scrubber' ucode invoked from acrlib
// during WPR setup)

#![allow(dead_code)]

use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::serial_println;

use crate::drivers::gpu::nvidia::falcon::{
    self, Engine, CPUCTL_HALTED, FALCON_CPUCTL, FALCON_MAILBOX0, FALCON_MAILBOX1, soft_reset,
};
use super::tu116_fw::{self, NvfwBinHdr};

/// Bit set in FALCON_RM (offset 0x84) to request a soft reset. Hardware clears
/// it when the reset completes, as in gsp.rs


/// Halt-poll timeout. The scrubber finishes its zeroing loop or halts early on
/// a missing descriptor within microseconds; 100 ms of wall-clock is far past
/// that, burning no seconds of MMIO on a failed boot
const HALT_TIMEOUT_NS: u64 = 100_000_000;

#[derive(Debug, Copy, Clone)]
pub enum NvdecError {
    /// NVDEC Falcon HWCFG reads as 0, all-ones, or the PRI sentinel: gated in
    /// NV_PMC_DEVICE_ENABLE or floor-swept. No ungating is attempted
    EngineGated,
    /// The scrubber blob did not parse as a valid NVFW container
    BadHeader,
    /// Scrubber payload exceeds NVDEC IMEM capacity (would not fit)
    PayloadTooLarge { payload: u32, imem: u32 },
    /// CPU did not assert HALTED within the timeout. The engine was soft-reset;
    /// status registers mean nothing
    Timeout,
}

#[derive(Debug, Copy, Clone)]
pub struct ScrubStatus {
    /// MAILBOX0 at halt. 0 = success, nonzero = error/stage code
    pub mb0: u32,
    /// MAILBOX1 at halt. Often carries a stage / sub-error code
    pub mb1: u32,
    /// CPUCTL at halt. Bit CPUCTL_HALTED is the primary signal
    pub cpuctl: u32,
}

/// The region of VRAM the scrubber should zero, in absolute VRAM byte offsets.
/// From a 'WprLayout' (the WPR2 span) or supplied directly
#[derive(Debug, Copy, Clone)]
pub struct ScrubRegion {
    /// Start offset inside VRAM (FB physical / BAR1-relative)
    pub start: u64,
    /// Length in bytes
    pub size: u64,
}


/// Run an NVDEC scrubber first-contact boot with no scrub descriptor. Mailboxes
/// are cleared so a written status differs from stale POST values. Returns the
/// scrubber's halt status, or an error before CPUCTL is touched
pub fn attempt_scrub(bar0: &MmioRegion) -> Result<ScrubStatus, NvdecError> {
    attempt_scrub_region(bar0, None)
}

/// As 'attempt_scrub', but 'Some' hands the scrub base (in MB0) and size (in
/// MB1) to the scrubber before CPUCTL is kicked. Without the DMEM
/// scrub-descriptor layout it probably halts early, reading the remaining
/// fields as zero
pub fn attempt_scrub_region(
    bar0: &MmioRegion,
    region: Option<ScrubRegion>,
) -> Result<ScrubStatus, NvdecError> {
    let nvdec = Engine::new(bar0, falcon::PNVDEC_BASE, "nvdec");

    // Engine must be alive before we touch it
    if !nvdec.is_alive() {
        serial_println!(
            "[nvdec] engine gated: HWCFG={:#x} - aborting (NV_PMC_DEVICE_ENABLE bring-up not yet implemented)",
            nvdec.read(falcon::FALCON_HWCFG)
        );
        return Err(NvdecError::EngineGated);
    }

    let imem = nvdec.imem_size();
    let dmem = nvdec.dmem_size();
    serial_println!(
        "[nvdec] engine alive: imem={}B dmem={}B cpuctl_pre={:#010x}",
        imem, dmem, nvdec.read(FALCON_CPUCTL)
    );

    // Soft reset to a known state. Non-fatal: the Falcon may be usable from
    // POST
    if !soft_reset(&nvdec) {
        serial_println!("[nvdec] warn: soft reset did not complete; proceeding anyway");
    }

    // Parse the scrubber HS container
    let scrubber_fw = tu116_fw::nvdec_scrubber();
    let blob = scrubber_fw.bytes();
    let hdr = NvfwBinHdr::parse(blob).ok_or(NvdecError::BadHeader)?;
    let payload = hdr.data(blob);
    serial_println!(
        "[nvdec] scrubber: payload {} bytes (NVFW hdr: header_off={:#x} data_off={:#x})",
        payload.len(), hdr.header_offset, hdr.data_offset
    );

    if (payload.len() as u32) > imem {
        return Err(NvdecError::PayloadTooLarge {
            payload: payload.len() as u32,
            imem,
        });
    }

    // PIO upload the entire payload to IMEM with the SECURE flag. The HS
    // image's internal load-headers tell the Falcon bootstrap where its data
    // segment lives, as in the SEC2 acr/bl and GSP booter_load uploads
    let uploaded = nvdec.imem_load(0, 0, payload, /*secure=*/ true);
    serial_println!("[nvdec] uploaded {} bytes to IMEM (SECURE)", uploaded);

    // Mailbox setup. With a scrub region, base in MB0 and size in MB1; without,
    // both cleared
    match region {
        Some(r) => {
            // Region offsets are VRAM byte offsets. Low 32 bits raw: the
            // early-exit status confirms the wiring
            nvdec.write(FALCON_MAILBOX0, r.start as u32);
            nvdec.write(FALCON_MAILBOX1, r.size as u32);
            serial_println!(
                "[nvdec] scrub region: start={:#x} size={:#x} (MB0={:#010x} MB1={:#010x})",
                r.start, r.size, r.start as u32, r.size as u32
            );
        }
        None => {
            nvdec.write(FALCON_MAILBOX0, 0);
            nvdec.write(FALCON_MAILBOX1, 0);
        }
    }

    // Kick: bootvec=0 (image starts at IMEM offset 0), STARTCPU + IINVAL
    serial_println!("[nvdec] CPUCTL kick: bootvec=0, polling for halt...");
    nvdec.start_at(0);

    // Wait for HALTED. On timeout, soft reset and bail out
    if !nvdec.wait_halted_ns(HALT_TIMEOUT_NS) {
        let cpuctl_live = nvdec.read(FALCON_CPUCTL);
        serial_println!(
            "[nvdec] timeout waiting for halt (cpuctl={:#010x}); issuing soft reset",
            cpuctl_live
        );
        soft_reset(&nvdec);
        return Err(NvdecError::Timeout);
    }

    // Halt observed - read status
    let mb0 = nvdec.read(FALCON_MAILBOX0);
    let mb1 = nvdec.read(FALCON_MAILBOX1);
    let cpuctl = nvdec.read(FALCON_CPUCTL);

    serial_println!(
        "[nvdec] halted: mb0={:#010x} mb1={:#010x} cpuctl={:#010x} (HALTED bit={})",
        mb0, mb1, cpuctl,
        if cpuctl & CPUCTL_HALTED != 0 { 1 } else { 0 }
    );

    // A class for the common scrubber outcomes. The status word format is
    // opaque without the source; this helps triage which precondition is
    // missing
    match mb0 {
        0 => serial_println!(
            "[nvdec] scrubber status: 0 (clean exit - region either zeroed or descriptor empty)"
        ),
        _ if crate::drivers::gpu::nvidia::pri::is_falcon(mb0) => serial_println!(
            "[nvdec] scrubber status: PRI / descriptor error class ({:#x})", mb0
        ),
        _ => serial_println!(
            "[nvdec] scrubber status: opaque code {:#x} (decode requires scrubber source)", mb0
        ),
    }

    Ok(ScrubStatus { mb0, mb1, cpuctl })
}
