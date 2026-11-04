// What the boot says, where it can still be read
//
// Two channels the GPU cannot take away. The PRAMIN console writes the screen
// through BAR0 after GSP-RM's display init has remapped BAR1; the persistent
// log survives a reboot. `prepare` arms the first and loads GSP-RM's logging
// ELF while a disk can still be read; `verdict` is the ending, on both
// (`dev/gsp-foundation.md`, decision 12).

use crate::serial_println;

use super::super::chips::Chip;
use super::super::gspboot::{fmc_error, print_fmc_error, BootReport};
use super::super::regs::*;

/// The PRAMIN console, if the scanout surface could be found. The logging ELF
/// lives in `kgsp::logs`
pub(super) struct Narration {
    pub vram_con: Option<crate::drivers::gpu::nvidia::pramin::VramConsole>,
}

pub(super) fn prepare(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    gpu: &crate::drivers::gpu::nvidia::pci::GpuDevice,
    chip: &Chip,
) -> Narration {
    // Load GSP-RM's logging ELF now, while nothing is time-critical: once the
    // handshake starts there is no safe moment to go to disk. Without it the
    // log buffers stay opaque token streams. Handed to `kgsp::logs`, which
    // reads the buffers for as long as this GSP-RM runs
    match crate::kcore::firmware::load::request(chip.gsp_log) {
        Ok(f) => {
            crate::println!("    gsp log elf: {} KiB - live decode on", f.len() / 1024);
            crate::drivers::gpu::nvidia::gsp_common::kgsp::logs::install(f);
        }
        Err(e) => {
            crate::println!("    no gsp log elf - GSP-RM's own log will stay undecoded");
            serial_println!("[gb206/boot] logging ELF unavailable: {:?}", e);
            crate::drivers::gpu::nvidia::gsp_common::kgsp::logs::forget();
        }
    }

    // Second console, reaching the same screen through BAR0/PRAMIN. When
    // GSP-RM's display init remaps BAR1 the normal console goes silent; this
    // one keeps writing
    let mut vram_con = crate::drivers::gpu::nvidia::pramin::VramConsole::locate(gpu, bar0);
    match &vram_con {
        Some(_) => crate::println!("    PRAMIN console armed (bottom of screen, survives BAR1 remap)"),
        None => crate::println!(
            "    no PRAMIN console (scanout surface not found in the first 512 MiB of VRAM)"),
    }
    Narration { vram_con }
}

/// The ending: the last FMC error read, the verdict, and why GSP_INIT was not
/// answered, if it was not
pub(super) fn verdict(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    vram_con: &mut Option<crate::drivers::gpu::nvidia::pramin::VramConsole>,
    report: &mut BootReport,
) {
    // Last look at the FMC error channel: a GSP-RM that dies inside its own
    // init can still post a code here after the falcon came up
    if let Some((raw, decoded)) = fmc_error(bar0) {
        report.fmc_error_raw = raw;
        print_fmc_error(raw, decoded);
    }
    // The verdict, on every channel the GPU cannot take away. The framebuffer
    // console has been dead for a minute or more, so these lines reach a human
    // through PRAMIN or through the persistent RAM log after a reboot
    {
        let cpuctl = bar0.read32(PRISCV_RISCV_CPUCTL);
        let verdict = if report.init_done {
            "RESULT: GSP_INIT answered - GSP-RM is up"
        } else {
            "RESULT: no answer to GSP_INIT"
        };
        let detail = alloc::format!(
            "  cpuctl {:#010x}  msgq_init {}  fmc-err {:#x}",
            cpuctl, report.msgq_initialized, report.fmc_error_raw
        );
        // The one check the 615 ring has left: records read twice
        let integrity = alloc::format!(
            "GSP: {} stale record(s) skipped",
            crate::drivers::gpu::nvidia::gsp_common::rpc::stale_records()
        );
        if let Some(vc) = vram_con.as_mut() {
            vc.line(bar0, verdict);
            vc.line(bar0, &detail);
            vc.line(bar0, &integrity);
        }
        crate::pstore_println!("{}", verdict);
        crate::pstore_println!("{}", detail);
        crate::pstore_println!("{}", integrity);
        serial_println!("[gb206/boot] {}", integrity);
    }
    if !report.init_done {
        let cpuctl = bar0.read32(PRISCV_RISCV_CPUCTL);
        report.riscv_active = cpuctl & RISCV_CPUCTL_ACTIVE_STAT != 0;
        crate::println!(
            "      no GSP_INIT answer: RISCV_CPUCTL={:#010x} (active={} halted={})",
            cpuctl,
            report.riscv_active,
            cpuctl & RISCV_CPUCTL_HALTED != 0
        );
    }
}
