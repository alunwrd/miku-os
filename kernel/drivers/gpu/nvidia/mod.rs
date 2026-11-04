// NVIDIA GPU drivers for MikuOS
//
// Three layers, with the dependency arrow pointing up this list:
//
//   chip-neutral       mmio, intr, regs, chip, pci, vbios, falcon, fbif, pmc,
//                      nvfw_hs, profile, fb, pramin, gmmu, scanout, blur
//                      Registers and mechanisms with the same shape on every
//                      GSP-era part.
//
//   gsp_common/        The GSP-RM world once the RISC-V core is running: boot
//                      arguments, the CMDQ/MSGQ transport, RPC framing, the RM
//                      object/control layer, display and compute channels, and
//                      the firmware-container parsers in `gspfw.rs`.
//                      The object class numbers in disp.rs / chan.rs /
//                      compute.rs are GB20x; a second architecture needs its
//                      own table.
//
//   turing/            Per-architecture drivers. These may use both layers
//   blackwell/         above; neither layer above may use them.
//
// Turing is the first NVIDIA family with a GSP (GPU System Processor)
// co-processor. Without a signed GSP firmware blob, most engines beyond the
// host registers are off-limits. Dispatch in init():
//   - GTX 1650 / 1660 (TU116/TU117): the one Turing chip with an embedded
//     firmware bundle, so it runs the full GSP-RM offload pipeline
//     (turing::init).
//   - RTX 5060 / 5060 Ti (GB206): the FSP chain-of-trust boot path
//     (blackwell::init). The display work runs on this card.
//   - every other NVIDIA card: generic::bringup does host-side recognition (BAR
//     map, chip-ID, MSI, VBIOS, PMC, thermal, Falcon liveness) and registers
//     it; the firmware pipeline is gated on a per-chip bundle.
//
// The module names are architectures, not SKUs; the SKU tables live inside
// (turing/tu116.rs, turing/tu117.rs, blackwell/mod.rs). The firmware
// directories under firmware/ use SKU names, wired into the builder, and hold
// blobs that are not in git.

// `main.rs` sets `#![allow(dead_code, ...)]` crate-wide; this keeps the warning
// on for this subtree
#![warn(dead_code, unused_imports)]

pub mod le;

/// A failure told through a `say` sink, the `&dyn Fn(&str)` the bring-up hands
/// down, recorded in the kernel log at `level` and not as console output
/// (`klog::at_least`, `dev/klog.md`)
pub fn said(level: crate::kcore::klog::Level, say: &dyn Fn(&str), text: &str) {
    crate::kcore::klog::at_least(level, || say(text));
}

/// `say`, with every line through it signed `site` (`klog::speaking_for`), so
/// the lines belong to the subsystem in `nvidia log`. Each engine's load does
/// this with its own file's `SITE`
pub fn speaking<'a>(
    site: &'static crate::kcore::klog::Site,
    say: &'a dyn Fn(&str),
) -> impl Fn(&str) + 'a {
    move |s: &str| crate::kcore::klog::speaking_for(site, || say(s))
}

pub mod pri;
pub mod cubin;
pub mod shaders;
pub mod pci;
pub mod chipset;
pub mod reset;
pub mod mmio;
pub mod intr;
pub mod regs;
pub mod falcon;
pub mod fbif;
pub mod nvfw_hs;
pub mod pmc;
pub mod chip;
pub mod vbios;
pub mod fb;
pub mod pramin;
pub mod gmmu;
pub mod vaspace;
/// A program's video memory, through BAR1 (3.7)
pub mod user_vram;
pub mod surface;
pub mod scanout;
/// The display layer's driver for this card (`drivers::gpu::kms`)
pub mod kms;
pub mod blur;
pub mod profile;
pub mod turing;
pub mod blackwell;
pub mod gsp_common;
pub mod generic;
pub mod selftest;

use spin::Mutex;

use crate::serial_println;

pub const VENDOR_NVIDIA: u16 = 0x10DE;

/// Global handle for the first GTX 1650 we bring up, none until init() runs
static ACTIVE_GTX1650: Mutex<Option<turing::Gtx1650>> = Mutex::new(None);

pub fn set_active_gtx1650(dev: turing::Gtx1650) {
    *ACTIVE_GTX1650.lock() = Some(dev);
}

/// Run a closure with the active GTX 1650 device, if any. Returns None if
/// the driver did not bring up a card
pub fn with_gtx1650<R>(f: impl FnOnce(&turing::Gtx1650) -> R) -> Option<R> {
    let guard = ACTIVE_GTX1650.lock();
    guard.as_ref().map(f)
}

/// The GB20x this machine brought up, if it has one.
///
/// Held only to copy the record out: `with_gb20x`'s closures make RPCs, which
/// sleep, and other CPUs wanting the card would spin on this lock meanwhile
static ACTIVE_GB20X: Mutex<Option<blackwell::Gb20x>> = Mutex::new(None);

pub fn set_active_gb20x(dev: blackwell::Gb20x) {
    *ACTIVE_GB20X.lock() = Some(dev);
}

/// The card, by value. The BAR mapping, the chip and the capabilities do not
/// change after bring-up
pub fn device() -> Option<blackwell::Gb20x> {
    ACTIVE_GB20X.lock().clone()
}

/// Run a closure with the card, if there is one. The closure gets a copy
/// (`device`); no lock is held while it runs
pub fn with_gb20x<R>(f: impl FnOnce(&blackwell::Gb20x) -> R) -> Option<R> {
    device().map(|d| f(&d))
}

/// PMC_BOOT_42 of a card nothing has touched yet, or `None` when BAR0 is not a
/// memory window the firmware assigned.
///
/// OpenRM picks a chip's HAL from it (`gpu_mgr.c:1264`), not from the PCI
/// device id. Needs memory decode, which every path below turns on
fn boot42(gpu: &pci::GpuDevice) -> Option<u32> {
    let bar0 = &gpu.bars[0];
    if bar0.is_io || bar0.phys == 0 || bar0.size == 0 {
        return None;
    }
    pci::enable_memory_and_bus_master(gpu);
    Some(mmio::MmioRegion::new(bar0.phys, bar0.size).read32(regs::PMC_BOOT_42))
}

/// Bring the display up on the card the machine has.
///
/// `init` above is recognition: BAR, chip id, MSI, and which firmware this
/// board would need. This is the rest: GSP-RM, the modeset, the channels.
///
/// Everything is a skip. A GPU that will not come up leaves a system that
/// boots: the firmware framebuffer is there, the compositor has a software path
/// (`mayland::render::start`), and the shell runs. The only `Err` says the card
/// was asked and refused, and the caller records and steps over it.
///
/// `nogpu` on the boot line, next to `nointr`, `nosmap` and `nousb`, skips this
pub fn autoboot() -> Result<(), &'static str> {
    if crate::arch::x86_64::grub::cmdline_flag("nogpu") {
        serial_println!(
            "[nvidia] 'nogpu' on the boot line - the card is left as the firmware set it"
        );
        return Ok(());
    }

    let ran = with_gb20x(|d| blackwell::boot::boot(&d.bar0, &d.pci, d.gb));
    let Some(result) = ran else {
        // No Blackwell card
        return Ok(());
    };

    let out = match result {
        Ok(r) => {
            match blackwell::boot::live_scanout() {
                Some(l) => {
                    serial_println!(
                        "[nvidia] display up: {}x{} at vram {:#x}, head {} window {}",
                        l.width, l.height, l.vram, l.head, l.window
                    );
                    // Variable refresh only when asked for on the boot line,
                    // and only once there is a picture to fall back to
                    // (`dev/display-nvkms.md`, decision 6), through the display
                    // layer, as `display vrr on` is
                    if crate::arch::x86_64::grub::cmdline_flag("vrr") {
                        use crate::drivers::gpu::kms;
                        let st = kms::state();
                        let primary = gsp_common::output::primary_id();
                        if let Some(o) = st.outputs.iter().find(|o| Some(o.key) == primary) {
                            let mut r = kms::HeadRequest::current(o);
                            r.vrr = true;
                            match kms::apply(&kms::Request::one(r, false)) {
                                Ok(lines) => serial_println!("[nvidia] 'vrr': {}", lines.join("; ")),
                                Err(e) => crate::kwarn!("[nvidia] 'vrr': variable refresh not turned on: {}", e),
                            }
                        }
                    }
                }
                None => serial_println!(
                    "[nvidia] GSP-RM is up (init_done {}) but no scanout was armed",
                    r.init_done
                ),
            }
            Ok(())
        }
        // The blobs are not in this tree and their absence is normal
        // (`firmware/README.md`). `init` has already said which directory it
        // wanted
        Err(blackwell::boot::BootError::FirmwareMissing(p)) => {
            serial_println!("[nvidia] no display: /lib/firmware/{} is not on this image", p);
            Ok(())
        }
        Err(e) => {
            crate::kerror!("[nvidia] display bring-up failed: {:?}", e);
            serial_println!(
                "[nvidia] the firmware framebuffer is still live; \
                 'nvidia gb206-scratch' says where it stopped"
            );
            Err("GSP-RM bring-up failed - see serial")
        }
    };
    log_summary();
    out
}

/// The driver's modules, as the kernel log records them (`module_path!()`):
/// what `nvidia log` shows when no section is named
pub const LOG_MODULES: &[&str] = &["drivers::gpu::nvidia", "drivers::gpu::edid"];

/// One line at the end of the bring-up: how much went wrong, and where to read
/// about it
fn log_summary() {
    use crate::kcore::klog::{view, Level};
    let f = view::Filter {
        least: Level::Warn,
        modules: LOG_MODULES.iter().map(|m| alloc::string::String::from(*m)).collect(),
        ..Default::default()
    };
    let bad = view::collect(&f);
    let errors = bad.iter().filter(|l| l.level == Level::Error).count();
    let warnings = bad.len() - errors;
    let level = if errors > 0 { Level::Warn } else { Level::Info };
    crate::kcore::klog::emit(level, crate::klog_site!(), format_args!(
        "[nvidia] log: {} error(s), {} warning(s) since boot{}",
        errors, warnings, if bad.is_empty() { "" } else { " - 'nvidia log err' shows them" }
    ));
}

/// Tell the card the driver is going away, on the way to a reset.
///
/// Called from `kcore::power`, the one door every reboot and shutdown goes
/// through. Whatever is drawing lets go of the surface first, and only then is
/// GSP-RM told to stop: a copy engine finishing a frame while the firmware
/// owning that memory unloads is a race.
///
/// Silent and quick when there is no card, it was never booted, or it has
/// stopped answering
pub fn shutdown() {
    let Some(dev_present) = with_gb20x(|_| ()) else { return };
    let _ = dev_present;

    // The compositor first, then the console: `stop` hands the screen back to
    // the console. Guarded since `stop` sleeps for two frames and repaints the
    // whole console
    if crate::mayland::render::running() {
        crate::mayland::render::stop();
    }
    scanout::detach_console();

    let say = |l: &str| crate::kinfo!("[nvidia] shutdown: {}", l);
    let report = with_gb20x(|d| blackwell::boot::shutdown(&d.bar0, &d.pci, &say));
    let Some(Some(r)) = report else { return };
    if r.halted {
        serial_println!(
            "[nvidia] GSP-RM unloaded, suspended {} and halted (mbox0 {:#x})",
            r.suspended, r.mbox0
        );
    } else if r.rpc_ok {
        serial_println!(
            "[nvidia] GSP-RM took the unload but did not halt in time (mbox0 {:#x}, cpuctl {:#010x})",
            r.mbox0, r.cpuctl
        );
    } else {
        crate::kwarn!("[nvidia] GSP-RM did not answer the unload; resetting anyway");
    }
}

pub fn init() -> Result<(), &'static str> {
    let gpus = pci::scan_nvidia();
    if gpus.is_empty() {
        serial_println!("[nvidia] no NVIDIA GPU found");
        return Ok(());
    }

    for gpu in gpus.iter() {
        serial_println!(
            "[nvidia] found {:04x}:{:04x} at {:02x}:{:02x}.{:x} rev={:#x}",
            gpu.vendor, gpu.device, gpu.bus, gpu.dev, gpu.func, gpu.revision
        );
        let gb20x = boot42(gpu).and_then(blackwell::chips::by_boot42);
        // Which firmware this machine needs, for the builder.
        //
        // GRUB reads all of boot/firmware.img into RAM before the kernel
        // starts, so unused microcode is still read off the USB stick every
        // boot. The builder may run on another computer, so it stages
        // everything unless told, and this line tells it
        let fw_dir = if turing::matches(gpu.device) {
            Some(turing::FW_DIR)
        } else {
            gb20x.map(|c| c.fw_dir)
        };
        match fw_dir {
            Some(dir) => serial_println!(
                "[nvidia] this card's firmware is /lib/firmware/{} - build with MIKU_FW={} \
                 to leave the other cards out of the boot image",
                dir, dir
            ),
            None => serial_println!(
                "[nvidia] no firmware bundle in this tree for {:04x} (host-side bring-up only)",
                gpu.device
            ),
        }
        if turing::matches(gpu.device) {
            // GTX 1650 / 1660 (TU116/TU117): the one chip with an embedded
            // firmware bundle, so it runs the full GSP-RM pipeline
            if let Err(e) = turing::init(gpu) {
                crate::kerror!("[nvidia] turing init failed: {}", e);
            }
        } else if let Some(chip) = gb20x {
            // Any GB20x (RTX 50 series), by PMC_BOOT_42: FSP-based boot path;
            // host bring-up + FSP transport + firmware staging probe
            if let Err(e) = blackwell::init(gpu, chip) {
                crate::kerror!("[nvidia] blackwell init failed: {}", e);
            }
        } else {
            // Any other NVIDIA GPU: generic host-side bring-up + register it.
            // Recognizes the chip, probes engines, but has no firmware so it
            // stops short of the GSP-RM offload pipeline
            match generic::bringup(gpu) {
                Ok(codename) => serial_println!(
                    "[nvidia] {:04x} brought up as {} (host-side only)", gpu.device, codename
                ),
                Err(e) => crate::kerror!(
                    "[nvidia] device {:04x} host-side bring-up failed: {}", gpu.device, e
                ),
            }
        }
    }
    Ok(())
}
