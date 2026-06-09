// GSP first-contact boot for TU116
//
// Goal: engage the GSP Falcon with the signed 'booter_load' HS image,
// observe the result
//
// Why this is "first contact" and not full GSP-RM boot:
//   The booter HS image is a small bootstrapper. It expects to:
//     1) read the WPR config from PFB_PRI_MMU_WPR registers,
//     2) find a GSP-RM image already staged in VRAM (placed there by the
//        host driver, signed and locked into WPR by SEC2 ACR),
//     3) fix up GSP-RM relocation, then jump into it
//   Our blob set under tu116/ contains the booter but NO GSP-RM image
//   (no gsp_t.bin or equivalent). And we have no SEC2 ACR boot yet, no
//   WPR, no DMA buffer allocator, no VRAM allocator. The booter is
//   therefore expected to halt early - either because WPR registers
//   read zero or because the GSP-RM image header check fails. The halt
//   status (MAILBOX1) is the deliverable: it confirms that the GSP
//   Falcon is engaging our upload and that the HS signature path is
//   running on real hardware
//
// Safety:
//     Halt poll is timeout-bounded. If GSP does not halt within the
//     timeout we issue a soft reset and return GspError::Timeout.
//     Top-level interrupts are already masked by pmc::mask_all_interrupts
//     before this runs, so any GSP-side interrupt assertion stays gated
//     in the host LAPIC.
//     We never write to PMC_ENABLE here. If GSP is gated (HWCFG = 0
//     or all-ones), we abort with EngineGated rather than guess at the
//     correct enable bit - on Turing the GSP enable lives in
//     NV_PMC_DEVICE_ENABLE which has a chip-specific layout we do not
//     fully model yet
//
// What lands as a follow-up to make this fully boot:
//     PMC_ENABLE / NV_PMC_DEVICE_ENABLE programming for GSP
//     SEC2 ACR boot (needs a DMA buffer allocator) to set up WPR
//     VRAM staging of a GSP-RM image (we do not ship one)

#![allow(dead_code)]

use crate::nvidia::mmio::MmioRegion;
use crate::serial_println;

use super::falcon::{
    self, Engine, CPUCTL_HALTED, FALCON_CPUCTL, FALCON_MAILBOX0, FALCON_MAILBOX1, FALCON_RM,
};
use super::tu116_fw::{self, NvfwBinHdr};

/// Bit set in FALCON_RM (offset 0x84) to request a soft reset of the Falcon
/// Hardware clears it when the reset completes
const FALCON_RM_RESET: u32 = 1 << 1;

/// Halt-poll timeout. The booter normally either runs to completion or
/// halts within microseconds; 100 ms of wall-clock (via PTIMER) is far past
/// any legitimate completion time and, unlike a raw spin count, does not
/// burn seconds of MMIO reads on a boot that never halts
const HALT_TIMEOUT_NS: u64 = 100_000_000;

#[derive(Debug, Copy, Clone)]
pub enum GspError {
    /// GSP Falcon HWCFG reads as 0 or all-ones - engine is gated by
    /// PMC / device-enable / floorsweep. We do not attempt to ungate
    EngineGated,
    /// The booter blob did not parse as a valid NVFW container
    BadHeader,
    /// Booter payload exceeds GSP IMEM capacity (would not fit)
    PayloadTooLarge { payload: u32, imem: u32 },
    /// CPU did not assert HALTED within the timeout. Engine has been
    /// soft-reset; status registers are not meaningful
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

/// Issue a soft reset to a Falcon engine. Spins up to ~100k iterations
/// waiting for hardware to clear the request bit. Returns true if reset completed cleanly
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

/// Attempt a first-contact boot of the GSP Falcon using booter_load.
/// On success returns the booter's halt status. On any pre-execution
/// failure returns an error without ever touching CPUCTL
pub fn attempt_boot(bar0: &MmioRegion) -> Result<BootStatus, GspError> {
    attempt_boot_with_arg(bar0, None)
}

/// As 'attempt_boot', but if 'arg' is 'Some(phys)'' the booter is handed
/// that sysmem address (the 'GspFwWprMeta' block) through MAILBOX0/MAILBOX1
/// before CPUCTL is kicked, following the standard HS-ucode argument convention
/// matching nouveau's r535_gsp_booter_load. With 'None' the mailboxes are
/// just cleared (first-contact / no WPR meta available)
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

    // Soft reset to a known state. Failure here is non-fatal; the
    // Falcon may already be in a usable state from POST
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

    // PIO upload of the entire payload to IMEM with the SECURE flag.
    // For HS images, internal load-headers tell the Falcon's own
    // bootstrap code where to find its data segment within the image,
    // so we do not need to split code/data on the host side
    let uploaded = gsp.imem_load(0, 0, payload, /*secure=*/ true);
    serial_println!("[gsp] uploaded {} bytes to IMEM (SECURE)", uploaded);

    // Mailbox setup. With a WPR-meta sysmem address, pass it as the booter
    // argument (lo in MB0, hi in MB1); the booter reads its config from
    // there. Without one, just clear both so a written status is
    // distinguishable from stale POST values
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

    // Sketch a class for the most common booter outcomes. The booter's
    // exact status word format is opaque to us without its source; this
    // just helps a human reading the kernel log triage which precondition
    // is missing
    match mb1 {
        0 => serial_println!(
            "[gsp] booter status: 0 (early exit - typical when WPR is unconfigured / GSP-RM not staged)"
        ),
        _ if mb1 & 0xFFFF_0000 == 0xBADF_0000 => serial_println!(
            "[gsp] booter status: WPR / image-header error class ({:#x})", mb1
        ),
        _ => serial_println!(
            "[gsp] booter status: opaque code {:#x} (decode requires booter source)", mb1
        ),
    }

    Ok(BootStatus { mb0, mb1, cpuctl })
}
