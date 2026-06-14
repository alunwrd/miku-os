// NVDEC VRAM scrubber first-contact for TU116
//
// Goal: engage the NVDEC Falcon with the signed 'nvdec/scrubber' HS image
// and observe the result. The scrubber is the first firmware consumer in
// the documented bring-up order (tu116_fw): it zeroes the region of VRAM
// that SEC2 ACR is about to lock as WPR2, so the ACR sequence starts from
// a known-clean state and the WPR signature check is not fooled by stale
// firmware-leftover bytes.
//
// Why this is "first contact" and not a full scrub:
//   The scrubber HS image reads its scrub descriptor (base address + size
//   of the region to zero, plus a completion handshake word) from its
//   DMEM. nouveau stages that descriptor via the ACR before kicking the
//   engine. We do not model the full ACR DMEM scratch layout yet, so we
//   hand the scrub region through MAILBOX0/MAILBOX1 - the standard HS-ucode
//   argument convention, the same one 'gsp::attempt_boot_with_arg' and
//   'sec2::attempt_acr' use - and let the scrubber halt. The halt status
//   (MAILBOX0/MAILBOX1, CPUCTL.HALTED) is the deliverable: it confirms the
//   NVDEC Falcon engages our upload and runs the HS signature path on real
//   silicon.
//
// Safety:
//    Halt poll is timeout-bounded; on timeout we soft-reset and return
//     'NvdecError::Timeout'.
//     Top-level interrupts are masked by 'pmc::mask_all_interrupts' before
//     this runs, so any NVDEC-side interrupt assertion stays gated in the
//     host LAPIC.
//     We never write PMC_ENABLE / NV_PMC_DEVICE_ENABLE here. On Turing the
//     NVDEC enable bit lives in NV_PMC_DEVICE_ENABLE (BAR0+0x88c), which has
//     a chip-specific layout we do not fully model. If NVDEC reads back as
//     gated we abort with 'EngineGated' rather than guess at the bit.
//
// Sources cross-checked: nouveau (drivers/gpu/drm/nouveau/nvkm/subdev/acr,
// engine/nvdec) and open-gpu-kernel-modules (the scrubber is the
// 'nvdec_scrubber' ucode invoked from acrlib during WPR setup)

#![allow(dead_code)]

use crate::nvidia::mmio::MmioRegion;
use crate::serial_println;

use super::falcon::{
    self, Engine, CPUCTL_HALTED, FALCON_CPUCTL, FALCON_MAILBOX0, FALCON_MAILBOX1, FALCON_RM,
};
use super::tu116_fw::{self, NvfwBinHdr};

/// Bit set in FALCON_RM (offset 0x84) to request a soft reset. Hardware
/// clears it when the reset completes. Same convention as gsp.rs
const FALCON_RM_RESET: u32 = 1 << 1;

/// Halt-poll timeout. The scrubber either runs the zeroing loop to
/// completion or halts early on a missing descriptor within microseconds;
/// 100 ms of wall-clock is far past any legitimate completion time and,
/// unlike a raw spin count, does not burn seconds of MMIO on a failed boot
const HALT_TIMEOUT_NS: u64 = 100_000_000;

#[derive(Debug, Copy, Clone)]
pub enum NvdecError {
    /// NVDEC Falcon HWCFG reads as 0, all-ones, or the PRI sentinel;
    /// engine is gated in NV_PMC_DEVICE_ENABLE or floor-swept. We do not
    /// attempt to ungate
    EngineGated,
    /// The scrubber blob did not parse as a valid NVFW container
    BadHeader,
    /// Scrubber payload exceeds NVDEC IMEM capacity (would not fit)
    PayloadTooLarge { payload: u32, imem: u32 },
    /// CPU did not assert HALTED within the timeout. Engine has been
    /// soft-reset; status registers are not meaningful
    Timeout,
}

#[derive(Debug, Copy, Clone)]
pub struct ScrubStatus {
    /// MAILBOX0 at halt. Convention: 0 = success, nonzero = error/stage code
    pub mb0: u32,
    /// MAILBOX1 at halt. Often carries a stage / sub-error code
    pub mb1: u32,
    /// CPUCTL at halt. Bit CPUCTL_HALTED is the primary signal
    pub cpuctl: u32,
}

/// The region of VRAM the scrubber should zero, in absolute VRAM byte
/// offsets. Built from a 'WprLayout' (the WPR2 span) or supplied directly
#[derive(Debug, Copy, Clone)]
pub struct ScrubRegion {
    /// Start offset inside VRAM (FB physical / BAR1-relative)
    pub start: u64,
    /// Length in bytes
    pub size: u64,
}

/// Issue a soft reset to the NVDEC Falcon. Spins up to ~100k iterations
/// waiting for hardware to clear the request bit. Returns true on success
fn soft_reset(eng: &Engine) -> bool {
    eng.write(FALCON_RM, FALCON_RM_RESET);
    for _ in 0..100_000 {
        if eng.read(FALCON_RM) & FALCON_RM_RESET == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Run an NVDEC scrubber first-contact boot with no scrub descriptor.
/// Mailboxes are cleared so a written status is distinguishable from stale
/// POST values. On success returns the scrubber's halt status; on any
/// pre-execution failure returns an error without ever touching CPUCTL
pub fn attempt_scrub(bar0: &MmioRegion) -> Result<ScrubStatus, NvdecError> {
    attempt_scrub_region(bar0, None)
}

/// As 'attempt_scrub', but if 'region' is 'Some', the scrub base (in MB0)
/// and size (in MB1) are handed to the scrubber as its argument before
/// CPUCTL is kicked. Until the full DMEM scrub-descriptor layout is
/// modelled the scrubber will most likely still halt early reading the
/// remaining descriptor fields as zero - but the mailbox handoff and the
/// HS signature path are real and reusable
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

    // Soft reset to a known state. Non-fatal: the Falcon may already be
    // usable straight from POST
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
    // image's internal load-headers tell the Falcon bootstrap where its
    // data segment lives, so the host does not split code/data here -
    // identical to the SEC2 acr/bl and GSP booter_load uploads
    let uploaded = nvdec.imem_load(0, 0, payload, /*secure=*/ true);
    serial_println!("[nvdec] uploaded {} bytes to IMEM (SECURE)", uploaded);

    // Mailbox setup. With a scrub region, pass base in MB0 and size in MB1;
    // without one, clear both
    match region {
        Some(r) => {
            // Region offsets are VRAM byte offsets. First-contact only needs
            // the handoff to be deterministic, so pass the low 32 bits raw
            // and let the early-exit status confirm the wiring
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

    // Sketch a class for the common scrubber outcomes. The exact status
    // word format is opaque without the scrubber source; this just helps a
    // human reading the kernel log triage which precondition is missing
    match mb0 {
        0 => serial_println!(
            "[nvdec] scrubber status: 0 (clean exit - region either zeroed or descriptor empty)"
        ),
        _ if mb0 & 0xFFFF_0000 == 0xBADF_0000 => serial_println!(
            "[nvdec] scrubber status: PRI / descriptor error class ({:#x})", mb0
        ),
        _ => serial_println!(
            "[nvdec] scrubber status: opaque code {:#x} (decode requires scrubber source)", mb0
        ),
    }

    Ok(ScrubStatus { mb0, mb1, cpuctl })
}
