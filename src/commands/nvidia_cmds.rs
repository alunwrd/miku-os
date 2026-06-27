// NVIDIA GPU command handlers - extracted from src/commands/system.rs
// All cmd_nvidia_ helpers have been moved here; the cmd_nvidia dispatcher*
// remains the sole public entry point

use crate::{cprint, cprintln};

pub fn cmd_nvidia(arg: &str) {
    match arg.trim() {
        "" | "info" | "status"           => cmd_nvidia_info(),
        "list" | "ls" | "gpus"           => cmd_nvidia_list(),
        "splash" | "test" | "draw"       => {
            crate::splash::draw();
            cprintln!(100, 220, 150, "  splash drawn - check the screen");
        }
        "debug" | "regs"                 => cmd_nvidia_debug(),
        "firmware" | "fw"                => cmd_nvidia_firmware(),
        "falcon" | "engines"             => cmd_nvidia_falcon(),
        "ungate" | "enable"              => cmd_nvidia_ungate(),
        "pmc-scan" | "pmcscan" | "scan"  => cmd_nvidia_pmc_scan(),
        "dma-state" | "dmastate" | "dma" => cmd_nvidia_dma_state(),
        "fbif-scan" | "fbifscan"         => cmd_nvidia_fbif_scan(),
        "fbif-decode" | "fbif"           => cmd_nvidia_fbif_decode(),
        "dma-test" | "dmatest" | "loopback" => cmd_nvidia_dma_test(),
        "imem-test" | "imemtest"         => cmd_nvidia_imem_test(),
        "acr-info" | "acrinfo" | "acr"   => cmd_nvidia_acr_info(),
        "temp" | "thermal" | "temperature" => cmd_nvidia_temp(),
        "gsp"                            => cmd_nvidia_gsp(),
        "gsp-rm" | "gsprm"               => cmd_nvidia_gsprm(),
        "gsp-rm-dryrun" | "gsprm-dryrun" => cmd_nvidia_gsprm_dryrun(),
        "gsp-rm-load" | "gsprm-load"     => cmd_nvidia_gsprm_load(),
        "gsp-rm-boot" | "gsprm-boot"     => cmd_nvidia_gsprm_boot(),
        "gsp-rm-boot-full" | "gsprm-boot-full" | "gsp-boot" => cmd_nvidia_gsprm_boot_full(),
        "gsp-bootargs" | "bootargs"      => cmd_nvidia_gsp_bootargs(),
        "nvdec-scrub" | "scrub" | "scrubber" => cmd_nvidia_nvdec_scrub(),
        "sec2-acr" | "sec2acr" | "acr-boot" => cmd_nvidia_sec2_acr(),
        "sec2-acr-v2" | "sec2acrv2" | "acr-boot-v2" => cmd_nvidia_sec2_acr_v2(),
        "wpr-state" | "wpr"              => cmd_nvidia_wpr_state(),
        "msgq-test" | "msgq"             => cmd_nvidia_msgq_test(),
        "rpc-test" | "rpc"               => cmd_nvidia_rpc_test(),
        "next" | "todo" | "roadmap"      => cmd_nvidia_next(),
        "help" | "?"                     => cmd_nvidia_help_local(),
        _ => {
            crate::println!("Usage: nvidia [info|list|debug|firmware|falcon|ungate|pmc-scan|dma-state|fbif-scan|fbif-decode|dma-test|imem-test|acr-info|temp|gsp|gsp-rm|gsp-rm-dryrun|gsp-rm-load|gsp-rm-boot|gsp-rm-boot-full|nvdec-scrub|sec2-acr|sec2-acr-v2|next|splash|help]");
        }
    }
}

fn cmd_nvidia_help_local() {
    cprintln!(118, 185, 0, "  nvidia subcommands");
    crate::println!("    info                - summary (default)");
    crate::println!("    list (ls)           - enumerate all recognized NVIDIA GPUs (any family)");
    crate::println!("    debug               - full BAR0 register dump (PMC, PBUS, PFIFO, PTOP)");
    crate::println!("    firmware (fw)       - list embedded TU116 blobs and their NVFW headers");
    crate::println!("    falcon              - per-engine (sec2/gsp/nvdec/fecs/gpccs) liveness");
    crate::println!("    ungate (enable)     - set PMC_ENABLE.GR + CE0 to bring up FECS / GPCCS / CE0");
    crate::println!("    pmc-scan            - read-only scan of PMC area (0x000..0x1000) to identify");
    crate::println!("                           NV_PMC_DEVICE_ENABLE / ELPG_ENABLE registers");
    crate::println!("    dma-state           - per-engine DMATRF* register snapshot + IDLE status");
    crate::println!("                           (read-only; verifies the DMA path is reachable)");
    crate::println!("    fbif-scan           - read-only sweep of FBIF window (+0x500..+0xa00) per");
    crate::println!("                           live engine to identify TRANSCFG / REGIONCFG offsets");
    crate::println!("    fbif-decode (fbif)  - decode all 8 TRANSCFG slots per live engine");
    crate::println!("                           (target / mem_type / cache flags, human-readable)");
    crate::println!("    dma-test            - end-to-end loopback: alloc DMA buffer, fill pattern,");
    crate::println!("                           program SEC2 TRANSCFG[7], DMA into DMEM, read+verify");
    crate::println!("    imem-test           - IMEM variant of dma-test: same path but target=IMEM,");
    crate::println!("                           set_imem_tag=true (the path used to load HS ucode)");
    crate::println!("    acr-info (acr)      - dump structure of every SEC2 ACR blob: NVFW container,");
    crate::println!("                           inner-header bytes, payload prefix (read-only)");
    crate::println!("    temp (thermal)      - read the on-die temperature sensor + thermal limits");
    crate::println!("    nvdec-scrub (scrub) - run NVDEC scrubber first-contact boot (zeroes WPR region)");
    crate::println!("    gsp                 - run GSP first-contact boot on demand");
    crate::println!("    gsp-rm (gsprm)      - VRAM size + WPR2 layout + GSP ABI scaffolding");
    crate::println!("    gsp-rm-dryrun       - self-test radix3 + WPR-meta path with a synthetic ELF");
    crate::println!("    gsp-rm-load         - parse embedded GSP-RM ELF, stage .fwimage, build radix3 + meta (pins state)");
    crate::println!("    gsp-bootargs        - build + verify libos/rmargs/CMDQ-MSGQ boot args in sysmem (no Falcon kick)");
    crate::println!("    gsp-rm-boot         - hand the pinned WPR-meta to the GSP booter and kick booter_load");
    crate::println!("    gsp-rm-boot-full    - drive the entire boot pipeline (scrub->load->ACR->WPR2->booter->handshake)");
    crate::println!("    sec2-acr            - first-contact SEC2 ACR boot: stage ahesasc in sysmem,");
    crate::println!("                           upload acr/bl into SEC2 IMEM (SECURE), kick, observe halt");
    crate::println!("    sec2-acr-v2         - same path but parse ahesasc HS layers and upload a real");
    crate::println!("                           flcn_bl_dmem_desc to SEC2 DMEM before kick");
    crate::println!("    wpr-state (wpr)     - dump PFB MMU WPR1/WPR2 lock registers (read-only)");
    crate::println!("    msgq-test (msgq)    - alloc + verify GSP CMDQ/MSGQ ring layout in sysmem");
    crate::println!("    rpc-test  (rpc)     - GSP RPC frame round-trip self-test (no GPU traffic)");
    crate::println!("    next                - inspect state and prescribe the next concrete step");
    crate::println!("    splash              - redraw the boot splash via the framebuffer");
}

// nvidia list - enumerate every NVIDIA GPU the driver recognized, across
// all families. The GTX 1650 (if present) runs the full firmware pipeline;
// any other card is brought up host-side only and listed from the generic
// registry
fn cmd_nvidia_list() {
    use crate::nvidia::{generic, with_gtx1650};

    cprintln!(118, 185, 0, "  recognized NVIDIA GPUs");

    let mut total = 0usize;

    let gtx = with_gtx1650(|dev| {
        crate::println!(
            "    [{:04x}:{:04x}] {} ({}) - full driver (firmware pipeline)",
            dev.pci.vendor, dev.pci.device, dev.model_name, dev.chip.codename()
        );
    });
    if gtx.is_some() { total += 1; }

    generic::with_generic_gpus(|gpus| {
        for g in gpus {
            crate::println!(
                "    [{:04x}:{:04x}] {} ({}) - host-side only{}",
                g.pci.vendor, g.pci.device, g.profile.model_hint, g.chip.codename(),
                if g.profile.has_firmware { "" } else { " (no firmware bundle)" }
            );
        }
        total += gpus.len();
    });

    if total == 0 {
        crate::print_warn!("  no NVIDIA GPU recognized");
        crate::println!("  (on QEMU without VFIO this is expected; the host owns the device)");
    } else {
        crate::println!("");
        crate::println!("  {} GPU(s); {} brought up host-side", total, generic::count());
    }
}

fn cmd_nvidia_info() {
    use crate::nvidia::{gtx1650, with_gtx1650};

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  NVIDIA {} ({})", dev.model_name, dev.chip.codename());
        crate::println!(
            "    pci:         {:04x}:{:04x} @ {:02x}:{:02x}.{:x} rev={:#x}",
            dev.pci.vendor, dev.pci.device,
            dev.pci.bus, dev.pci.dev, dev.pci.func, dev.pci.revision
        );
        crate::println!(
            "    subsys:      {:04x}:{:04x}",
            dev.pci.subsystem_vendor, dev.pci.subsystem_device
        );
        crate::println!(
            "    chip:        arch={:?} impl={:#x} rev={}.{} step={} boot42={:#010x}",
            dev.chip.arch, dev.chip.implementation,
            dev.chip.major_rev, dev.chip.minor_rev, dev.chip.stepping,
            dev.boot42
        );
        crate::println!(
            "    bar0:        phys={:#x} size={} KB",
            dev.bar0.virt_base() - crate::grub::hhdm(), dev.bar0.size() / 1024
        );
        if dev.bar1_size != 0 {
            crate::println!(
                "    bar1 (FB):   phys={:#x} size={} MB",
                dev.bar1_phys, dev.bar1_size / (1024 * 1024)
            );
        }
        if dev.bar3_size != 0 {
            crate::println!(
                "    bar3 (USER): phys={:#x} size={} MB",
                dev.bar3_phys, dev.bar3_size / (1024 * 1024)
            );
        }

        let enable = gtx1650::pmc::read_enable(&dev.bar0);
        let straps = gtx1650::pmc::read_straps(&dev.bar0);
        crate::println!("    pmc_enable:  {:#010x}", enable);
        crate::println!("    pmc_boot_2:  {:#010x}  (straps)", straps);

        let freq = gtx1650::pmc::read_ptimer_freq(&dev.bar0);
        let hz = freq.input_clock_hz();
        if hz != 0 {
            crate::println!(
                "    ptimer:      num={} denom={} ({} MHz)",
                freq.numerator, freq.denominator, hz / 1_000_000
            );
        } else {
            crate::println!(
                "    ptimer:      num={} denom={} (not programmed)",
                freq.numerator, freq.denominator
            );
        }
        crate::println!("    time:        {} ns", dev.read_ptimer_ns());

        if let Some(m) = dev.caps.msi {
            crate::println!(
                "    msi:         cap@{:#x} 64bit={} maskable={} max_vec={}",
                m.cap_offset, m.is_64bit, m.per_vector_masking,
                1u32 << m.multi_message_capable
            );
        } else {
            crate::println!("    msi:         absent");
        }
        if let Some(x) = dev.caps.msix {
            crate::println!(
                "    msi-x:       cap@{:#x} table_size={} bar{}@{:#x}",
                x.cap_offset, x.table_size, x.table_bir, x.table_offset
            );
        }
        crate::println!("    irq line:    {} pin:{}", dev.pci.irq_line, dev.pci.irq_pin);
        match dev.boot_fb {
            Some(loc) => cprintln!(100, 220, 150,
                "    scanout:     boot framebuffer is in BAR{} @ offset {:#x} (display is through this card)",
                loc.bar_index, loc.offset),
            None => cprintln!(220, 200, 80,
                "    scanout:     boot framebuffer is not in any of this card's BARs"),
        }
    });

    if shown.is_none() {
        crate::print_warn!("  no supported NVIDIA GPU active");
        crate::println!("  (driver currently brings up TU117/TU116-based GTX 1650 only (right now))");
    }
}

// nvidia debug - BAR0 register dump
//
// Dumps every register the host can read without engaging an engine. Useful
// when (a) on QEMU the card is not bound, in which case 'info' will print
// "no supported NVIDIA GPU" and this prints nothing; (b) on real hardware
// you want the raw values to compare against nouveau's expected ranges
fn cmd_nvidia_debug() {
    use crate::nvidia::{gtx1650, with_gtx1650};
    use crate::nvidia::gtx1650::regs::{
        PMC_BOOT_0, PMC_BOOT_1, PMC_BOOT_2, PMC_DEBUG_1, PMC_INTR_0, PMC_INTR_EN_0,
        PMC_ENABLE, PMC_BOOT_42,
        PBUS_INTR_0, PBUS_INTR_EN_0,
        PFIFO_INTR_0, PFIFO_INTR_EN_0,
        PTIMER_TIME_0, PTIMER_TIME_1, PTIMER_NUMERATOR, PTIMER_DENOMINATOR,
        PTOP_DEVICE_INFO, PTOP_DEVICE_INFO_COUNT,
    };

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  BAR0 register dump");
        let r = |off: u32| dev.bar0.read32(off);
        crate::println!("    PMC_BOOT_0       {:#010x}", r(PMC_BOOT_0));
        crate::println!("    PMC_BOOT_1       {:#010x}", r(PMC_BOOT_1));
        crate::println!("    PMC_BOOT_2       {:#010x}  (straps)", r(PMC_BOOT_2));
        crate::println!("    PMC_BOOT_42      {:#010x}  (extended chip ID)", r(PMC_BOOT_42));
        crate::println!("    PMC_DEBUG_1      {:#010x}", r(PMC_DEBUG_1));
        crate::println!("    PMC_INTR_0       {:#010x}  (pending top-level intrs)", r(PMC_INTR_0));
        crate::println!("    PMC_INTR_EN_0    {:#010x}  (top-level enable mask)", r(PMC_INTR_EN_0));
        let enable = r(PMC_ENABLE);
        crate::println!("    PMC_ENABLE       {:#010x}", enable);
        crate::println!("                       HOST={} GR={} PWR={} CE0={} DISP={}",
            (enable >> 8) & 1, (enable >> 12) & 1, (enable >> 13) & 1,
            (enable >> 14) & 1, (enable >> 30) & 1);
        crate::println!("    PBUS_INTR_0      {:#010x}", r(PBUS_INTR_0));
        crate::println!("    PBUS_INTR_EN_0   {:#010x}", r(PBUS_INTR_EN_0));
        crate::println!("    PFIFO_INTR_0     {:#010x}", r(PFIFO_INTR_0));
        crate::println!("    PFIFO_INTR_EN_0  {:#010x}", r(PFIFO_INTR_EN_0));
        let t0 = r(PTIMER_TIME_0); let t1 = r(PTIMER_TIME_1);
        crate::println!("    PTIMER           hi={:#010x} lo={:#010x} ({} ns)",
            t1, t0, ((t1 as u64) << 32) | (t0 as u64));
        crate::println!("    PTIMER_NUMER     {:#010x}", r(PTIMER_NUMERATOR));
        crate::println!("    PTIMER_DENOM     {:#010x}", r(PTIMER_DENOMINATOR));

        crate::println!("    PTOP_DEVICE_INFO (first nonzero entries):");
        let mut shown = 0;
        for i in 0..PTOP_DEVICE_INFO_COUNT {
            let e = r(PTOP_DEVICE_INFO + i * 4);
            if e == 0 { continue; }
            crate::println!("      [{:>2}] {:#010x}", i, e);
            shown += 1;
            if shown >= 12 { break; }
        }

        let _ = gtx1650::pmc::read_straps(&dev.bar0); // touch to keep import live
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound (expected on QEMU without VFIO passthrough)");
    }
}

// nvidia firmware - inventory the embedded TU116 blob bundle
//
// Works regardless of whether a card is bound, because the blobs are
// embedded into the kernel image at compile time
fn cmd_nvidia_firmware() {
    use crate::nvidia::gtx1650::tu116_fw::{self, NvfwBinHdr, Engine as FwEngine};

    cprintln!(118, 185, 0, "  TU116 firmware bundle (on firmware store, loaded on demand)");
    crate::println!("    blobs:        {}", tu116_fw::TU116_FIRMWARE.len());
    crate::println!("    total size:   {} bytes", tu116_fw::total_size());
    crate::println!("    gsp line:     {}", tu116_fw::GSP_DEFAULT_LINE);
    crate::println!("    store:        {}", if crate::fwload::available() { "mounted" } else { "NOT FOUND" });
    crate::println!("");
    crate::println!("    {:<32} {:<6} {:>7} {:<24}", "blob", "engine", "bytes", "container");
    for b in tu116_fw::TU116_FIRMWARE {
        let eng = match b.engine {
            FwEngine::Sec2   => "sec2",
            FwEngine::Gsp    => "gsp",
            FwEngine::Nvdec  => "nvdec",
            FwEngine::Fecs   => "fecs",
            FwEngine::Gpccs  => "gpccs",
            FwEngine::HostSw => "host",
        };
        // Pull each blob from the store on demand; the buffer is freed when
        // 'fw' drops at the end of the iteration.
        let fw = match crate::fwload::request(b.path) {
            Ok(fw) => fw,
            Err(e) => {
                crate::println!("    {:<32} {:<6} {:>7} unavailable ({:?})", b.name, eng, "-", e);
                continue;
            }
        };
        let bytes = fw.bytes();
        let container = if b.wrapped {
            if NvfwBinHdr::parse(bytes).is_some() { "NVFW v1" } else { "NVFW (parse failed)" }
        } else { "raw" };
        crate::println!("    {:<32} {:<6} {:>7} {:<24}", b.name, eng, bytes.len(), container);
        if b.wrapped {
            if let Some(h) = NvfwBinHdr::parse(bytes) {
                crate::println!(
                    "        ver={} bin_size={:#x} hdr@{:#x} data@{:#x}+{:#x}",
                    h.bin_ver, h.bin_size, h.header_offset, h.data_offset, h.data_size
                );
            }
        }
    }
    crate::println!("");
    crate::println!("  NOTE: GSP-RM image (gsp-570.144.bin) lives on the firmware store too,");
    crate::println!("        not in this bundle list. The booter alone cannot run a kernel-");
    crate::println!("        resident GPU OS without staging GSP-RM in WPR.");
}

// nvidia falcon - liveness probe of every Falcon engine
fn cmd_nvidia_falcon() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::falcon::{
        Engine, Liveness, PSEC_BASE, PGSP_BASE, PNVDEC_BASE, PFECS_BASE,
        PGPCCS0_BASE, PGPCCS1_BASE, FALCON_CPUCTL, FALCON_MAILBOX0,
        FALCON_MAILBOX1, CPUCTL_HALTED, CPUCTL_STOPPED,
    };

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  Falcon engines (BAR0 windows)");
        let engines = [
            ("sec2",   PSEC_BASE),
            ("gsp",    PGSP_BASE),
            ("nvdec",  PNVDEC_BASE),
            ("fecs",   PFECS_BASE),
            ("gpccs0", PGPCCS0_BASE),
            ("gpccs1", PGPCCS1_BASE),
        ];
        crate::println!("    {:<7} {:<10} {:<5} {:<5} {:<10} {:<10} {:<10}",
            "engine", "base", "imem", "dmem", "cpuctl", "mb0", "mb1");
        for (name, base) in engines {
            let e = Engine::new(&dev.bar0, base, name);
            match e.liveness() {
                Liveness::Alive => {
                    let cpuctl = e.read(FALCON_CPUCTL);
                    let state = if cpuctl & CPUCTL_HALTED  != 0 { "HALT" }
                           else if cpuctl & CPUCTL_STOPPED != 0 { "STOP" }
                           else                                  { "RUN " };
                    crate::println!(
                        "    {:<7} {:#010x} {:>5} {:>5} {:#010x}({}) {:#010x} {:#010x}",
                        name, base, e.imem_size(), e.dmem_size(), cpuctl, state,
                        e.read(FALCON_MAILBOX0), e.read(FALCON_MAILBOX1));
                }
                Liveness::GatedPriSentinel => {
                    crate::println!(
                        "    {:<7} {:#010x} GATED  PRI sentinel {:#010x} - engine off in PMC_ENABLE/DEVICE_ENABLE",
                        name, base, e.hwcfg());
                }
                Liveness::NoResponse => {
                    crate::println!(
                        "    {:<7} {:#010x} GATED  no response   HWCFG={:#010x}",
                        name, base, e.hwcfg());
                }
                Liveness::BadHwcfg => {
                    crate::println!(
                        "    {:<7} {:#010x} GATED  bad HWCFG     HWCFG={:#010x} (zero imem/dmem fields)",
                        name, base, e.hwcfg());
                }
            }
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound (expected on QEMU)");
    }
}

// nvidia ungate - re-apply PMC_ENABLE bits for PGRAPH + CE0 from the shell
// init.rs already does this once during boot; this lets the user re-trigger
// the ungate after experimentation without rebooting
fn cmd_nvidia_ungate() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::pmc;

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  PMC_ENABLE ungate (set GR + CE0)");
        let (before, after) = pmc::ungate_default_engines(&dev.bar0);
        crate::println!("    before: {:#010x}", before);
        crate::println!("    after:  {:#010x}", after);
        if before == after {
            crate::println!("    (bits were already set)");
        } else {
            crate::println!("    set bits: {:#010x}", after & !before);
            crate::println!("    rerun 'nvidia falcon' to confirm FECS / GPCCS now report alive");
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - nothing to ungate");
    }
}

// nvidia pmc-scan - read-only sweep of the PMC register area (BAR0 + 0x000..0x1000) looking for plausible device-enable / power-gating
// registers. Goal: identify the register that gates NVDEC on TU116
// without committing a guess at a hard-coded offset
//
// Heuristics:
//     skip zeros and the 0xFFFF_FFFF "no decode" pattern
//     flag values that look like bitmasks (popcount <= 8) or sentinels
//     (0xBADF_xxxx is a PRI bus reject - never a device enable)
//     annotate well-known offsets we already understand
//     collapse runs of identical reads into a single line so the output
//     is easy to skim on a real serial log
//
// This subcommand never writes; it is safe to run on a live system
fn cmd_nvidia_pmc_scan() {
    use crate::nvidia::with_gtx1650;

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  PMC area scan (BAR0 + 0x000..0x1000, read-only)");
        crate::println!("    legend: [E]=engine-enable candidate  [B]=bitmask  [S]=PRI sentinel");
        crate::println!("            [K]=known register             [I]=ID/version-like");
        crate::println!("    {:<8} {:<10} {:<3} {}", "offset", "value", "tag", "annotation");

        let mut last_val: u32 = 0xDEAD_BEEF; // value that cannot occur first
        let mut run_start: u32 = 0;
        let mut printed = 0u32;

        let mut flush = |start: u32, end: u32, val: u32| {
            if val == 0 || val == 0xFFFF_FFFF { return; }
            let tag = classify_pmc_word(val);
            let annot = annotate_pmc_offset(start);
            if start == end {
                crate::println!("    {:#06x}    {:#010x} {:<3} {}",
                    start, val, tag, annot);
            } else {
                crate::println!("    {:#06x}+   {:#010x} {:<3} {} (repeats through {:#06x})",
                    start, val, tag, annot, end);
            }
        };

        // Sweep 0x000..0x1000 in 4-byte steps. 1024 reads -> negligible
        for off in (0..0x1000u32).step_by(4) {
            let v = dev.bar0.read32(off);
            if v == last_val && off != 0 {
                continue;
            }
            // Edge: emit the previous run (last_val held from run_start..off-4)
            if last_val != 0xDEAD_BEEF {
                flush(run_start, off - 4, last_val);
                if last_val != 0 && last_val != 0xFFFF_FFFF {
                    printed += 1;
                    if printed >= 64 {
                        crate::println!("    ... (truncated at 64 distinct values)");
                        return;
                    }
                }
            }
            last_val = v;
            run_start = off;
        }
        // Tail
        flush(run_start, 0x0FFCu32, last_val);

        crate::println!("");
        crate::println!("  candidates worth checking against open-gpu-kernel-modules:");
        crate::println!("    a [E]/[B]-tagged register that holds a bitmask similar in shape to");
        crate::println!("    PMC_ENABLE (current = {:#010x}) is the most likely engine-enable.",
            dev.bar0.read32(crate::nvidia::gtx1650::regs::PMC_ENABLE));
        crate::println!("    nvdec scrubber gating bit lives in one of these, NOT in PMC_ENABLE.");
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - pmc-scan only runs against real silicon");
    }
}

/// Classify a 32-bit register read into a short tag for the scan output
/// Heuristics: PMC bitmask registers have only a handful of bits set;
/// ID/version registers tend to have most of their bits in the upper
/// half; PRI sentinels match 0xBADF_xxxx
fn classify_pmc_word(v: u32) -> &'static str {
    use crate::nvidia::gtx1650::falcon::is_pri_sentinel;
    if is_pri_sentinel(v)         { return "S"; }
    let pop = v.count_ones();
    if pop == 0 || pop == 32      { return "-"; }
    if pop <= 8                   { return "EB"; } // enable-like bitmask
    if (v & 0xFFFF_0000) != 0 && (v & 0x0000_FFFF) != 0
        && pop > 16               { return "I";  } // ID-like
    "B"
}

/// Annotate well-known PMC-area offsets so the scan output reads itself
fn annotate_pmc_offset(off: u32) -> &'static str {
    use crate::nvidia::gtx1650::regs::*;
    match off {
        x if x == PMC_BOOT_0       => "PMC_BOOT_0 (chip ID)",
        x if x == PMC_BOOT_1       => "PMC_BOOT_1",
        x if x == PMC_BOOT_2       => "PMC_BOOT_2 (straps)",
        x if x == PMC_INTR_0       => "PMC_INTR_0 (pending)",
        x if x == PMC_INTR_EN_0    => "PMC_INTR_EN_0",
        x if x == PMC_ENABLE       => "PMC_ENABLE (engine gates)",
        x if x == PMC_DEBUG_1      => "PMC_DEBUG_1",
        x if x == PMC_BOOT_42      => "PMC_BOOT_42 (extended chip ID)",
        // Frequently-discussed Turing power/enable offsets. We do NOT assert these are correct on TU116; the scan exists to verify
        0x0140                     => "(?) ELPG_ENABLE candidate",
        0x0148                     => "(?) ELPG_ENABLE_1 candidate",
        0x0600                     => "(?) DEVICE_ENABLE candidate",
        0x0604                     => "(?) DEVICE_ENABLE_1 candidate",
        0x088C                     => "(?) DEVICE_ENABLE candidate (alt offset)",
        0x0C00                     => "(?) DEVICE_ENABLE candidate (alt offset)",
        _ => "",
    }
}

// nvidia dma-state - per-engine DMATRF snapshot + IDLE/ERROR decoding Read-only
// Used to confirm the Falcon DMA register window is reachable
// before we send our first DMA transfer through Engine::dma_load
fn cmd_nvidia_dma_state() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::falcon::{
        Engine, Liveness, PSEC_BASE, PGSP_BASE, PNVDEC_BASE, PFECS_BASE,
        FALCON_DMACTL, FALCON_DMATRFBASE, FALCON_DMATRFBASE1,
        FALCON_DMATRFMOFFS, FALCON_DMATRFCMD, FALCON_DMATRFFBOFFS,
        DMATRFCMD_IDLE, DMATRFCMD_ERROR,
    };

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  Falcon DMA register snapshot");
        let engines = [
            ("sec2",  PSEC_BASE),
            ("gsp",   PGSP_BASE),
            ("nvdec", PNVDEC_BASE),
            ("fecs",  PFECS_BASE),
        ];
        for (name, base) in engines {
            let e = Engine::new(&dev.bar0, base, name);
            if !matches!(e.liveness(), Liveness::Alive) {
                crate::println!("    {:<6} {:#010x} (engine not alive - skipping DMA snapshot)",
                    name, base);
                continue;
            }
            let cmd      = e.read(FALCON_DMATRFCMD);
            let dmactl   = e.read(FALCON_DMACTL);
            let base_lo  = e.read(FALCON_DMATRFBASE);
            let base_hi  = e.read(FALCON_DMATRFBASE1);
            let moffs    = e.read(FALCON_DMATRFMOFFS);
            let fboffs   = e.read(FALCON_DMATRFFBOFFS);
            let idle     = cmd & DMATRFCMD_IDLE  != 0;
            let err      = cmd & DMATRFCMD_ERROR != 0;
            crate::println!("    {:<6} @ {:#010x}", name, base);
            crate::println!("      DMACTL    = {:#010x}", dmactl);
            crate::println!("      DMATRFCMD = {:#010x}  idle={} error={}",
                cmd, idle as u32, err as u32);
            crate::println!("      DMATRFBASE  hi:lo = {:#010x}:{:#010x}  (phys = {:#018x})",
                base_hi, base_lo,
                ((base_hi as u64) << 40) | ((base_lo as u64) << 8));
            crate::println!("      DMATRFMOFFS  = {:#010x}  (target byte offset in IMEM/DMEM)", moffs);
            crate::println!("      DMATRFFBOFFS = {:#010x}  (source chunk offset, 256B units)", fboffs);
        }
        crate::println!("");
        crate::println!("  notes:");
        crate::println!("      all values are pre-DMA-setup; we have not configured any FBIF yet.");
        crate::println!("      idle=1 means the engine is ready to accept a dma_load call once");
        crate::println!("      step (A) lands a buffer allocator + FBIF aperture programming.");
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - dma-state needs real silicon");
    }
}

// nvidia fbif-scan - read-only sweep of each live falcon's FBIF window
//
// Goal: identify the actual offset of FBIF_TRANSCFG (per-context-DMA
// aperture descriptor) and FBIF_REGIONCFG (WPR region selector) on TU116
// without committing to a hard-coded guess. Different sources (nouveau
// branches, envytools, open-gpu-kernel-modules) place FBIF differently
// across NV generations - typical Turing layouts are at engine_base +
// 0x600 or +0x800 with FBIF registers in the first 0x100 bytes
//
// Heuristics applied to identify FBIF registers:
//     TRANSCFG entries (8 of them, 4 bytes each) read as 0 at boot if
//     no driver has programmed them, or as a bitmask with low bits set
//     (target type + access mode) if hw POST set defaults
//     REGIONCFG is a 4-bit-per-context array (32 bits, 8 contexts)
//     PRI sentinels (0xBADF_xxxx) flag dead/gated regions and let us
//     bound the actually-decoded part of the FBIF window
//
// Read-only: safe on any live system
fn cmd_nvidia_fbif_scan() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::falcon::{
        Engine, Liveness, PSEC_BASE, PGSP_BASE, PFECS_BASE, is_pri_sentinel,
    };

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  FBIF window scan (read-only, +0x500..+0xa00 per engine)");
        let engines = [
            ("sec2", PSEC_BASE),
            ("gsp",  PGSP_BASE),
            ("fecs", PFECS_BASE),
        ];

        for (name, base) in engines {
            let e = Engine::new(&dev.bar0, base, name);
            if !matches!(e.liveness(), Liveness::Alive) {
                crate::println!("    {:<5} @ {:#010x}: not alive, skipping", name, base);
                continue;
            }
            crate::println!("     {} @ {:#010x} ", name, base);

            let mut last_val: u32 = 0xDEAD_BEEF;
            let mut run_start: u32 = 0;
            let mut nonzero_seen = 0u32;

            // Sweep +0x500..+0xa00 in 4-byte steps
            for off in (0x500u32..0xa00).step_by(4) {
                let v = e.read(off);
                if v == last_val && off != 0x500 {
                    continue;
                }
                if last_val != 0xDEAD_BEEF {
                    fbif_emit_run(run_start, off - 4, last_val);
                    if last_val != 0 && last_val != 0xFFFF_FFFF && !is_pri_sentinel(last_val) {
                        nonzero_seen += 1;
                    }
                }
                last_val = v;
                run_start = off;
            }
            // Tail.
            fbif_emit_run(run_start, 0x9FCu32, last_val);

            if nonzero_seen == 0 {
                crate::println!("      (entire window reads zero or sentinel - FBIF likely unprogrammed)");
            }
            crate::println!("");
        }

        crate::println!("  reading the output:");
        crate::println!("      8 consecutive small bitmasks at fixed stride (typically 4B) ==> TRANSCFG[0..7]");
        crate::println!("      a single 32-bit value with 4-bit fields per ctx     ==> REGIONCFG");
        crate::println!("      the first non-sentinel offset is usually FBIF_BASE relative to engine");
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - fbif-scan needs real silicon");
    }
}

/// Emit one line for a run of identical FBIF reads, suppressing zeros and
/// PRI-sentinel runs unless they are the only thing in a region
fn fbif_emit_run(start: u32, end: u32, val: u32) {
    use crate::nvidia::gtx1650::falcon::is_pri_sentinel;
    if val == 0 || val == 0xFFFF_FFFF { return; }
    let tag = if is_pri_sentinel(val) { "S" }
              else if val.count_ones() <= 8 { "B" }
              else { "?" };
    if start == end {
        crate::println!("      +{:#05x}    {:#010x} [{}]", start, val, tag);
    } else {
        crate::println!("      +{:#05x}+   {:#010x} [{}] (repeats through +{:#05x})",
            start, val, tag, end);
    }
}

// nvidia fbif-decode - decode all 8 TRANSCFG slots for each live engine
// Read-only. Confirms the bit layout we're about to program against
fn cmd_nvidia_fbif_decode() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::falcon::{
        Engine, Liveness, PSEC_BASE, PGSP_BASE, PFECS_BASE,
    };
    use crate::nvidia::gtx1650::fbif::{
        self, FbifTarget, FbifMemType, FBIF_TRANSCFG_COUNT,
    };

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  FBIF TRANSCFG decode (read-only)");
        let engines = [
            ("sec2", PSEC_BASE),
            ("gsp",  PGSP_BASE),
            ("fecs", PFECS_BASE),
        ];
        for (name, base) in engines {
            let e = Engine::new(&dev.bar0, base, name);
            if !matches!(e.liveness(), Liveness::Alive) {
                crate::println!("    {:<5} @ {:#010x}: not alive, skipping", name, base);
                continue;
            }
            crate::println!("     {} @ {:#010x} (FBIF +{:#x}) ",
                name, base, fbif::FBIF_BASE_OFFSET);
            for ctx in 0..FBIF_TRANSCFG_COUNT {
                let raw = fbif::read_transcfg_raw(&e, ctx);
                let d = fbif::decode_transcfg(raw);
                let target = match d.target {
                    FbifTarget::LocalFb            => "LOCAL_FB         ",
                    FbifTarget::CoherentSysmem     => "COHERENT_SYSMEM  ",
                    FbifTarget::NoncoherentSysmem  => "NONCOHERENT_SYSME",
                };
                let mt = match d.mem_type {
                    FbifMemType::Virtual  => "virt",
                    FbifMemType::Physical => "phys",
                };
                fn f(b: bool, s: &'static str) -> &'static str {
                    if b { s } else { "" }
                }
                crate::println!(
                    "      ctx[{}] = {:#010x}  {} {} {}{}{}{}{}{}",
                    ctx, raw, target, mt,
                    f(d.l2c_wr, "L2C_WR "), f(d.l2c_rd, "L2C_RD "),
                    f(d.wachk0, "WACHK0 "), f(d.wachk1, "WACHK1 "),
                    f(d.rachk0, "RACHK0 "), f(d.rachk1, "RACHK1 "));
            }
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - fbif-decode needs real silicon");
    }
}

// nvidia dma-test - end-to-end loopback against SEC2 (which is alive and HALTed at boot and therefore safe to drive)
//
// Sequence:
//   1) allocate a single 4 KiB phys-contiguous page from the kernel
//   2) fill it with a 32-bit pattern that encodes the source offset
//   3) issue an sfence so the writes are visible to a DMA reader
//   4) program SEC2 TRANSCFG[7] for COHERENT_SYSMEM + physical addressing
//   5) call Engine::dma_load to move 256 B from the buffer into SEC2 DMEM (offset 0), through ctxdma=7
//   6) read DMEM back through the PIO DMEM_C/DMEM_D port and compare to the original buffer
//   7) report which 32-bit slots match / mismatch
//
// Failure modes we expect to recover from cleanly:
//     DMATRFCMD_ERROR set: layout mismatch -> reported, buffer freed
//     dma_load returns Timeout: GPU hung in DMA -> diagnostic, buffer freed
//     read-back mismatch: aperture/address-translation issue -> diagnostic
fn cmd_nvidia_dma_test() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::falcon::{
        Engine, Liveness, PSEC_BASE, FALCON_DMEM_C0, FALCON_DMEM_D0,
        DmaTransfer, DmaTarget, DmaDirection, MEM_C_AINCR,
    };
    use crate::nvidia::gtx1650::fbif::{self, FbifTarget, FbifMemType};
    use crate::nvidia::gtx1650::dma_buf::DmaBuffer;

    const CTX:        u8  = 7;     // last TRANSCFG slot
    const CHUNK_BYTES: u32 = 256;   // size_log2=6
    const PATTERN_BASE: u32 = 0xCAFE_0000;

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  DMA loopback test (SEC2 ctxdma={})", CTX);

        // 0. SEC2 must be alive + halted. We never drive a running falcon
        let sec2 = Engine::new(&dev.bar0, PSEC_BASE, "sec2");
        if !matches!(sec2.liveness(), Liveness::Alive) {
            crate::print_warn!("    sec2 not alive ({:?}), aborting", sec2.liveness());
            return;
        }
        if !sec2.is_halted() {
            crate::print_warn!("    sec2 not halted (cpuctl={:#x}), aborting...", sec2.read(crate::nvidia::gtx1650::falcon::FALCON_CPUCTL));
            return;
        }
        crate::println!("    sec2 alive and halted, dmem={} B", sec2.dmem_size());

        // 1) Allocate one phys-contiguous page (4 KiB)
        let mut buf = match DmaBuffer::alloc(1) {
            Ok(b) => b,
            Err(e) => {
                crate::print_warn!("    DmaBuffer::alloc failed: {:?}", e);
                return;
            }
        };
        crate::println!("    buffer phys = {:#x}, size = {} B", buf.phys(), buf.size());

        // 2) Fill the first chunk with a recognizable pattern: each 4-byte
        //    slot holds (PATTERN_BASE | offset/4) so a partial transfer is visible
        {
            let s = buf.as_mut_slice();
            for i in 0..(CHUNK_BYTES as usize / 4) {
                let val = PATTERN_BASE | (i as u32);
                s[i*4..i*4+4].copy_from_slice(&val.to_le_bytes());
            }
        }
        DmaBuffer::write_barrier();

        // 3) Snapshot existing TRANSCFG[7] + FBIF_CTL so we can restore on exit
        let prev_transcfg = fbif::read_transcfg_raw(&sec2, CTX);
        let prev_fbif_ctl = sec2.read(fbif::FBIF_CTL_OFFSET);
        crate::println!("    sec2 TRANSCFG[{}] before = {:#010x}", CTX, prev_transcfg);
        crate::println!("    sec2 FBIF_CTL     before = {:#010x}", prev_fbif_ctl);

        // 4) Program TRANSCFG[7] for NONCOHERENT_SYSMEM + physical addressing
        //    NONCOHERENT is the path used by Nouveau/OpenGPU for direct PCIe
        //    DMA on Turing; CPU side flushes store-buffer via
        //    DmaBuffer::write_barrier() above
        let new_transcfg = fbif::program_transcfg(
            &sec2, CTX, FbifTarget::NoncoherentSysmem, FbifMemType::Physical,
        );
        crate::println!("    sec2 TRANSCFG[{}] after  = {:#010x} (target=NONCOHERENT_SYSMEM, phys)",
            CTX, new_transcfg);

        // 4b) Set FBIF_CTL.ALLOW_PHYS_NO_CTX. Without this bit, the Falcon
        //     silently drops fetches in TRANSCFG physical mode (kick echo
        //     returns, but idle=0 error=0 remains forever). Nouveau/OpenGPU
        //     set this bit during initialization for host-driven ucode-load
        sec2.write(fbif::FBIF_CTL_OFFSET, prev_fbif_ctl | fbif::FBIF_CTL_ALLOW_PHYS_NO_CTX);
        let new_fbif_ctl = sec2.read(fbif::FBIF_CTL_OFFSET);
        crate::println!("    sec2 FBIF_CTL     after  = {:#010x} (ALLOW_PHYS_NO_CTX set)",
            new_fbif_ctl);

        // 5) Kick the DMA: sysmem -> SEC2 DMEM, 256 B chunk, ctxdma=7
        let xfer = DmaTransfer {
            src_phys:      buf.phys(),
            src_off_bytes: 0,
            dst_off_bytes: 0,
            size_log2:     6,           // 4 << 6 = 256 B
            ctxdma:        CTX,
            target:        DmaTarget::Dmem,
            dir:           DmaDirection::ToFalcon,
            set_imem_tag:  false,
        };
        match sec2.dma_load(xfer) {
            Ok(()) => crate::println!("    dma_load: OK (256 B sysmem -> DMEM)"),
            Err(e) => {
                crate::print_warn!("    dma_load failed: {:?}", e);
                // Post-mortem snapshot to distinguish stuck-busy from
                // ERROR-bit from silent-reject
                use crate::nvidia::gtx1650::falcon::{
                    FALCON_DMATRFCMD, FALCON_DMACTL,
                    FALCON_DMATRFBASE, FALCON_DMATRFBASE1,
                    FALCON_DMATRFFBOFFS, FALCON_DMATRFMOFFS,
                };
                let cmd     = sec2.read(FALCON_DMATRFCMD);
                let dmactl  = sec2.read(FALCON_DMACTL);
                let base    = sec2.read(FALCON_DMATRFBASE);
                let base1   = sec2.read(FALCON_DMATRFBASE1);
                let fboffs  = sec2.read(FALCON_DMATRFFBOFFS);
                let moffs   = sec2.read(FALCON_DMATRFMOFFS);
                let tcfg    = fbif::read_transcfg_raw(&sec2, CTX);
                crate::println!("    post: DMATRFCMD ={:#010x}  idle={} error={}",
                    cmd, (cmd >> 1) & 1, (cmd >> 25) & 1);
                crate::println!("    post: DMACTL    ={:#010x}", dmactl);
                crate::println!("    post: BASE/BASE1={:#010x}/{:#010x}  FBOFFS={:#x} MOFFS={:#x}",
                    base, base1, fboffs, moffs);
                let post_ctl = sec2.read(fbif::FBIF_CTL_OFFSET);
                crate::println!("    post: TRANSCFG[{}]={:#010x}  FBIF_CTL={:#010x}",
                    CTX, tcfg, post_ctl);
                // Restore TRANSCFG + FBIF_CTL before returning
                sec2.write(fbif::transcfg_offset(CTX), prev_transcfg);
                sec2.write(fbif::FBIF_CTL_OFFSET, prev_fbif_ctl);
                return;
            }
        }

        // 6) Read DMEM back via the PIO port
        sec2.write(FALCON_DMEM_C0, 0 | MEM_C_AINCR);
        let mut readback = [0u32; 64]; // 256 B / 4
        for w in &mut readback {
            *w = sec2.read(FALCON_DMEM_D0);
        }

        // 7) Compare against the source buffer
        let mut mismatches = 0u32;
        let src = buf.as_slice();
        for i in 0..readback.len() {
            let expect = u32::from_le_bytes([
                src[i*4], src[i*4+1], src[i*4+2], src[i*4+3],
            ]);
            if readback[i] != expect {
                if mismatches < 4 {
                    crate::print_warn!("    slot[{:>2}]: dmem={:#010x} src={:#010x}",
                        i, readback[i], expect);
                }
                mismatches += 1;
            }
        }
        if mismatches == 0 {
            cprintln!(100, 220, 150, "    pass: 64/64 u32 slots match across the DMA path");
        } else {
            crate::print_warn!("    fail: {} of 64 u32 slots mismatched", mismatches);
        }

        // Always restore TRANSCFG + FBIF_CTL so we don't leave the engine
        // with a sysmem aperture armed or ALLOW_PHYS_NO_CTX flipped on
        sec2.write(fbif::transcfg_offset(CTX), prev_transcfg);
        sec2.write(fbif::FBIF_CTL_OFFSET, prev_fbif_ctl);
        let restored = fbif::read_transcfg_raw(&sec2, CTX);
        crate::println!("    sec2 TRANSCFG[{}] restored to {:#010x}", CTX, restored);
        // buf drops here, frames are returned to pmm
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - dma-test needs real silicon");
    }
}

// nvidia imem-test - IMEM variant of dma-test. Validates that
// dma_load(target=Imem, set_imem_tag=true) actually moves bytes from sysmem
// to SEC2 IMEM. This is the path we'll later use to upload ACR HS ucode
// IMEM may be PRIV-locked for readback on some SKUs; in that case the bus
// returns 0xBADFxxxx sentinels, which we report distinctly from a mismatch
// because dma_load reporting OK already proves the write side worked
fn cmd_nvidia_imem_test() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::falcon::{
        Engine, Liveness, PSEC_BASE, FALCON_IMEM_C0, FALCON_IMEM_D0,
        DmaTransfer, DmaTarget, DmaDirection, MEM_C_AINCR,
    };
    use crate::nvidia::gtx1650::fbif::{self, FbifTarget, FbifMemType};
    use crate::nvidia::gtx1650::dma_buf::DmaBuffer;

    const CTX:          u8  = 7;
    const CHUNK_BYTES:  u32 = 256;
    const PATTERN_BASE: u32 = 0xC0DE_0000;

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  IMEM loopback test (SEC2 ctxdma={})", CTX);

        let sec2 = Engine::new(&dev.bar0, PSEC_BASE, "sec2");
        if !matches!(sec2.liveness(), Liveness::Alive) {
            crate::print_warn!("    sec2 not alive ({:?}), aborting", sec2.liveness());
            return;
        }
        if !sec2.is_halted() {
            crate::print_warn!("    sec2 not halted (cpuctl={:#x}), aborting",
                sec2.read(crate::nvidia::gtx1650::falcon::FALCON_CPUCTL));
            return;
        }
        crate::println!("    sec2 alive and halted, imem={} B", sec2.imem_size());

        let mut buf = match DmaBuffer::alloc(1) {
            Ok(b) => b,
            Err(e) => {
                crate::print_warn!("    DmaBuffer::alloc failed: {:?}", e);
                return;
            }
        };
        crate::println!("    buffer phys = {:#x}, size = {} B", buf.phys(), buf.size());

        // Pattern 0xC0DE_xxxx - distinct from dma-test's 0xCAFE so a stale
        // DMEM fragment never accidentally looks like an IMEM-test pass
        {
            let s = buf.as_mut_slice();
            for i in 0..(CHUNK_BYTES as usize / 4) {
                let val = PATTERN_BASE | (i as u32);
                s[i*4..i*4+4].copy_from_slice(&val.to_le_bytes());
            }
        }
        DmaBuffer::write_barrier();

        let prev_transcfg = fbif::read_transcfg_raw(&sec2, CTX);
        let prev_fbif_ctl = sec2.read(fbif::FBIF_CTL_OFFSET);
        crate::println!("    sec2 TRANSCFG[{}] before = {:#010x}", CTX, prev_transcfg);
        crate::println!("    sec2 FBIF_CTL     before = {:#010x}", prev_fbif_ctl);

        let new_transcfg = fbif::program_transcfg(
            &sec2, CTX, FbifTarget::NoncoherentSysmem, FbifMemType::Physical,
        );
        crate::println!("    sec2 TRANSCFG[{}] after  = {:#010x} (NONCOHERENT_SYSMEM, phys)",
            CTX, new_transcfg);
        sec2.write(fbif::FBIF_CTL_OFFSET, prev_fbif_ctl | fbif::FBIF_CTL_ALLOW_PHYS_NO_CTX);
        crate::println!("    sec2 FBIF_CTL     after  = {:#010x} (ALLOW_PHYS_NO_CTX set)",
            sec2.read(fbif::FBIF_CTL_OFFSET));

        // Kick: sysmem -> SEC2 IMEM, 256 B chunk, ctxdma=7, set_imem_tag=true.
        // set_imem_tag tells the engine to update the IMEM virtual-tag table
        // for the destination 256-byte block (mandatory for executable code,
        // harmless for a data-only loopback)
        let xfer = DmaTransfer {
            src_phys:      buf.phys(),
            src_off_bytes: 0,
            dst_off_bytes: 0,
            size_log2:     6,
            ctxdma:        CTX,
            target:        DmaTarget::Imem,
            dir:           DmaDirection::ToFalcon,
            set_imem_tag:  true,
        };
        match sec2.dma_load(xfer) {
            Ok(()) => crate::println!("    dma_load: ok (256 B sysmem -> IMEM)"),
            Err(e) => {
                crate::print_warn!("    dma_load failed: {:?}", e);
                use crate::nvidia::gtx1650::falcon::{
                    FALCON_DMATRFCMD, FALCON_DMACTL,
                    FALCON_DMATRFBASE, FALCON_DMATRFBASE1,
                    FALCON_DMATRFFBOFFS, FALCON_DMATRFMOFFS,
                };
                let cmd     = sec2.read(FALCON_DMATRFCMD);
                let dmactl  = sec2.read(FALCON_DMACTL);
                let base    = sec2.read(FALCON_DMATRFBASE);
                let base1   = sec2.read(FALCON_DMATRFBASE1);
                let fboffs  = sec2.read(FALCON_DMATRFFBOFFS);
                let moffs   = sec2.read(FALCON_DMATRFMOFFS);
                let tcfg    = fbif::read_transcfg_raw(&sec2, CTX);
                let post_ctl = sec2.read(fbif::FBIF_CTL_OFFSET);
                crate::println!("    post: DMATRFCMD ={:#010x}  idle={} error={}",
                    cmd, (cmd >> 1) & 1, (cmd >> 25) & 1);
                crate::println!("    post: DMACTL    ={:#010x}", dmactl);
                crate::println!("    post: BASE/BASE1={:#010x}/{:#010x}  FBOFFS={:#x} MOFFS={:#x}",
                    base, base1, fboffs, moffs);
                crate::println!("    post: TRANSCFG[{}]={:#010x}  FBIF_CTL={:#010x}",
                    CTX, tcfg, post_ctl);
                sec2.write(fbif::transcfg_offset(CTX), prev_transcfg);
                sec2.write(fbif::FBIF_CTL_OFFSET, prev_fbif_ctl);
                return;
            }
        }

        // IMEM port read: byte offset 0 / AINCR. If IMEM is PRIV-locked the
        // bus returns 0xBADFxxxx sentinels; treat that as "unreadable from
        // host PRIV but write side validated by dma_load=ok"
        sec2.write(FALCON_IMEM_C0, MEM_C_AINCR);
        let mut readback = [0u32; 64];
        for w in &mut readback {
            *w = sec2.read(FALCON_IMEM_D0);
        }

        let priv_locked = readback.iter().all(|w| (w & 0xFFFF_0000) == 0xBADF_0000);
        if priv_locked {
            crate::println!("    IMEM port is PRIV-locked (all reads = 0xBADFxxxx);");
            crate::println!("    dma_load reported ok - write side validated by hw");
            crate::println!("    first 4 readback words: {:#010x} {:#010x} {:#010x} {:#010x}",
                readback[0], readback[1], readback[2], readback[3]);
        } else {
            let mut mismatches = 0u32;
            let src = buf.as_slice();
            for i in 0..readback.len() {
                let expect = u32::from_le_bytes([
                    src[i*4], src[i*4+1], src[i*4+2], src[i*4+3],
                ]);
                if readback[i] != expect {
                    if mismatches < 4 {
                        crate::print_warn!("    slot[{:>2}]: imem={:#010x} src={:#010x}",
                            i, readback[i], expect);
                    }
                    mismatches += 1;
                }
            }
            if mismatches == 0 {
                cprintln!(100, 220, 150,
                    "    PASS: 64/64 u32 slots match across the IMEM DMA path");
            } else {
                crate::print_warn!("    fail: {} of 64 u32 slots mismatched", mismatches);
            }
        }

        sec2.write(fbif::transcfg_offset(CTX), prev_transcfg);
        sec2.write(fbif::FBIF_CTL_OFFSET, prev_fbif_ctl);
        let restored = fbif::read_transcfg_raw(&sec2, CTX);
        crate::println!("    sec2 TRANSCFG[{}] restored to {:#010x}", CTX, restored);
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - imem-test needs real silicon");
    }
}

// nvidia acr-info - read-only structural dump of every SEC2 ACR blob
//
// For each blob we print:
//     raw size, container kind (NVFW vs raw)
//     if NVFW-wrapped: the parsed bin-hdr fields, then a 64-byte hex dump
//     starting at hdr.header_offset (the per-image HS header) and a 32-byte
//     hex dump starting at hdr.data_offset (the ucode payload prefix)
//     if raw: a 64-byte hex dump from offset 0
//
// This is purely observational - it touches no MMIO, no GPU state, runs
// fine without silicon. It exists so we have concrete bytes to compare
// against open-gpu-kernel-modules HS-header structs before we start
// kicking SEC2
fn cmd_nvidia_acr_info() {
    use crate::nvidia::gtx1650::tu116_fw::{self, NvfwBinHdr, Engine as FwEngine};
    use crate::nvidia::gtx1650::nvfw_hs::{NvfwHsHeader, NvfwHsLoadHeader};

    cprintln!(118, 185, 0, "  SEC2 ACR firmware structure");
    let mut shown = 0u32;
    for b in tu116_fw::TU116_FIRMWARE {
        if !matches!(b.engine, FwEngine::Sec2) { continue; }
        if !b.name.starts_with("acr/") { continue; }
        shown += 1;

        // Fetch from the firmware store; freed when fw drops at loop end.
        let fw = match crate::fwload::request(b.path) {
            Ok(fw) => fw,
            Err(e) => {
                crate::print_warn!("  {} unavailable ({:?})", b.name, e);
                continue;
            }
        };
        let bytes = fw.bytes();

        crate::println!("");
        cprintln!(180, 220, 255,
            "  {} ({} bytes)", b.name, bytes.len());

        // Outer NVFW container
        let bin_hdr = match NvfwBinHdr::parse(bytes) {
            Some(h) => h,
            None => {
                crate::print_warn!("    NVFW parse failed (magic/size mismatch)");
                continue;
            }
        };
        crate::println!("    nvfw_bin_hdr:");
        crate::println!("      magic={:#010x} ver={} bin_size={:#x}",
            bin_hdr.bin_magic, bin_hdr.bin_ver, bin_hdr.bin_size);
        crate::println!("      header_offset={:#x} data_offset={:#x} data_size={:#x}",
            bin_hdr.header_offset, bin_hdr.data_offset, bin_hdr.data_size);

        // Middle HS header. bl.bin / unload_bl.bin are NVFW-wrapped but
        // their inner 0x100..0x200 region is an LS bootloader descriptor,
        // not an HS header. looks_valid() filters those out
        let hs_hdr = match NvfwHsHeader::parse(bytes, bin_hdr.header_offset as usize) {
            Some(h) => h,
            None => {
                crate::print_warn!("    nvfw_hs_header parse failed at off {:#x}",
                    bin_hdr.header_offset);
                continue;
            }
        };

        if !hs_hdr.looks_valid(bin_hdr.header_offset, bin_hdr.data_offset) {
            crate::println!("    no HS header (LS bootloader descriptor)");
            let desc_start = bin_hdr.header_offset as usize;
            let desc_end   = bin_hdr.data_offset as usize;
            let desc_len   = desc_end.saturating_sub(desc_start).min(64);
            if desc_len > 0 && desc_end <= bytes.len() {
                crate::println!("    descriptor[0..{:#x}] (abs off {:#x}):",
                    desc_len, desc_start);
                dump_hex(&bytes[desc_start..desc_start + desc_len], 0);
            }
            let payload = bin_hdr.data(bytes);
            let pn = payload.len().min(32);
            crate::println!("    payload[0..{:#x}] (abs off {:#x}):",
                pn, bin_hdr.data_offset);
            dump_hex(&payload[..pn], 0);
            crate::println!("    payload total: {:#x} bytes", payload.len());
            continue;
        }

        crate::println!("    nvfw_hs_header:");
        crate::println!("      sig_dbg : off={:#x} size={:#x}",
            hs_hdr.sig_dbg_offset, hs_hdr.sig_dbg_size);
        crate::println!("      sig_prod: off={:#x} size={:#x}",
            hs_hdr.sig_prod_offset, hs_hdr.sig_prod_size);
        let pl_val = hs_hdr.read_patch_loc_value(bytes).unwrap_or(0xFFFF_FFFF);
        let ps_val = hs_hdr.read_patch_sig_value(bytes).unwrap_or(0xFFFF_FFFF);
        crate::println!("      patch_loc: ptr={:#x} *ptr={:#x}",
            hs_hdr.patch_loc, pl_val);
        crate::println!("      patch_sig: ptr={:#x} *ptr={:#x}",
            hs_hdr.patch_sig, ps_val);
        crate::println!("      hdr_offset={:#x} hdr_size={:#x}",
            hs_hdr.hdr_offset, hs_hdr.hdr_size);

        // Inner HS load header
        match NvfwHsLoadHeader::parse(bytes, hs_hdr.hdr_offset as usize, hs_hdr.hdr_size) {
            Some(lh) => {
                crate::println!("    nvfw_hs_load_header:");
                crate::println!("      non_sec_code : off={:#x} size={:#x}",
                    lh.non_sec_code_off, lh.non_sec_code_size);
                crate::println!("      data         : dma_base={:#x} size={:#x}",
                    lh.data_dma_base, lh.data_size);
                crate::println!("      num_apps={}", lh.num_apps);
                for (i, (co, cs)) in lh.apps_iter().enumerate() {
                    crate::println!("      app[{}]: code off={:#x} size={:#x}", i, co, cs);
                }
            }
            None => {
                crate::print_warn!(
                    "    nvfw_hs_load_header parse failed at off {:#x} size {:#x}",
                    hs_hdr.hdr_offset, hs_hdr.hdr_size);
            }
        }

        // Payload prefix
        let payload = bin_hdr.data(bytes);
        let pn = payload.len().min(32);
        crate::println!("    payload[0..{:#x}] (abs off {:#x}):",
            pn, bin_hdr.data_offset);
        dump_hex(&payload[..pn], 0);
        crate::println!("    payload total: {:#x} bytes", payload.len());
    }
    if shown == 0 {
        crate::print_warn!("  no acr/ blobs found in TU116_FIRMWARE bundle");
    }
}

// 16-byte-per-line hex dump used by acr-info. base is added to the row
// offset so the printed addresses match the source-blob's coordinate system
fn dump_hex(bytes: &[u8], base: usize) {
    for (row, chunk) in bytes.chunks(16).enumerate() {
        // Build a 47-char hex column: "AA BB CC ... " (3 chars per byte)
        let mut hex = [b' '; 48];
        for (i, b) in chunk.iter().enumerate() {
            let lo = b & 0xf;
            let hi = b >> 4;
            let hexc = |n: u8| -> u8 {
                if n < 10 { b'0' + n } else { b'a' + (n - 10) }
            };
            hex[i*3]     = hexc(hi);
            hex[i*3 + 1] = hexc(lo);
        }
        // SAFETY: hex contains only ASCII bytes by construction
        let hex_str = core::str::from_utf8(&hex[..47]).unwrap_or("?");
        crate::println!("      {:04x}: {}", base + row * 16, hex_str);
    }
}

// nvidia temp - read the on-die temperature sensor (PTHERM). Non-destructive
fn cmd_nvidia_rpc_test() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::rpc;

    let shown = with_gtx1650(|_dev| {
        cprintln!(118, 185, 0, "  GSP RPC frame round-trip self-test (no GPU traffic)");
        match rpc::self_test() {
            Ok(r) => {
                crate::println!("    encoded function={:#x} sequence={:#x} length={}",
                    r.function, r.seq, r.length);
                if r.frame_decoded_ok {
                    cprintln!(120, 220, 150, "    frame round-trip ok (header + payload)");
                } else {
                    crate::print_warn!("    frame round-trip failed");
                }
            }
            Err(e) => crate::print_warn!("    self-test failed: {:?}", e),
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound");
    }
}

fn cmd_nvidia_msgq_test() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::msgq;

    let shown = with_gtx1650(|_dev| {
        cprintln!(118, 185, 0, "  GSP msgq layout self-test (allocates 128 sysmem pages)");
        match msgq::self_test() {
            Ok(r) => {
                crate::println!("    region: base={:#x} size={} bytes entries={}",
                    r.phys_base, r.size, r.entries);
                crate::println!("    CMDQ.TxHdr @ {:#x}  CMDQ.RxHdr @ {:#x}",
                    r.cmdq_tx_phys, r.cmdq_rx_phys);
                crate::println!("    MSGQ.TxHdr @ {:#x}  MSGQ.RxHdr @ {:#x}",
                    r.msgq_tx_phys, r.msgq_rx_phys);
                crate::println!("    CMDQ data @ {:#x}   MSGQ data @ {:#x}",
                    r.cmdq_data_phys, r.msgq_data_phys);
                if r.ok {
                    cprintln!(120, 220, 150, "    layout ok (pointer round-trip + page alignment)");
                } else {
                    crate::print_warn!("    layout failed self-test");
                }
            }
            Err(e) => crate::print_warn!("    msgq alloc failed: {:?}", e),
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound");
    }
}

fn cmd_nvidia_wpr_state() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::regs::{
        decode_wpr_addr,
        PFB_PRI_MMU_ALLOW_READ, PFB_PRI_MMU_ALLOW_WRITE,
        PFB_PRI_MMU_WPR1_ADDR_HI, PFB_PRI_MMU_WPR1_ADDR_LO,
        PFB_PRI_MMU_WPR2_ADDR_HI, PFB_PRI_MMU_WPR2_ADDR_LO,
    };

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  PFB MMU WPR-region state (read-only)");
        let bar0 = &dev.bar0;

        let wpr1_lo_raw = bar0.read32(PFB_PRI_MMU_WPR1_ADDR_LO);
        let wpr1_hi_raw = bar0.read32(PFB_PRI_MMU_WPR1_ADDR_HI);
        let wpr2_lo_raw = bar0.read32(PFB_PRI_MMU_WPR2_ADDR_LO);
        let wpr2_hi_raw = bar0.read32(PFB_PRI_MMU_WPR2_ADDR_HI);
        let allow_r    = bar0.read32(PFB_PRI_MMU_ALLOW_READ);
        let allow_w    = bar0.read32(PFB_PRI_MMU_ALLOW_WRITE);

        let wpr1_lo = decode_wpr_addr(wpr1_lo_raw);
        let wpr1_hi = decode_wpr_addr(wpr1_hi_raw);
        let wpr2_lo = decode_wpr_addr(wpr2_lo_raw);
        let wpr2_hi = decode_wpr_addr(wpr2_hi_raw);

        crate::println!("    WPR1: lo={:#010x} hi={:#010x}", wpr1_lo_raw, wpr1_hi_raw);
        crate::println!("          [{:#x} .. {:#x}]", wpr1_lo, wpr1_hi);
        crate::println!("    WPR2: lo={:#010x} hi={:#010x}", wpr2_lo_raw, wpr2_hi_raw);
        crate::println!("          [{:#x} .. {:#x}]", wpr2_lo, wpr2_hi);
        crate::println!("    ALLOW_READ  = {:#010x}", allow_r);
        crate::println!("    ALLOW_WRITE = {:#010x}", allow_w);

        if wpr2_lo != 0 && wpr2_lo <= wpr2_hi {
            cprintln!(120, 220, 150,
                "    WPR2 LOCKED: {} MiB at top of VRAM",
                (wpr2_hi.saturating_sub(wpr2_lo)) >> 20);
        } else {
            cprintln!(235, 200, 90,
                "    WPR2 NOT locked - run 'nvidia sec2-acr-v2' to attempt ACR boot");
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - nothing to read");
    }
}

fn cmd_nvidia_temp() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::therm;

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  GPU thermal sensor (PTHERM, read-only)");
        let t = therm::read(&dev.bar0);
        crate::println!("    TEMP_SENSOR = {:#010x}  valid={}  shadowed={}",
            t.raw, t.valid, t.shadowed);
        if t.valid {
            let (r, g, b) = if t.celsius >= 90 { (235, 80, 80) }
                            else if t.celsius >= 75 { (235, 200, 90) }
                            else { (120, 220, 150) };
            cprintln!(r, g, b, "    temperature: {} C{}", t.celsius,
                if t.shadowed { " (stale latch)" } else { "" });
        } else {
            crate::print_warn!("    sensor reports no valid reading");
        }
        match t.slowdown_celsius() {
            Some(c) => crate::println!("    slowdown threshold: {} C ({:#010x})", c, t.slowdown_raw),
            None    => crate::println!("    slowdown threshold: not programmed (VBIOS devinit not run)"),
        }
        match t.shutdown_celsius() {
            Some(c) => crate::println!("    shutdown threshold: {} C ({:#010x})", c, t.shutdown_raw),
            None    => crate::println!("    shutdown threshold: not programmed (VBIOS devinit not run)"),
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - nothing to read");
    }
}

// nvidia gsp-rm - walk the GSP-RM bring-up scaffolding (VRAM size, WPR2
// layout, ABI sizes). Stops at MissingFirmware; no GSP-RM blob is shipped
fn cmd_nvidia_gsprm() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::gsprm::{self, GsprmError};

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  GSP-RM bring-up scaffolding");
        match gsprm::prepare(&dev.bar0) {
            Ok(prep) => {
                crate::println!("    VRAM: {} MiB total, {} MiB usable (ecc_reserved={})",
                    prep.vram.total_bytes >> 20, prep.vram.usable_bytes >> 20,
                    prep.vram.ecc_reserved);
                let l = prep.layout;
                crate::println!("    WPR2: [{:#x} .. {:#x}]", l.gsp_fw_wpr_start, l.gsp_fw_wpr_end);
                crate::println!("      heap   @ {:#x} + {:#x}", l.gsp_fw_heap_offset, l.gsp_fw_heap_size);
                crate::println!("      fw elf @ {:#x} + {:#x}", l.gsp_fw_offset, l.gsp_fw_size);
                crate::println!("      bootbin@ {:#x}", l.boot_bin_offset);
                crate::println!("      frts   @ {:#x} + {:#x}", l.frts_offset, l.frts_size);
                crate::println!("      non-wpr@ {:#x} + {:#x}", l.non_wpr_heap_offset, l.non_wpr_heap_size);
            }
            Err(GsprmError::NoVram) => {
                crate::print_warn!("    PFB reports 0 VRAM - devinit has not run, layout unavailable");
            }
            Err(GsprmError::MissingFirmware) => {
                crate::println!("    scaffolding OK (see serial log for VRAM / WPR2 layout)");
                crate::print_warn!("    next step needs a signed GSP-RM image (gsp-*.bin) - not in tree");
            }
            Err(e) => {
                crate::print_warn!("    gsp-rm prepare failed: {:?}", e);
            }
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - nothing to do");
    }
}

// nvidia gsp-rm-dryrun - self-test the radix3 + WPR-meta path with a
// synthetic in-memory ELF (default 256 pages = 1 MiB)
fn cmd_nvidia_gsprm_dryrun() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::gsprm;

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  GSP-RM scaffolding dry-run (synthetic ELF, 256 pages)");
        match gsprm::dryrun(&dev.bar0, 256) {
            Ok(r) => {
                crate::println!("    fake ELF: {} pages @ {:#x}", r.fake_elf_pages, r.fake_elf_phys);
                crate::println!("    radix3:   root @ {:#x}  (lvl1={} pages, lvl2={} pages)",
                    r.radix3_root_phys, r.lvl1_pages, r.lvl2_pages);
                crate::println!("    resolved page0 via lvl0->lvl1->lvl2 = {:#x}", r.resolved_first_page);
                crate::println!("    WprMeta size = {} bytes", r.meta_size);
                if r.ok {
                    cprintln!(120, 220, 150, "    PASS - radix3 chain resolves, meta block consistent");
                } else {
                    crate::print_warn!("    FAIL - chain mismatch (see serial log)");
                }
            }
            Err(e) => crate::print_warn!("    dry-run failed: {:?}", e),
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - nothing to do");
    }
}

// nvidia gsp-rm-load - parse the GSP-RM ELF (fetched from the firmware
// store on demand), stage .fwimage, build the radix3 + WPR-meta.
fn cmd_nvidia_gsprm_load() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::{gsprm, tu116_fw};
    use crate::nvidia::gtx1650::gsprm::LoadError;

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  GSP-RM load (TU116, line 570.144)");
        let gsp_rm_fw = tu116_fw::gsp_rm_570();
        let blob = gsp_rm_fw.bytes();
        if blob.is_empty() {
            crate::print_warn!("    GSP-RM firmware not available on the firmware store.");
            crate::println!("    Expected at /nvidia/tu116/gsp/gsp-570.144.bin on the store.");
            crate::println!("    The builder stages it from src/nvidia/gtx1650/tu116/gsp/gsp-570.144.bin");
            crate::println!("    (zstd -dc /usr/lib/firmware/nvidia/tu102/gsp/gsp-570.144.bin.zst > that path)");
            return;
        }
        match gsprm::load(&dev.bar0, blob) {
            Ok(r) => {
                let ver = core::str::from_utf8(&r.version)
                    .unwrap_or("?").trim_end_matches('\0');
                crate::println!("    fwimage   = {} bytes  signature = {} bytes  version = {}",
                    r.fwimage_len, r.signature_len, ver);
                crate::println!("    VRAM      = {} MiB", r.vram_total >> 20);
                crate::println!("    staged    @ {:#x}  ({} pages)", r.staged_phys, r.staged_pages);
                crate::println!("    radix3    root @ {:#x}  (lvl1={} pages, lvl2={} pages)  resolves={}",
                    r.radix3_root_phys, r.radix3_lvl1_pages, r.radix3_lvl2_pages, r.radix3_resolves);
                let l = r.layout;
                crate::println!("    WPR2      [{:#x} .. {:#x}]  fw@{:#x}+{:#x}  heap@{:#x}+{:#x}",
                    l.gsp_fw_wpr_start, l.gsp_fw_wpr_end, l.gsp_fw_offset, l.gsp_fw_size,
                    l.gsp_fw_heap_offset, l.gsp_fw_heap_size);
                crate::println!("    WprMeta   @ {:#x}  ({} bytes, pinned)", r.meta_phys, r.meta_size);
                if r.radix3_resolves {
                    cprintln!(120, 220, 150, "    OK - staged & pinned; run 'nvidia gsp-rm-boot' to hand it to the booter");
                } else {
                    crate::print_warn!("    radix3 chain does NOT resolve - see serial log");
                }
            }
            Err(LoadError::NoFirmware) => {
                crate::print_warn!("    GSP-RM firmware slice empty (feature off)");
            }
            Err(LoadError::Elf(e)) => {
                crate::print_warn!("    GSP-RM ELF parse failed: {:?}", e);
            }
            Err(LoadError::Gsprm(e)) => {
                crate::print_warn!("    GSP-RM staging failed: {:?}", e);
                if matches!(e, gsprm::GsprmError::Alloc(_)) {
                    crate::println!("    (could not get ~28 MiB of physically-contiguous sysmem)");
                }
            }
            Err(LoadError::BadBootloader) => {
                crate::print_warn!("    GSP RISC-V bootloader (bootloader-570.144.bin) did not parse");
            }
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - nothing to do");
    }
}

// nvidia gsp-rm-boot - hand the pinned WPR-meta to the GSP booter and kick
// booter_load. Requires 'nvidia gsp-rm-load' to have run first
fn cmd_nvidia_gsprm_boot() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::gsprm::{self, BooterError};

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  GSP-RM booter handoff");
        if !gsprm::is_loaded() {
            crate::print_warn!("    GSP-RM not loaded - run 'nvidia gsp-rm-load' first");
            return;
        }
        let meta_phys = gsprm::with_state(|s| s.meta_phys()).unwrap_or(0);
        crate::println!("    WPR-meta @ {:#x} -> GSP MAILBOX0/MAILBOX1, kicking booter_load...", meta_phys);
        match gsprm::boot_booter(&dev.bar0) {
            Ok(st) => {
                crate::println!("    booter halted: mb0={:#010x} mb1={:#010x} cpuctl={:#010x}",
                    st.mb0, st.mb1, st.cpuctl);
                if st.mb1 == 0 {
                    crate::println!("    mb1=0 - booter exited early (expected: WPR2 not locked by SEC2 ACR yet)");
                } else if st.mb1 & 0xFFFF_0000 == 0xBADF_0000 {
                    crate::print_warn!("    mb1 = WPR / image-header error class ({:#x})", st.mb1);
                } else {
                    crate::println!("    mb1 = opaque booter code {:#x}", st.mb1);
                }
            }
            Err(BooterError::NotLoaded) => crate::print_warn!("    GSP-RM not loaded"),
            Err(BooterError::Gsp(e))    => crate::print_warn!("    GSP booter aborted: {:?}", e),
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - nothing to do");
    }
}

// nvidia gsp-bootargs - build + verify the GSP-RM boot arguments in sysmem
//
// Allocates the libos init args, GSP_ARGUMENTS_CACHED and the CMDQ/MSGQ
// shared region exactly as the boot pipeline does, reads every field back
// out of sysmem, and checks it against the firmware ABI. No Falcon is
// started, so this is safe to run on a live card to confirm the byte layout
// is correct before attempting the full boot. All buffers are freed on exit
fn cmd_nvidia_gsp_bootargs() {
    use crate::nvidia::gtx1650::bootargs::GspBootArgs;

    cprintln!(118, 185, 0, "  GSP-RM boot-args layout self-test");
    match GspBootArgs::self_test() {
        Ok(r) => {
            crate::println!(
                "    shared @ {:#x} ({} pages, {} PTEs)  cmdq@{:#x} msgq@{:#x} depth={}",
                r.shared_phys, r.shared_pages, r.pte_count, r.cmdq_off, r.msgq_off, r.msg_count
            );
            crate::println!(
                "    libos @ {:#x}  rmargs @ {:#x}  loginit @ {:#x}",
                r.libos_phys, r.rmargs_phys, r.loginit_phys
            );
            let line = |name: &str, ok: bool| {
                if ok { cprintln!(100, 220, 150, "    [ok]   {}", name); }
                else  { crate::print_warn!("    [FAIL] {}", name); }
            };
            line("PTE array (identity-maps shared region)", r.ptes_ok);
            line("CMDQ tx header (size/msgSize/msgCount/flags/rxHdrOff/entryOff)", r.cmdq_ok);
            line("rmargs messageQueueInitArguments", r.rmargs_ok);
            line("libos init args (4 named regions, kind/loc)", r.libos_ok);
            line("log region embedded PTE arrays", r.log_ok);
            if r.ok {
                cprintln!(100, 220, 150, "  all boot-arg structures match the 570.144 ABI");
            } else {
                crate::print_warn!("  one or more structures FAILED - see serial log");
            }
        }
        Err(e) => crate::print_warn!("  boot-args alloc failed: {:?}", e),
    }
}

// nvidia gsp-rm-boot-full - drive the entire GSP-RM boot pipeline end to end
//
// Runs scrubber -> load -> SEC2 ACR -> WPR2 verify -> booter_load -> MSGQ
// handshake in order, stopping at the first stage whose hardware
// precondition is unmet and reporting exactly where and why
fn cmd_nvidia_gsprm_boot_full() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::gsprm::{self, BootStage};

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  GSP-RM full boot pipeline (6 stages)");
        let rep = gsprm::boot(&dev.bar0, &dev.pci);

        let stage_num = match rep.reached {
            BootStage::Scrubber     => 1,
            BootStage::Load         => 2,
            BootStage::Fwsec        => 3,
            BootStage::Acr          => 3,
            BootStage::Wpr2Locked   => 4,
            BootStage::Booter       => 5,
            BootStage::GspHandshake => 6,
        };
        crate::println!("    reached stage {}/6 ({:?})", stage_num, rep.reached);

        if rep.wpr2_locked {
            cprintln!(100, 220, 150,
                "    WPR2 locked: {:#x}..{:#x} ({} MiB)",
                rep.wpr2_lo, rep.wpr2_hi, (rep.wpr2_hi.saturating_sub(rep.wpr2_lo)) >> 20);
        } else {
            crate::print_warn!("    WPR2 not locked - FWSEC FRTS did not lock the region");
            crate::println!("    check the FWSEC desc parse + frts_err={:#06x} in the serial log", rep.frts_err);
        }

        match rep.reached {
            BootStage::GspHandshake => cprintln!(100, 220, 150,
                "    GSP-RM responded: function={:#x} result={:#x}",
                rep.gsp_msg_function, rep.gsp_msg_result),
            BootStage::Booter => {
                crate::println!("    booter ran (mb1={:#010x}); libos boot args handed to GSP, FALCON_OS set,",
                    rep.booter_mb1);
                crate::println!("    but GSP-RM has not posted to the MSGQ yet (see serial log for the poll result)");
            }
            _ => {}
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - GSP boot only runs against real silicon");
        crate::println!("  (on QEMU without VFIO this is expected)");
    }
}

// nvidia sec2-acr - first-contact SEC2 ACR boot
//
// Stages ucode_ahesasc into sysmem, points FBIF ctx0 at it, loads
// acr/bl into SEC2 IMEM (SECURE), hands the ahesasc phys address via
// MAILBOX0/MAILBOX1, kicks CPUCTL and reports the halt status.
// Without the full DMEM scratch layout the ACR bl is expected to
// halt early; the diagnostic value is observing the engine engaging
// our HS-signed upload
fn cmd_nvidia_sec2_acr() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::sec2;

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  SEC2 ACR first-contact boot");
        match sec2::attempt_acr(&dev.bar0) {
            Ok(st) => {
                crate::println!("    ahesasc staged: @ {:#x} ({} bytes)", st.ahesasc_phys, st.ahesasc_size);
                cprintln!(100, 220, 150,
                    "    halted: mb0={:#010x} mb1={:#010x} cpuctl={:#010x}",
                    st.mb0, st.mb1, st.cpuctl);
                if st.mb1 == 0 {
                    crate::println!("    mb1=0 - ACR exited early (expected: DMEM scratch layout not populated yet)");
                } else if st.mb1 & 0xFFFF_0000 == 0xBADF_0000 {
                    crate::print_warn!("    mb1 = PRI / sentinel error class ({:#x})", st.mb1);
                } else {
                    crate::println!("    mb1 = opaque ACR code {:#x}", st.mb1);
                }
            }
            Err(e) => crate::print_warn!("    aborted: {:?}", e),
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - nothing to do");
    }
}

// nvidia sec2-acr-v2 - SEC2 ACR boot with a real flcn_bl_dmem_desc
//
// Parses ahesasc HS layers (NVFW -> HS header -> HS load_header),
// builds a flcn_bl_dmem_desc pointing at the sysmem-staged image,
// uploads it to SEC2 DMEM, then kicks bl. A correct desc moves bl
// past the early-exit mb1=0 path into actually DMA-ing the HS image
fn cmd_nvidia_sec2_acr_v2() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::sec2;

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  SEC2 ACR v2 boot (with bl_dmem_desc)");
        match sec2::attempt_acr_v2(&dev.bar0) {
            Ok(st) => {
                crate::println!("    ahesasc: @ {:#x} ({} bytes)", st.ahesasc_phys, st.ahesasc_size);
                cprintln!(100, 220, 150,
                    "    halted: mb0={:#010x} mb1={:#010x} cpuctl={:#010x}",
                    st.mb0, st.mb1, st.cpuctl);
                if st.mb1 == 0 {
                    crate::println!("    mb1=0 - clean exit or pre-DMA halt");
                } else if st.mb1 & 0xFFFF_0000 == 0xBADF_0000 {
                    crate::print_warn!("    mb1 = PRI / sentinel error class ({:#x})", st.mb1);
                } else {
                    crate::println!("    mb1 = opaque ACR code {:#x}", st.mb1);
                }
            }
            Err(e) => crate::print_warn!("    aborted: {:?}", e),
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - nothing to do");
    }
}

// nvidia gsp - run the GSP first-contact boot from gsp::attempt_boot
fn cmd_nvidia_nvdec_scrub() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::nvdec;

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  NVDEC scrubber first-contact boot");
        match nvdec::attempt_scrub(&dev.bar0) {
            Ok(st) => {
                cprintln!(100, 220, 150,
                    "    halted: mb0={:#010x} mb1={:#010x} cpuctl={:#010x}",
                    st.mb0, st.mb1, st.cpuctl);
                if st.mb0 == 0 {
                    crate::println!("    interpretation: scrubber clean exit - region zeroed or descriptor empty");
                } else if st.mb0 & 0xFFFF_0000 == 0xBADF_0000 {
                    crate::println!("    interpretation: scrubber rejected the descriptor (PRI / descriptor class)");
                } else {
                    crate::println!("    interpretation: opaque code; cross-reference the nvdec scrubber ucode");
                }
            }
            Err(e) => crate::print_warn!("    aborted: {:?}", e),
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - nothing to scrub");
        crate::println!("  (on QEMU without VFIO this is expected; NVDEC scrub only runs against real silicon)");
    }
}

fn cmd_nvidia_gsp() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::gsp;

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  GSP first-contact boot");
        match gsp::attempt_boot(&dev.bar0) {
            Ok(st) => {
                cprintln!(100, 220, 150,
                    "    halted: mb0={:#010x} mb1={:#010x} cpuctl={:#010x}",
                    st.mb0, st.mb1, st.cpuctl);
                if st.mb1 == 0 {
                    crate::println!("    interpretation: booter took early-exit path - WPR or GSP-RM image absent");
                } else if st.mb1 & 0xFFFF_0000 == 0xBADF_0000 {
                    crate::println!("    interpretation: booter rejected the runtime state (WPR / image-header class)");
                } else {
                    crate::println!("    interpretation: opaque code; cross-reference open-gpu-kernel-modules booter source");
                }
            }
            Err(e) => crate::print_warn!("    aborted: {:?}", e),
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - nothing to boot");
        crate::println!("  (on QEMU without VFIO this is expected; GSP boot only runs against real silicon)");
    }
}

// nvidia next - inspect live state and prescribe the next concrete step
//
// This is the most useful subcommand when working on the driver: it walks
// the prerequisite chain (engine alive -> WPR set up -> GSP-RM staged ->
// booter runs) and stops at the first unmet precondition, telling you
// exactly what to implement next
fn cmd_nvidia_next() {
    use crate::nvidia::with_gtx1650;
    use crate::nvidia::gtx1650::falcon::{Engine, PSEC_BASE, PGSP_BASE, PFECS_BASE};

    cprintln!(118, 185, 0, "  TU116 driver bring-up checklist");

    let shown = with_gtx1650(|dev| {
        let sec2 = Engine::new(&dev.bar0, PSEC_BASE, "sec2");
        let gsp  = Engine::new(&dev.bar0, PGSP_BASE, "gsp");
        let fecs = Engine::new(&dev.bar0, PFECS_BASE, "fecs");

        let sec2_ok = sec2.is_alive();
        let gsp_ok  = gsp.is_alive();
        let fecs_ok = fecs.is_alive();

        crate::println!("    [{}] step 1: PCI bind + BAR0 mapped",         mark(true));
        crate::println!("    [{}] step 2: chip identified ({})",
            mark(true), dev.chip.codename());
        crate::println!("    [{}] step 3: firmware bundle embedded ({} blobs)",
            mark(true), crate::nvidia::gtx1650::tu116_fw::TU116_FIRMWARE.len());
        crate::println!("    [{}] step 4: SEC2 falcon alive (HWCFG nonzero)",  mark(sec2_ok));
        crate::println!("    [{}] step 5: GSP falcon alive (HWCFG nonzero)",   mark(gsp_ok));
        crate::println!("    [{}] step 6: FECS falcon alive (HWCFG nonzero)",  mark(fecs_ok));

        // Steps below are not auto-detectable (no infra in tree yet)    
        crate::println!("    [ ] step 7: DMA buffer allocator (phys-contiguous, GPU-visible)");
        crate::println!("    [ ] step 8: SEC2 ACR boot (consumes acr/bl + acr/ucode_ahesasc)");
        crate::println!("    [ ] step 9: WPR established + NVDEC scrubber pass");
        crate::println!("    [ ] step 10: GSP-RM image staged in WPR  (NOT shipped in tu116/)");
        crate::println!("    [ ] step 11: booter_load runs to completion + GSP-RM RPC up");
        crate::println!("    [ ] step 12: FECS/GPCCS contexts loaded; PGRAPH usable");

        crate::println!("");
        cprintln!(100, 220, 150, "  next concrete action:");
        if !sec2_ok || !gsp_ok {
            crate::println!("    SEC2 or GSP is gated. These live in NV_PMC_DEVICE_ENABLE_0 (BAR0+0x88c)");
            crate::println!("    on Turing, NOT in PMC_ENABLE; nvidia ungate does not touch them.");
            crate::println!("    On the GTX 1650 SEC2 + GSP normally come up alive at POST, so seeing");
            crate::println!("    them gated here points to a stale state - try nvidia ungate then");
            crate::println!("    a clean reboot. If still gated, model NV_PMC_DEVICE_ENABLE_0 explicitly.");
        } else if !fecs_ok {
            crate::println!("    FECS is still gated even though PMC_ENABLE.GR was set during init.");
            crate::println!("    Run nvidia ungate to re-apply, then nvidia falcon. If FECS still");
            crate::println!("    reports the PRI sentinel after that, the GPC is floor-swept (PFUSE)");
            crate::println!("    or there is a missing per-GR reset step we have not modelled yet.");
        } else {
            crate::println!("    implement a DMA buffer allocator: physically contiguous pages, mapped");
            crate::println!("    GPU-visible (BAR1 for VRAM staging, or sysmem PA for FBIF).");
            crate::println!("    Then port nouveau's nvkm_acr_hsfw_load to upload acr/ucode_ahesasc and");
            crate::println!("    boot SEC2 -> establishes WPR. After WPR, scrub region with NVDEC, then");
            crate::println!("    you still need to obtain a GSP-RM blob (gsp_t.bin) and stage it.");
        }

        crate::println!("");
        crate::println!("  references:");
        crate::println!("    nouveau:                drivers/gpu/drm/nouveau/nvkm/{{falcon,subdev/acr,engine/sec2}}");
        crate::println!("    open-gpu-kernel-modules: src/common/inc/swref/published/turing/tu102/");
        crate::println!("    envytools rnndb:        rnnutil/falcon.xml, hwref/turing/tu104/*");
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - driver did not register a card");
        crate::println!("  on QEMU this is normal: the host's Linux nvidia driver owns the device");
        crate::println!("  to test the driver path, either:");
        crate::println!("    1. boot MikuOS on bare metal where the GTX 1650 is not bound to anything else, or");
        crate::println!("    2. detach the GPU on the host (vfio-pci) and pass it through with -device vfio-pci.");
        crate::println!("  even without a bound card, nvidia firmware works and shows the embedded blob set");
    }
}

#[inline]
fn mark(ok: bool) -> char { if ok { 'x' } else { ' ' } }
