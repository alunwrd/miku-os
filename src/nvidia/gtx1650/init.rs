// GTX 1650 initialization sequence
//
// Current steps:
//   1) Validate the BAR layout handed to us by firmware
//   2) Enable memory space + bus mastering in the PCI command register
//   3) Map BAR0 (MMIO registers) via HHDM.
//   4) Read PMC_BOOT_0 / PMC_BOOT_42 for chip identification
//   5) Walk PCI capabilities for MSI / MSI-X.
//   6) Walk PTOP_DEVICE_INFO to enumerate visible engines
//   7) Read VBIOS from the expansion ROM (if assigned)
//   8) Hand the device to the global registry
//
// Not yet done (and blockers before we can touch engines):
//   devinit script execution from the VBIOS init table
//   GSP firmware load and boot (TU117 needs it for anything serious)
//   ioremap path for BARs outside HHDM
//   MSI routing (waits on apic::alloc_msi_vector)

use crate::nvidia::chip::{ChipId, PMC_BOOT_0};
use crate::nvidia::mmio::MmioRegion;
use crate::nvidia::{msi, pci, vbios};
use crate::serial_println;

use super::regs::{
    self, PMC_BOOT_42, PTOP_DEVICE_INFO, PTOP_DEVICE_INFO_COUNT,
    PTOP_ENTRY_CHAIN, PTOP_ENTRY_TYPE_DATA, PTOP_ENTRY_TYPE_ENUM,
    PTOP_ENTRY_TYPE_MASK, PTOP_ENTRY_TYPE_SHIFT,
};
use super::{pmc, tu116, tu117, Gtx1650};

pub fn init(gpu: &pci::GpuDevice) -> Result<(), &'static str> {
    // Validate BAR0. On Turing this is a 16 MiB MMIO window
    let bar0 = &gpu.bars[0];
    if bar0.is_io || bar0.phys == 0 || bar0.size == 0 {
        return Err("bar0 is not memory or not assigned by firmware");
    }
    // TU117 and TU116 both expose a 16 MiB MMIO BAR0; pick the variant
    // matching this card's device id so the diagnostic is correctly labelled
    let expected_bar0 = if tu116::matches(gpu.device) {
        tu116::EXPECTED_BAR0_SIZE
    } else {
        tu117::EXPECTED_BAR0_SIZE
    };
    if bar0.size < expected_bar0 {
        serial_println!(
            "[gtx1650] warn: BAR0 size {:#x} < expected {:#x}",
            bar0.size, expected_bar0
        );
    }

    // BAR1: framebuffer aperture, typically 256 MiB on GTX 1650
    let bar1 = &gpu.bars[1];
    if bar1.phys == 0 {
        serial_println!("[gtx1650] warn: BAR1 (framebuffer) not assigned");
    }
    // BAR3: USER / IFB, 64-bit prefetchable
    let bar3 = &gpu.bars[3];

    // Turn on memory decoding and bus mastering, mask legacy INTx so
    // MSI (if we program it later) is not racing with INTx
    pci::enable_memory_and_bus_master(gpu);
    pci::disable_intx(gpu);

    // Map BAR0 through HHDM. Real hardware may need vmm::ioremap with
    // PCD|PWT flags for BARs outside HHDM; deferred
    let bar0_region = MmioRegion::new(bar0.phys, bar0.size);

    // Chip identification
    let boot0_raw = bar0_region.read32(PMC_BOOT_0);
    let boot42    = bar0_region.read32(PMC_BOOT_42);
    let chip = ChipId::from_boot0(boot0_raw);

    serial_println!(
        "[gtx1650] PMC_BOOT_0  = {:#010x}  chip = {} (arch={:?} impl={:#x} rev={}.{} step={})",
        boot0_raw, chip.codename(), chip.arch, chip.implementation,
        chip.major_rev, chip.minor_rev, chip.stepping
    );
    serial_println!("[gtx1650] PMC_BOOT_42 = {:#010x}", boot42);

    // Sanity-check: TU117 or TU116
    let expected = matches!(
        (chip.arch, chip.implementation),
        (crate::nvidia::chip::Architecture::Turing, tu117::IMPL_TU117)
            | (crate::nvidia::chip::Architecture::Turing, tu116::IMPL_TU116)
    );
    if !expected {
        serial_println!(
            "[gtx1650] warn: unexpected chip (driver targets TU117 / TU116)"
        );
    }

    // Resolve per-chip quirks (PMC reset masks, firmware blob refs, ...)
    // and report so the operator can confirm the right variant was
    // selected before any engine work runs
    match super::quirks::for_chip(chip) {
        Some(q) => serial_println!(
            "[gtx1650] quirks: {} (sec2_pmc={:#x} gsp_pmc={:#x} acr_wpr2={})",
            q.codename, q.sec2_pmc_reset_mask, q.gsp_pmc_reset_mask, q.needs_acr_wpr2
        ),
        None => serial_println!(
            "[gtx1650] warn: no quirks entry for impl={:#x} - engine paths fall back to defaults",
            chip.implementation
        ),
    }

    // Pick the model name from whichever silicon table claims this device id
    let model_name = super::model_name(gpu.device);
    serial_println!(
        "[gtx1650] model: {}  subsys={:04x}:{:04x}  irq_pin={} line={}",
        model_name,
        gpu.subsystem_vendor, gpu.subsystem_device,
        gpu.irq_pin, gpu.irq_line
    );

    // MSI / MSI-X discovery
    let caps = msi::read_caps(gpu);
    msi::log_capabilities(&caps);

    // PTOP engine walk.
    walk_ptop(&bar0_region);

    // VBIOS extraction (best-effort; some hypervisors hide the ROM)
    match vbios::read_rom(gpu) {
        Some(rom) => {
            let images = vbios::parse_images(&rom);
            if images.is_empty() {
                serial_println!("[gtx1650] VBIOS ROM present but no valid images parsed");
            } else {
                vbios::log_images(&images);
                if let Some(legacy) = vbios::pick_legacy(&images) {
                    serial_println!(
                        "[gtx1650] legacy VBIOS: {} bytes (vendor {:04x}, device {:04x})",
                        legacy.bytes.len(), legacy.vendor, legacy.device
                    );
                    if let Some(hdr) = vbios::find_bit_header(&legacy.bytes) {
                        let tokens = vbios::parse_tokens(&legacy.bytes, &hdr);
                        vbios::log_bit(&hdr, &tokens);
                        if let Some(init_tok) = vbios::find_token(&tokens, b'I') {
                            serial_println!(
                                "[gtx1650] init script table @ {:#x} ({} bytes)",
                                init_tok.data_ptr, init_tok.data_size
                            );
                        }
                        if let Some(dcb_tok) = vbios::find_token(&tokens, b'D') {
                            serial_println!(
                                "[gtx1650] DCB ptr @ {:#x} ({} bytes)",
                                dcb_tok.data_ptr, dcb_tok.data_size
                            );
                        }
                    } else {
                        serial_println!("[gtx1650] no BIT header in legacy VBIOS");
                    }
                }
            }
        }
        None => serial_println!(
            "[gtx1650] VBIOS ROM not available (phys={:#x} size={:#x})",
            gpu.rom_phys, gpu.rom_size
        ),
    }

    // Mask every top-level interrupt before we do anything else that
    // might unblock them. Without a GSP blob we have no way to service engine IRQs, so they must stay quiet
    pmc::mask_all_interrupts(&bar0_region);

    // PMC_ENABLE snapshot before we touch anything. Turing boots with
    // host+PFIFO already on; anything else depends on VBIOS devinit,
    // which we do not run
    let enable_pre = pmc::read_enable(&bar0_region);
    serial_println!("[gtx1650] PMC_ENABLE pre  = {:#010x}", enable_pre);

    // Ungate PGRAPH (GR) and CE0. Without GR the FECS / GPCCS register
    // windows return the 0xBADF_xxxx PRI sentinel rather than a real
    // HWCFG, which makes liveness diagnostics misleading. CE0 is needed
    // later by SEC2 ACR for DMA staging; setting the bit here is cheap
    // and the engine stays idle until ACR pokes it
    let (en_before, en_after) = pmc::ungate_default_engines(&bar0_region);
    if en_before != en_after {
        serial_println!(
            "[gtx1650] PMC_ENABLE post = {:#010x} (set GR + CE0; was {:#010x})",
            en_after, en_before
        );
    } else {
        serial_println!(
            "[gtx1650] PMC_ENABLE post = {:#010x} (GR + CE0 already on)",
            en_after
        );
    }

    // PTIMER calibration. If numerator/denominator are zero the VBIOS
    // devinit script has not run yet, which is expected for us
    let tfreq = pmc::read_ptimer_freq(&bar0_region);
    let input_hz = tfreq.input_clock_hz();
    if input_hz != 0 {
        serial_println!(
            "[gtx1650] PTIMER scale: num={} denom={} ({} MHz input)",
            tfreq.numerator, tfreq.denominator, input_hz / 1_000_000
        );
    } else {
        serial_println!(
            "[gtx1650] PTIMER scale: num={} denom={} (not programmed)",
            tfreq.numerator, tfreq.denominator
        );
    }

    // PTIMER sanity: the counter must advance between two reads
    let ns0 = bar0_region.read32(regs::PTIMER_TIME_0);
    for _ in 0..5000 { core::hint::spin_loop(); }
    let ns1 = bar0_region.read32(regs::PTIMER_TIME_0);
    if ns1 != ns0 {
        serial_println!("[gtx1650] PTIMER alive (TIME_0: {} -> {})", ns0, ns1);
    } else {
        serial_println!("[gtx1650] warn: PTIMER did not advance");
    }

    // On-die temperature sensor, live straight out of POST with no devinit or
    // GSP required. This is a real reading of the silicon.
    let t = super::therm::read(&bar0_region);
    if t.valid {
        serial_println!(
            "[gtx1650] GPU temp: {} C{} (TEMP_SENSOR={:#010x})",
            t.celsius,
            if t.shadowed { " (shadowed/stale)" } else { "" },
            t.raw
        );
    } else {
        serial_println!(
            "[gtx1650] GPU temp: sensor not valid (TEMP_SENSOR={:#010x})",
            t.raw
        );
    }
    match (t.slowdown_celsius(), t.shutdown_celsius()) {
        (Some(s), Some(h)) => serial_println!(
            "[gtx1650] thermal limits: slowdown={} C shutdown={} C", s, h
        ),
        _ => serial_println!(
            "[gtx1650] thermal limits: not programmed (VBIOS devinit not run)"
        ),
    }

    serial_println!(
        "[gtx1650] BAR0: phys={:#x} size={:#x} (virt via HHDM)",
        bar0.phys, bar0.size
    );
    if bar1.phys != 0 {
        serial_println!("[gtx1650] BAR1 (FB): phys={:#x} size={:#x}", bar1.phys, bar1.size);
    }
    if bar3.phys != 0 {
        serial_println!("[gtx1650] BAR3 (USER): phys={:#x} size={:#x}", bar3.phys, bar3.size);
    }

    // Correlate the firmware-provided framebuffer with our BARs. If it
    // is inside BAR1, every pixel we draw via the console is already
    // being written into this card's video RAM - i.e., we are "using the GPU" for output even without a GSP-backed driver
    let boot_fb = match crate::grub::framebuffer() {
        Some(info) => {
            let loc = crate::nvidia::fb::find_in_bars(gpu, info.addr);
            if let Some(l) = loc {
                serial_println!(
                    "[gtx1650] boot framebuffer {:#x} is inside BAR{} at offset {:#x} (scanout is routed through this card)",
                    info.addr, l.bar_index, l.offset
                );
            } else {
                serial_println!(
                    "[gtx1650] boot framebuffer {:#x} is NOT inside any NVIDIA BAR - a different adapter owns the display",
                    info.addr
                );
            }
            loc
        }
        None => None,
    };

    // TU116-only: run the firmware bundle probe. Validates that every
    // embedded blob parses correctly and queries each Falcon engine
    // (SEC2 / GSP / NVDEC / FECS / GPCCS) for liveness. The probe never starts a CPU; it only touches HWCFG / CPUCTL reads
    if tu116::matches(gpu.device) {
        probe_tu116_firmware(&bar0_region);
    }

    // TU116-only: full GSP-RM bring-up pipeline. Three steps in order,
    // each non-fatal so the device still registers and the operator can
    // inspect state through the shell on any partial failure:
    //
    //   1) gsprm::load - parse GSP-RM ELF, stage .fwimage in phys-contig
    //      sysmem, build a radix3 page table, materialize the WPR-meta
    //      block. After this 'is_loaded()'' is true
    //   2) sec2::attempt_acr_v2 - run SEC2 ACR. On success the WPR2 lock
    //      is now active at the top of VRAM; that's the destination the
    //      booter will DMA into. WPR2 status is logged at the tail
    //   3) gsprm::boot_booter - hand the WPR-meta sysmem phys to the GSP
    //      booter_load HS image via MAILBOX0/1 and kick GSP CPUCTL. The
    //      booter DMAs the radix3-described pages into locked WPR2,
    //      verifies signatures, and jumps into GSP-RM
    if tu116::matches(gpu.device) {
        // Stage the GSP-RM image (parse ELF, build radix3, materialize the
        // WPR-meta in sysmem). This is fast and allocation-bound; no Falcon
        // is started, so it does not slow the boot. After this `is_loaded()`
        // is true and the device is ready for an on-demand boot.
        //
        // The actual engine bring-up (NVDEC scrubber -> SEC2 ACR -> booter ->
        // GSP handshake) is NOT run here on purpose: those kick Falcons and
        // poll for a halt that, until the ACR WPR2-lock gate is solved, never
        // comes (which previously added tens of seconds of MMIO spinning to
        // every kernel boot. Run it explicitly with `nvidia gsp-rm-boot-full`
        let gsp_rm_fw = super::tu116_fw::gsp_rm_570();
        match super::gsprm::load(&bar0_region, gsp_rm_fw.bytes()) {
            Ok(rep) => serial_println!(
                "[gtx1650/tu116] gsp-rm staged: fwimage={}B sig={}B radix3@{:#x} meta@{:#x} resolves={} (run 'nvidia gsp-rm-boot-full' to boot)",
                rep.fwimage_len, rep.signature_len,
                rep.radix3_root_phys, rep.meta_phys, rep.radix3_resolves
            ),
            Err(e) => serial_println!("[gtx1650/tu116] gsp-rm load failed: {:?}", e),
        }
    }

    // Register into the global slot. Further subsystems will reach in through crate::nvidia::with_gtx1650
    let dev = Gtx1650 {
        pci: gpu.clone(),
        bar0: bar0_region,
        bar1_phys: bar1.phys,
        bar1_size: bar1.size,
        bar3_phys: bar3.phys,
        bar3_size: bar3.size,
        chip,
        model_name,
        caps,
        boot42,
        boot_fb,
    };
    crate::nvidia::set_active_gtx1650(dev);

    // Without GSP firmware and VBIOS devinit we stop here: any further
    // engine writes would hang
    Ok(())
}

/// Probe every TU116 firmware blob and every Falcon engine that consumes
/// one. This step is non-destructive:
///     For each embedded blob, parse the NVFW container header (where
///     present) and log magic, version, payload size and offsets.
///     For SEC2 / GSP / NVDEC / FECS / GPCCS, read FALCON_HWCFG and
///     FALCON_CPUCTL. Engines that report a non-zero IMEM/DMEM size are
///     "alive" and ready to receive an upload; engines that read all-ones
///     are gated by PMC_ENABLE or by floor-sweeping
///
/// What this does NOT do (and why we stop here for now): firmware upload
/// requires either (a) DMA from a contiguous VRAM/sysmem buffer, which
/// in turn needs a working PFB MMU programmed by VBIOS devinit, or (b)
/// host-PIO via IMEM/DMEM ports. Path (b) works for the booter and the
/// SEC2 ACR bootloader, but the booter then expects a signed image
/// already staged in VRAM by GSP-RM-init RPC, which is the full GSP
/// driver. Until that infrastructure lands, kicking CPUCTL would just
/// halt with a signature error
fn probe_tu116_firmware(bar0: &MmioRegion) {
    use super::tu116_fw::{self, Engine as FwEngine, NvfwBinHdr};
    use super::falcon::{
        Engine, PSEC_BASE, PGSP_BASE, PNVDEC_BASE, PFECS_BASE,
        PGPCCS0_BASE, PGPCCS1_BASE,
    };

    serial_println!(
        "[gtx1650/tu116] firmware bundle: {} blobs, {} bytes total",
        tu116_fw::TU116_FIRMWARE.len(),
        tu116_fw::total_size()
    );

    // Per-blob report. Wrapped blobs get their NVFW header logged; raw
    // blobs (sec2 image, fecs / gpccs segments, sw_ host tables) only
    // get a size line
    for b in tu116_fw::TU116_FIRMWARE {
        let engine_str = match b.engine {
            FwEngine::Sec2   => "sec2",
            FwEngine::Gsp    => "gsp",
            FwEngine::Nvdec  => "nvdec",
            FwEngine::Fecs   => "fecs",
            FwEngine::Gpccs  => "gpccs",
            FwEngine::HostSw => "host",
        };
        // Fetch from the firmware store on demand. The buffer is dropped at
        // the end of each iteration, so this probe never pins firmware in RAM.
        let fw = match crate::fwload::request(b.path) {
            Ok(fw) => fw,
            Err(e) => {
                serial_println!(
                    "[gtx1650/tu116] {:<32} engine={:<5} unavailable ({:?})",
                    b.name, engine_str, e
                );
                continue;
            }
        };
        let bytes = fw.bytes();
        if b.wrapped {
            match NvfwBinHdr::parse(bytes) {
                Some(h) => serial_println!(
                    "[gtx1650/tu116] {:<32} engine={:<5} size={:>6} ver={} hdr@{:#x} data@{:#x}+{:#x}",
                    b.name, engine_str, bytes.len(),
                    h.bin_ver, h.header_offset, h.data_offset, h.data_size
                ),
                None => serial_println!(
                    "[gtx1650/tu116] {:<32} engine={:<5} size={:>6} (NVFW header parse failed)",
                    b.name, engine_str, bytes.len()
                ),
            }
        } else {
            serial_println!(
                "[gtx1650/tu116] {:<32} engine={:<5} size={:>6} (raw)",
                b.name, engine_str, bytes.len()
            );
        }
    }

    // Engine liveness probe
    let engines = [
        Engine::new(bar0, PSEC_BASE,    "sec2"),
        Engine::new(bar0, PGSP_BASE,    "gsp"),
        Engine::new(bar0, PNVDEC_BASE,  "nvdec"),
        Engine::new(bar0, PFECS_BASE,   "fecs"),
        Engine::new(bar0, PGPCCS0_BASE, "gpccs0"),
        Engine::new(bar0, PGPCCS1_BASE, "gpccs1"),
    ];
    for e in &engines {
        if e.is_alive() {
            serial_println!(
                "[gtx1650/tu116] falcon {:<6} @ BAR0+{:#x}: imem={} dmem={} halted={}",
                e.name, e.base(), e.imem_size(), e.dmem_size(), e.is_halted()
            );
        } else {
            serial_println!(
                "[gtx1650/tu116] falcon {:<6} @ BAR0+{:#x}: gated (PMC_ENABLE off or floor-swept)",
                e.name, e.base()
            );
        }
    }

    serial_println!(
        "[gtx1650/tu116] firmware staged in kernel image; engine upload pending GSP-RM bring-up"
    );
}

/// Walk PTOP_DEVICE_INFO and log the engines the hardware reports. Uses
/// the legacy single-word DEVICE_INFO encoding. The CHAIN bit marks a
/// continuation of the previous entry; we just log chains
fn walk_ptop(bar0: &MmioRegion) {
    let mut logged = 0u32;
    for i in 0..PTOP_DEVICE_INFO_COUNT {
        let entry = bar0.read32(PTOP_DEVICE_INFO + i * 4);
        if entry == 0 { continue; }
        let etype = (entry >> PTOP_ENTRY_TYPE_SHIFT) & PTOP_ENTRY_TYPE_MASK;
        let kind = match etype {
            0 => "NOT_VALID",
            PTOP_ENTRY_TYPE_DATA => "DATA",
            PTOP_ENTRY_TYPE_ENUM => "ENUM",
            _ => "other",
        };
        let chained = entry & PTOP_ENTRY_CHAIN != 0;
        serial_println!(
            "[gtx1650] PTOP[{:>2}] = {:#010x}  type={} ({}) chain={}",
            i, entry, etype, kind, chained
        );
        logged += 1;
        if logged >= 16 { break; } // keep the log short
    }
}

