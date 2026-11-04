// What a GB20x does to boot GSP-RM, once everything is staged
//
// `kgspBootstrap_GH100` (`kernel_gsp_gh100.c:801`) is what every GB20x runs:
// the COT to the FSP, the wait for the FMC to release the falcon, the checks
// that it started, and FALCON_OS. Before it, `pre_cot`: the GFW progress read
// (`kgspWaitForGfwBootOk`, not a gate on FSP-managed chips), the FMC error
// channel cleared so a later code is this boot's
// (`gpuResetGspFmcErrorCode_GH100`, the first statement of
// `kgspBootstrap_GH100`), and the FSP's version for the log.
//
// Before the staging, as in `kgspInitRm_IMPL`: the FSP's secure-boot wait
// (`kgspWaitForGfwBootOk_GH100` -> `kfspWaitForSecureBoot_GB202`), the GPU
// timer set to wall-clock time (`tmrSetCurrentTime_GH100`), and the WPR2 check
// `_kgspBootGspRm` starts with (`kgspIsWpr2Up_GH100`).

use crate::serial_println;

use super::bringup::stage::Staged;
use super::fsp::Fsp;
use super::gspboot::{
    fmc_error, lockdown_step, phase, print_fmc_error, BootError, BootReport, LOCKDOWN_TIMEOUT_NS,
};
use super::regs::*;

/// How long the FSP gets to finish its own boot: 5 s, on the OS timer
/// (`kfspWaitForSecureBoot_GB202`, `kern_fsp_gb202.c:60`). The GPU's timer is
/// not usable yet
const SECURE_BOOT_TIMEOUT_MS: u64 = 5000;

/// Wait for the FSP to report its secure boot complete
/// (`kfspWaitForSecureBoot_GB202`). Returns how long it took.
///
/// OpenRM compares the whole register with 0xFF. A whole register disagreeing
/// with the low byte is logged and accepted (`dev/gsp-foundation.md`)
pub(super) fn wait_secure_boot(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
) -> Result<u64, BootError> {
    let sw = crate::kcore::time::Stopwatch::start();
    loop {
        let v = bar0.read32(THERM_I2CS_SCRATCH);
        if v & 0xFF == FSP_BOOT_COMPLETE_SUCCESS {
            if v != FSP_BOOT_COMPLETE_SUCCESS {
                serial_println!(
                    "[gb206/boot] FSP boot scratch {:#010x}: low byte says done, OpenRM would want exactly 0xff",
                    v
                );
            }
            return Ok(sw.elapsed_ms());
        }
        if sw.elapsed_ms() >= SECURE_BOOT_TIMEOUT_MS {
            // The same five words OpenRM logs on this timeout
            serial_println!(
                "[gb206/boot] FSP secure boot not complete after {} ms: therm {:#010x} scratch2 {:#010x} {:#010x} {:#010x} {:#010x}",
                SECURE_BOOT_TIMEOUT_MS, v,
                bar0.read32(PFSP_COMMON_SCRATCH_GROUP_2_0),
                bar0.read32(PFSP_COMMON_SCRATCH_GROUP_2_0 + 4),
                bar0.read32(PFSP_COMMON_SCRATCH_GROUP_2_0 + 8),
                bar0.read32(PFSP_COMMON_SCRATCH_GROUP_2_0 + 12),
            );
            return Err(BootError::SecureBootTimeout(v));
        }
        crate::kcore::time::udelay(100);
    }
}

/// Is a write-protected region still up? (`kgspIsWpr2Up_GH100`, which is
/// `kgspIsWpr2Up_TU102` without confidential compute). A GSP-RM booted and
/// never unloaded leaves one, and OpenRM refuses to boot over it
/// (`_kgspBootGspRm`): "the GPU is likely in a bad state and may need to be
/// reset". Returns the register
pub fn wpr2_hi(bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion) -> u32 {
    bar0.read32(PFB_PRI_MMU_WPR2_ADDR_HI)
}

/// `_VAL` is 31:4
pub fn is_wpr2_up(wpr2_hi: u32) -> bool {
    wpr2_hi >> 4 != 0
}

/// Set the GPU's time to the wall-clock time, and return the offset
/// (`tmrSetCurrentTime_GH100`, `timer_gh100.c:49`).
///
/// The secure timer counts from power-on; the offset to the OS's clock goes
/// into `SYS_TIMER_OFFSET`, which moves PTIMER, and into
/// `GspSystemInfo.sysTimerOffsetNs`. OpenRM does this after the GFW wait, "to
/// avoid PLM collisions", and before GSP-RM is loaded, since GSP-RM reads the
/// updated time. The FSP bug making the write unsafe
/// (`kfspRequiresBug3957833WAR`) is GH100's alone.
///
/// Returns 0 without writing when the kernel does not know the time, or when
/// the secure timer is ahead of it (OpenRM asserts on the second and its caller
/// ignores the status)
pub(super) fn set_current_time(bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion) -> u64 {
    use crate::kcore::clock;
    if !clock::is_set() {
        serial_println!("[gb206/boot] GPU time not set: the kernel has no wall clock yet");
        return 0;
    }
    let os_ns = clock::now_unix() * 1_000_000_000 + clock::now_nanos_frac();

    // hi-lo-hi, so the two halves belong to the same instant
    let mut hi2 = bar0.read32(PGC6_SCI_SEC_TIMER_TIME_1);
    let (lo, hi) = loop {
        let hi = hi2;
        let lo = bar0.read32(PGC6_SCI_SEC_TIMER_TIME_0);
        hi2 = bar0.read32(PGC6_SCI_SEC_TIMER_TIME_1);
        if hi == hi2 {
            break (lo, hi);
        }
    };
    let sec_ns = lo as u64 | (hi as u64) << 32;
    if sec_ns >= os_ns {
        serial_println!(
            "[gb206/boot] GPU time not set: secure timer {:#x} ns is ahead of the wall clock {:#x} ns",
            sec_ns, os_ns
        );
        return 0;
    }
    let offset = os_ns - sec_ns;
    let before = bar0.read32(crate::drivers::gpu::nvidia::regs::PTIMER_TIME_1);
    bar0.write32(PGC6_SCI_SYS_TIMER_OFFSET_1, (offset >> 32) as u32);
    bar0.write32(PGC6_SCI_SYS_TIMER_OFFSET_0, offset as u32 | SYS_TIMER_OFFSET_UPDATE_TRIGGER);
    serial_println!(
        "[gb206/boot] GPU time set: secure timer {} s, offset {:#x} ns; PTIMER hi {:#x} -> {:#x}",
        sec_ns / 1_000_000_000, offset, before,
        bar0.read32(crate::drivers::gpu::nvidia::regs::PTIMER_TIME_1)
    );
    offset
}

/// How long the FSP may take to drop the BAR firewall after a reset: 500 ms on
/// the OS timer (`kbifPollBarFirewallDisengage_GB202`)
const BAR_FIREWALL_TIMEOUT_MS: u64 = 500;

/// Wait for the FSP to disengage the BAR firewall after a reset
/// (`kbifPollBarFirewallDisengage_GB202`, `kernel_bif_gb202.c:172`). Returns
/// the milliseconds it took, or the last value read.
///
/// OpenRM reads the register with a configuration cycle. Here the read goes
/// through BAR0's mirror of config space. While the firewall is up that read
/// may be refused (a PRI error or all ones), which counts as still engaged
pub fn poll_bar_firewall(bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion) -> Result<u64, u32> {
    let sw = crate::kcore::time::Stopwatch::start();
    loop {
        let v = bar0.read32(EP_PCFGM_BASE + VSEC_DEBUG_SEC_2);
        let refused = v == u32::MAX || crate::drivers::gpu::nvidia::pri::is_target_locked(v);
        if !refused && v & BAR_FIREWALL_ENGAGE == 0 {
            return Ok(sw.elapsed_ms());
        }
        if sw.elapsed_ms() >= BAR_FIREWALL_TIMEOUT_MS {
            crate::kerror!("[gb206] BAR firewall still engaged after {} ms: {:#010x}", BAR_FIREWALL_TIMEOUT_MS, v);
            return Err(v);
        }
        crate::kcore::time::udelay(100);
    }
}

/// Before the COT: the GFW progress, the FMC error channel, the FSP's version
pub(super) fn pre_cot(bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion) {
    // GFW (devinit) boot progress gate. GSP-RM asserts on this scratch during
    // its own init, so surface the value before the COT and give a slow GFW a
    // few seconds (OpenRM kgspWaitForGfwBootOk does the same wait)
    let plm = bar0.read32(PGC6_AON_SECURE_SCRATCH_GROUP_05_PLM);
    let mut gfw = bar0.read32(PGC6_AON_SECURE_SCRATCH_GROUP_05_0);
    if gfw & 0xFF != GFW_BOOT_PROGRESS_COMPLETED {
        // Short grace only: on GB206 consumer silicon this scratch stays 0 even
        // on boots where GSP-RM comes up fine
        let sw = crate::kcore::time::Stopwatch::start();
        while sw.elapsed_ms() < 200 {
            gfw = bar0.read32(PGC6_AON_SECURE_SCRATCH_GROUP_05_0);
            if gfw & 0xFF == GFW_BOOT_PROGRESS_COMPLETED {
                break;
            }
            core::hint::spin_loop();
        }
    }
    // One line: the value does not change between runs and is not a gate
    // (OpenRM's Hopper+ path does not read this scratch)
    crate::println!("    gfw {:#04x} plm {:#010x}", gfw & 0xFF, plm);
    serial_println!(
        "[gb206/boot] GFW_BOOT_PROGRESS={:#010x} PLM={:#010x}", gfw, plm
    );
    // Not a gate. OpenRM's kgspWaitForGfwBootOk_GH100 reduces the "GFW ok"
    // check to "FSP secure boot done" on FSP-managed chips; the PGC6 progress
    // scratch reads 0 here pre-COT

    // Clear the FMC's error channel so a code found after the boot is this
    // attempt's (OpenRM gpuResetGspFmcErrorCode_GH100, first statement of
    // kgspBootstrap_GH100)
    let stale_fmc_err = bar0.read32(PBUS_SW_SCRATCH_GSP_FMC_ERROR);
    bar0.write32(PBUS_SW_SCRATCH_GSP_FMC_ERROR, 0);
    if stale_fmc_err != 0 {
        serial_println!(
            "[gb206/boot] cleared stale GSP-FMC error {:#010x}", stale_fmc_err
        );
    }
    serial_println!(
        "[gb206/boot] FSP ucode v{}.{} (scratch {:#010x})",
        (bar0.read32(GFW_FSP_UCODE_VERSION) >> 8) & 0xF,
        bar0.read32(GFW_FSP_UCODE_VERSION) & 0xFF,
        bar0.read32(GFW_FSP_UCODE_VERSION)
    );
}

/// The COT, and everything up to a GSP-RM that is running
pub(super) fn bootstrap(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    staged: &Staged,
    detach_console: bool,
    report: &mut BootReport,
) -> Result<(), BootError> {
    let fsp = Fsp::new(bar0);
    let cot = &staged.cot;
    let boot_params = &staged.boot_params;
    let bl_app_version = staged.bl_app_version;
    phase(5, "COT -> FSP (screen may die here, OS keeps running)");
    // The console stops updating around +8s, a couple of seconds before GSP-RM
    // is up: GSP-RM remapping BAR1, where the GOP framebuffer lives. Say so,
    // and where the ending is readable
    crate::println!("      (BAR1 gets remapped: the screen stops updating a few seconds");
    crate::println!("       before the boot ends. Reboot, then 'pstore' for the verdict)");
    if detach_console {
        crate::println!("    parking the framebuffer console - rest of this boot is serial only");
        crate::io::console::detach_framebuffer();
        serial_println!(
            "[gb206/boot] framebuffer console parked; BAR1 is not touched from here on"
        );
    }
    fsp.log_state("pre-COT");
    let resp = match fsp.send_sync(NVDM_TYPE_COT, &cot.as_words()) {
        Ok(r) => r,
        Err(e) => {
            fsp.log_state("post-COT-fail");
            // Queue pointers on screen too: cmdq h==t means the FSP consumed
            // the command; msgq h==t means it never answered
            crate::println!(
                "    FSP after failure: scratch={:#04x} cmdq h/t={:#x}/{:#x} msgq h/t={:#x}/{:#x}",
                fsp.boot_status(),
                bar0.read32(PFSP_QUEUE_HEAD0), bar0.read32(PFSP_QUEUE_TAIL0),
                bar0.read32(PFSP_MSGQ_HEAD0), bar0.read32(PFSP_MSGQ_TAIL0),
            );
            return Err(e.into());
        }
    };
    report.cot_response_words = resp.len_words;
    serial_println!(
        "[gb206/boot] FSP accepted COT (response type {:#x}, {} words)",
        resp.nvdm_type, resp.len_words
    );

    // 9) Lockdown release poll (TSC-bounded, 4 s)
    phase(6, "waiting for lockdown release");
    let sw = crate::kcore::time::Stopwatch::start();
    let (released, last_mbox0) = loop {
        match lockdown_step(bar0, boot_params.phys())? {
            (true, m) => break (true, m),
            (false, m) => {
                if sw.elapsed_ms() as u64 * 1_000_000 >= LOCKDOWN_TIMEOUT_NS as u64 {
                    break (false, m);
                }
                core::hint::spin_loop();
            }
        }
    };
    report.lockdown_released = released;
    report.mbox0 = last_mbox0;
    if !released {
        let hwcfg2 = bar0.read32(PGSP_FALCON_BASE + FALCON_HWCFG2);
        // OpenRM dumps the FSP scratch group and the FMC error code on this
        // failure (kfspDumpDebugState_GH100 + gpuReportGspFmcErrorCode_HAL);
        // both survive the decoupler
        if let Some((raw, decoded)) = fmc_error(bar0) {
            report.fmc_error_raw = raw;
            print_fmc_error(raw, decoded);
        }
        crate::println!(
            "    FSP scratch: {:#010x} {:#010x} {:#010x} {:#010x}",
            bar0.read32(PFSP_COMMON_SCRATCH_GROUP_2_0),
            bar0.read32(PFSP_COMMON_SCRATCH_GROUP_2_0 + 4),
            bar0.read32(PFSP_COMMON_SCRATCH_GROUP_2_0 + 8),
            bar0.read32(PFSP_COMMON_SCRATCH_GROUP_2_0 + 12),
        );
        // A read still 0 / 0xbadf41xx means the BAR0 decoupler was never
        // lifted: the FMC never handed the falcon back. Different from
        // "lockdown bit still set"
        if hwcfg2 == 0 || crate::drivers::gpu::nvidia::pri::is_target_locked(hwcfg2) {
            return Err(BootError::TargetMaskTimeout { hwcfg2 });
        }
        return Err(BootError::LockdownTimeout { mbox0: last_mbox0, hwcfg2 });
    }
    serial_println!(
        "[gb206/boot] RISC-V lockdown released (mbox0={:#x}) - GSP-RM is executing",
        last_mbox0
    );

    // gpuCheckGspFmcErrorCode_HAL: the FMC can release the falcon and still
    // have posted a fatal code
    if let Some((raw, decoded)) = fmc_error(bar0) {
        report.fmc_error_raw = raw;
        print_fmc_error(raw, decoded);
        return Err(BootError::FmcErrorCode { raw, decoded });
    }

    // kflcnIsRiscvActive_GA102. OpenRM treats an inactive core here as "Failed
    // to boot GSP" and stops; otherwise the driver waits 2 minutes for MSGQ
    // traffic from a core that never started
    let cpuctl = bar0.read32(PRISCV_RISCV_CPUCTL);
    report.riscv_active = cpuctl & RISCV_CPUCTL_ACTIVE_STAT != 0;
    serial_println!(
        "[gb206/boot] RISCV_CPUCTL={:#010x} (active={} halted={})",
        cpuctl,
        report.riscv_active,
        cpuctl & RISCV_CPUCTL_HALTED != 0
    );
    if !report.riscv_active {
        crate::println!(
            "    GSP RISC-V not running (CPUCTL {:#010x}, halted={})",
            cpuctl, cpuctl & RISCV_CPUCTL_HALTED != 0
        );
        return Err(BootError::RiscvNotActive { cpuctl });
    }
    // The whole post-FMC verdict goes here: after this GSP-RM may take scanout
    // away at any moment
    crate::println!(
        "    lockdown ok  mbox0 {:#x}  cpuctl {:#010x}  fmc-err none",
        last_mbox0, cpuctl
    );

    // 9b) FALCON_OS <- bootloader app version. gh100_gsp_init ends by calling
    // r535_gsp_init, whose first statement is nvkm_falcon_wr32(&gsp->falcon,
    // 0x080, gsp->boot.app_version): the version handshake on the Blackwell
    // path too, before anything waits on the MSGQ. After lockdown release: the
    // register is not writable while the falcon is fenced off
    bar0.write32(PGSP_FALCON_BASE + FALCON_OS, bl_app_version);
    serial_println!(
        "[gb206/boot] FALCON_OS <- app_version {:#x} (readback {:#x})",
        bl_app_version, bar0.read32(PGSP_FALCON_BASE + FALCON_OS)
    );
    Ok(())
}
