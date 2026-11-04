// GSP first-contact boot for TU116
//
// Engage the GSP Falcon with the signed 'booter_load' HS image and observe the
// result.
//
// The booter HS image is a small bootstrapper. It:
//     1) reads the WPR config from PFB_PRI_MMU_WPR registers,
//     2) finds a GSP-RM image staged in VRAM (by the host driver, signed and
//        locked into WPR by SEC2 ACR),
//     3) fixes up GSP-RM relocation, then jumps into it
// Without a staged image the booter halts early, when WPR registers read zero
// or the GSP-RM image header check fails. The halt status (MAILBOX1) confirms
// the GSP Falcon engaged the upload and the HS signature path ran.
//
// Safety:
//     Halt poll is timeout-bounded. If GSP does not halt in time, a soft reset and GspError::Timeout.
//     pmc::mask_all_interrupts runs first, so GSP-side interrupts stay gated.
//     PMC_ENABLE is not written. If GSP is gated (HWCFG = 0 or all-ones), abort with EngineGated: on Turing the GSP enable lives in NV_PMC_DEVICE_ENABLE, with a chip-specific layout

#![allow(dead_code)]

use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::serial_println;

use crate::drivers::gpu::nvidia::falcon::{
    self, Engine, CPUCTL_HALTED, FALCON_CPUCTL, FALCON_MAILBOX0, FALCON_MAILBOX1, soft_reset,
};
use super::tu116_fw::{self, NvfwBinHdr};

/// Bit set in FALCON_RM (offset 0x84) to request a soft reset of the Falcon.
/// Hardware clears it when the reset completes


/// Halt-poll timeout. The booter runs to completion or halts within
/// microseconds; 100 ms of wall-clock (via PTIMER) is far past that, and burns
/// no seconds of MMIO reads
const HALT_TIMEOUT_NS: u64 = 100_000_000;

#[derive(Debug, Copy, Clone)]
pub enum GspError {
    /// GSP Falcon HWCFG reads as 0 or all-ones: gated by PMC / device-enable /
    /// floorsweep. No ungating is attempted
    EngineGated,
    /// The booter blob did not parse as a valid NVFW container
    BadHeader,
    /// Booter payload exceeds GSP IMEM capacity (would not fit)
    PayloadTooLarge { payload: u32, imem: u32 },
    /// CPU did not assert HALTED within the timeout. The engine was soft-reset;
    /// status registers mean nothing
    Timeout,
}

#[derive(Debug, Copy, Clone)]
pub struct BootStatus {
    /// MAILBOX0 contents at halt. Convention: 0 = success, nonzero = code
    pub mb0: u32,
    /// MAILBOX1 contents at halt. Often carries a stage / sub-error code
    pub mb1: u32,
    /// CPUCTL contents at halt. Bit CPUCTL_HALTED is the primary signal
    pub cpuctl: u32,
}


/// Attempt a first-contact boot of the GSP Falcon using booter_load. Returns
/// the booter's halt status, or an error before CPUCTL is touched
pub fn attempt_boot(bar0: &MmioRegion) -> Result<BootStatus, GspError> {
    attempt_boot_with_arg(bar0, None)
}

/// As 'attempt_boot', but 'Some(phys)' hands the booter that sysmem address
/// (the 'GspFwWprMeta' block) through MAILBOX0/MAILBOX1 before CPUCTL is
/// kicked, the HS-ucode argument convention of nouveau's r535_gsp_booter_load.
/// 'None' clears the mailboxes
pub fn attempt_boot_with_arg(bar0: &MmioRegion, arg: Option<u64>) -> Result<BootStatus, GspError> {
    let gsp = Engine::new(bar0, falcon::PGSP_BASE, "gsp");

    // Engine must be alive before we touch it
    if !gsp.is_alive() {
        serial_println!(
            "[gsp] engine gated: HWCFG={:#x} - aborting (PMC/device-enable bring-up not yet implemented)",
            gsp.read(falcon::FALCON_HWCFG)
        );
        return Err(GspError::EngineGated);
    }

    let imem = gsp.imem_size();
    let dmem = gsp.dmem_size();
    serial_println!(
        "[gsp] engine alive: imem={}B dmem={}B cpuctl_pre={:#010x}",
        imem, dmem, gsp.read(FALCON_CPUCTL)
    );

    // Soft reset to a known state. Failure is non-fatal: the Falcon may be
    // usable from POST
    if !soft_reset(&gsp) {
        serial_println!("[gsp] warn: soft reset did not complete; proceeding anyway");
    }

    // Parse the booter HS container
    let booter_fw = tu116_fw::booter_load_570();
    let blob = booter_fw.bytes();
    let hdr = NvfwBinHdr::parse(blob).ok_or(GspError::BadHeader)?;
    let payload = hdr.data(blob);
    serial_println!(
        "[gsp] booter_load-{}: payload {} bytes (NVFW hdr: header_off={:#x} data_off={:#x})",
        tu116_fw::GSP_DEFAULT_LINE,
        payload.len(),
        hdr.header_offset,
        hdr.data_offset
    );

    if (payload.len() as u32) > imem {
        return Err(GspError::PayloadTooLarge {
            payload: payload.len() as u32,
            imem,
        });
    }

    // PIO upload of the entire payload to IMEM with the SECURE flag. For HS
    // images, internal load-headers tell the Falcon's bootstrap code where its
    // data segment is, so the host does not split code/data
    let uploaded = gsp.imem_load(0, 0, payload, /*secure=*/ true);
    serial_println!("[gsp] uploaded {} bytes to IMEM (SECURE)", uploaded);

    // Mailbox setup. A WPR-meta sysmem address is the booter argument (lo in
    // MB0, hi in MB1). Without one both are cleared, so a written status
    // differs from stale POST values
    match arg {
        Some(phys) => {
            gsp.write(FALCON_MAILBOX0, phys as u32);
            gsp.write(FALCON_MAILBOX1, (phys >> 32) as u32);
            serial_println!(
                "[gsp] booter arg: WPR-meta @ {:#x} (MB0={:#010x} MB1={:#010x})",
                phys, phys as u32, (phys >> 32) as u32
            );
        }
        None => {
            gsp.write(FALCON_MAILBOX0, 0);
            gsp.write(FALCON_MAILBOX1, 0);
        }
    }

    // Kick: bootvec=0 (image starts at IMEM offset 0), STARTCPU + IINVAL
    serial_println!("[gsp] CPUCTL kick: bootvec=0, polling for halt...");
    gsp.start_at(0);

    // Wait for HALTED. On timeout, soft reset and bail out
    if !gsp.wait_halted_ns(HALT_TIMEOUT_NS) {
        let cpuctl_live = gsp.read(FALCON_CPUCTL);
        serial_println!(
            "[gsp] timeout waiting for halt (cpuctl={:#010x}); issuing soft reset",
            cpuctl_live
        );
        soft_reset(&gsp);
        return Err(GspError::Timeout);
    }

    // Halt observed,read status
    let mb0 = gsp.read(FALCON_MAILBOX0);
    let mb1 = gsp.read(FALCON_MAILBOX1);
    let cpuctl = gsp.read(FALCON_CPUCTL);

    serial_println!(
        "[gsp] halted: mb0={:#010x} mb1={:#010x} cpuctl={:#010x} (HALTED bit={})",
        mb0,
        mb1,
        cpuctl,
        if cpuctl & CPUCTL_HALTED != 0 { 1 } else { 0 }
    );

    // A class for the common booter outcomes. The status word format is opaque
    // without its source; this helps triage which precondition is missing
    match mb1 {
        0 => serial_println!(
            "[gsp] booter status: 0 (early exit - typical when WPR is unconfigured / GSP-RM not staged)"
        ),
        _ if crate::drivers::gpu::nvidia::pri::is_falcon(mb1) => serial_println!(
            "[gsp] booter status: WPR / image-header error class ({:#x})", mb1
        ),
        _ => serial_println!(
            "[gsp] booter status: opaque code {:#x} (decode requires booter source)", mb1
        ),
    }

    Ok(BootStatus { mb0, mb1, cpuctl })
}
