// The GB20x boot, stage by stage (`dev/gsp-foundation.md`, step 1.10):
//
//   stage.rs         0-8: images, radix3s, boot args and our ring, GSP_INIT's
//                    data, WPR metadata, boot parameters, the COT
//   ../hal_gb20x.rs  the chip's half: GFW, the COT to the FSP, lockdown, the
//                    checks that GSP-RM runs, FALCON_OS (`kgspBootstrap_GH100`)
//   handshake.rs     10: GSP-RM's queue linked, GSP_INIT and the boot events
//   rm.rs            after GSP_INIT: RM client, display, channels, interrupt
//   narrate.rs       the PRAMIN console, and the verdict
//   unload.rs        the other direction: engines down, GSP-RM told and
//                    waited for, everything it named forgotten (step 1.14)
//   recover.rs       both, with a function-level reset between them: a dead
//                    GSP-RM without a reboot (`dev/gsp-foundation.md`, 16)
//
// `boot_opts` below is the order.

use crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc;

use super::gspboot::{BootError, BootFlags, BootImage, BootReport, DisplayMemory, RmState};

mod handshake;
mod narrate;
mod recover;
mod rm;
pub(super) mod stage;
mod unload;

pub use recover::{on_fatal, reset_and_reload};
pub use unload::{shutdown, unload};

/// `gspbase` on the boot line: boot GSP-RM as card boot A did, without steps
/// 1.12 and 1.13 of `dev/gsp-foundation.md`: the sysmem flush buffer, the
/// wall-clock time, `bIsPrimary` from the machine, the heap size, the CrashCat
/// queue. Like `edidbase` for the EDID
pub(super) fn base_init() -> bool {
    crate::arch::x86_64::grub::cmdline_flag("gspbase")
}

/// Full COT boot with default flags. Non-destructive up to the send_sync:
/// everything before it is sysmem staging and BAR0 reads. 'gpu' feeds the
/// system information GSP_INIT carries.
///
/// The shell uses `boot_opts` for its flags; this is for code that wants the
/// card up: `nvidia::autoboot`
pub fn boot(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    gpu: &crate::drivers::gpu::nvidia::pci::GpuDevice,
    chip: &'static super::chips::Chip,
) -> Result<BootReport, BootError> {
    boot_opts(bar0, gpu, chip, BootFlags::default())
}

/// What the RM client and the channel work said, held until the boot is over.
///
/// Not printed from inside `boot_opts`: everything there reaches the panel
/// through `vram_con`, painting video memory through PRAMIN, and by now the
/// framebuffer console owns that surface and repaints it from its character
/// grid. The only way onto the screen is through the console, after the command
/// returns.
///
/// Only lines the RM client produced, enforced by `record`. The closure filling
/// this also carries the whole display bring-up (every `DISP:`, `R ` and `D `
/// line), longer than a 1080p console has rows; the console prints that as it
/// happens
static RM_LOG: spin::Mutex<alloc::vec::Vec<alloc::string::String>> =
    spin::Mutex::new(alloc::vec::Vec::new());

/// Keep a line for the closing block if it is one of the RM client's own
pub(super) fn record(s: &str) {
    if s.starts_with("RM:") {
        RM_LOG.lock().push(alloc::string::String::from(s));
    }
}

/// Hand over what the RM client said, once, for the caller to print
pub fn take_rm_log() -> alloc::vec::Vec<alloc::string::String> {
    core::mem::take(&mut *RM_LOG.lock())
}

/// The whole boot, in order. Every step is a function in this directory or in
/// `hal_gb20x.rs`
pub fn boot_opts(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    gpu: &crate::drivers::gpu::nvidia::pci::GpuDevice,
    chip: &'static super::chips::Chip,
    flags: BootFlags,
) -> Result<BootReport, BootError> {
    let BootFlags {
        detach_console, log_verbose, led_heartbeat, not_primary, disp_vidmem,
    } = flags;
    let mut report = BootReport::default();

    // Stamp the persistent log with which run wrote it, flags included
    crate::pstore_println!(
        "=== gb206 boot: noscreen={} log={} leds={} noprimary={} dispvid={} gspbase={} ===",
        detach_console, log_verbose, led_heartbeat, not_primary, disp_vidmem, base_init()
    );

    // The bus first, before the FSP and the COT have a buffer to DMA from: what
    // the GPU is plugged into, and its Device Control set to match, as OpenRM's
    // `clUpdatePcieConfig` does right after the registers are mapped
    // (`dev/gsp-foundation.md`, step 1.17)
    {
        use crate::drivers::gpu::nvidia::chipset;
        let mut cl = chipset::discover(gpu);
        chipset::init_device_control(gpu, &mut cl);
        crate::kinfo!(
            "[nvidia/bus] chipset {} ({:#06x}), root port {}, ECAM {}, Device Control {:#06x} -> {:#06x}{}",
            cl.name, cl.chipset,
            cl.root.map(|r| alloc::format!("{:02x}:{:02x}.{} {:04x}:{:04x}", r.bus, r.dev, r.func, r.vendor, r.device))
                .unwrap_or_else(|| alloc::string::String::from("none")),
            if cl.has(chipset::Prop::PcieConfigAccessible) { "yes" } else { "no" },
            cl.devctl_before, cl.devctl_after,
            if chipset::base_devctl() { " (pciebase: left as the firmware set it)" } else { "" },
        );
    }

    // Dump all BARs once: "bars[1] is BAR1" is an assumption used elsewhere,
    // and a 64-bit BAR occupies two slots
    for (i, b) in gpu.bars.iter().enumerate() {
        if b.size != 0 {
            crate::println!(
                "R bar{} {:#x} size {:#x}{}", i, b.phys, b.size,
                if b.is_64bit { " 64-bit" } else { "" }
            );
        }
    }

    let staged = stage::stage(bar0, gpu, chip, not_primary, disp_vidmem, &mut report)?;

    // Filled in as the display comes up; parked in the boot state afterwards so
    // the hardware's copies of these addresses stay valid
    let mut disp_mem = DisplayMemory::default();

    super::hal_gb20x::pre_cot(bar0);
    let mut narration = narrate::prepare(bar0, gpu, chip);
    super::hal_gb20x::bootstrap(bar0, &staged, detach_console, &mut report)?;
    // Where this chip keeps the GSP's registers, for everything from here on:
    // the health check during the handshake, the interrupt, and an unload,
    // which has to find them even when RM never answered
    crate::drivers::gpu::nvidia::gsp_common::kgsp::set_regs(super::interrupts::GSP_REGS);

    let stage::Staged {
        fmc_image, bootloader, staged_fw, signature, radix3, ucodes, mut bootargs, gsp_init,
        wpr_meta, boot_params, ..
    } = staged;

    // 10) GSP-RM's queue linked and GSP_INIT answered; the bootup events on
    // the way are shown as they come (`handshake.rs`)
    let info = {
        let mut rpc = GspRpc::new(bar0, &mut bootargs);
        handshake::gsp_init(
            bar0, &mut rpc, &gsp_init, &mut narration.vram_con,
            log_verbose, led_heartbeat, &mut report,
        )
    };
    // GSP-RM has what it needs from the ucodes once GSP_INIT has answered, and
    // OpenRM frees them then (`kgspSendInitRpcs_IMPL`). Kept if it never
    // answered: GSP-RM may still be reading them
    if info.is_some() {
        drop(ucodes);
        bootargs.set_bindata(0, 0);
    } else {
        core::mem::forget(ucodes);
    }
    // With GSP-RM up, build the client -> device -> subdevice chain and ask one
    // question with a known answer. Here and not as a shell command, since the
    // screen is dead by now; one `nvidia gb206-boot` produces the whole record
    // in pstore
    if let Some(info) = info {
        let mut rpc = GspRpc::new(bar0, &mut bootargs);
        rm::bring_up(bar0, gpu, &mut rpc, &info, &mut disp_mem, &mut report);
    }

    narrate::verdict(bar0, &mut narration.vram_con, &mut report);

    super::gspboot::install(
        BootImage { fmc_image, staged_fw, radix3, bootloader, signature, wpr_meta, boot_params },
        RmState { bootargs },
        disp_mem,
    );
    // Last, since it outlives the boot: only this drains GSP-RM's event queue
    // once the boot has returned, and a machine on a desktop makes no RM calls.
    // RM being up is enough; a card with no monitor has events too
    if report.rm_client {
        super::control::start_event_thread();
    }
    // BAR1 in physical mode from here: the console writes video memory with
    // ordinary stores, and a program's video memory has a CPU address
    // (`dev/gpu-memory-compute.md`, 3.1 step 3 and 3.7). After the channels,
    // which the bind holds still, and before anything else draws. `nobar1` on
    // the boot line keeps it a command
    if report.rm_client && !crate::arch::x86_64::grub::cmdline_flag("nobar1") {
        match super::bar1::on(bar0) {
            Ok(a) => crate::kinfo!(
                "[nvidia] BAR1 physical: {} MiB of video memory at {:#x}, the console and programs write through it",
                a.size >> 20, a.phys),
            Err(e) => crate::kwarn!("[nvidia] BAR1 left as GSP-RM made it: {}", e),
        }
    }

    Ok(report)
}
