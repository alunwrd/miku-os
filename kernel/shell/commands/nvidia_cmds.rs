// NVIDIA GPU command handlers. The cmd_nvidia dispatcher is the sole public
// entry point

use crate::{cprint, cprintln};

pub fn cmd_nvidia(arg: &str) {
    // Splits the subcommand off its arguments: 'gb206-reset force' reaches the
    // handler as ("gb206-reset", "force"), and every other subcommand is a bare
    // word
    let arg = arg.trim();
    let (sub, args) = match arg.split_once(char::is_whitespace) {
        Some((s, r)) => (s, r.trim()),
        None => (arg, ""),
    };
    match sub {
        "" | "info" | "status"           => cmd_nvidia_info(),
        "list" | "ls" | "gpus"           => cmd_nvidia_list(),
        "splash" | "test" | "draw"       => {
            crate::io::framebuffer::splash::draw();
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
        "gsp"                            => cmd_nvidia_gsp(args),
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
        // Both names run the self-test for the region the firmware is handed
        "msgq-test" | "msgq"             => cmd_nvidia_gsp_bootargs(),
        "rpc-test" | "rpc"               => cmd_nvidia_gsp_bootargs(),
        "selftest" | "self-test"         => cmd_nvidia_selftest(),
        "chan-cycle" | "chancycle"       => cmd_nvidia_chan_cycle(args),
        "rm-tree" | "objects" | "rmtree" => cmd_nvidia_rm_tree(),
        "next" | "todo" | "roadmap"      => cmd_nvidia_next(),
        "gb206" | "blackwell" | "5060"   => cmd_nvidia_gb206(),
        "fsp" | "fsp-state"              => cmd_nvidia_fsp(),
        "cot-dryrun" | "cot"             => cmd_nvidia_cot_dryrun(),
        "gb206-reset" | "flr"            => cmd_nvidia_gb206_reset(args),
        "gb206-boot" | "cot-boot"        => cmd_nvidia_gb206_boot(args),
        "intr" | "interrupts"            => cmd_nvidia_intr(),
        "vram" | "fbmap"                 => cmd_nvidia_vram(),
        "va" | "vaspace"                 => cmd_nvidia_va(args),
        "perf" | "util"                  => cmd_nvidia_perf(args),
        "pcie" | "bus"                   => cmd_nvidia_pcie(),
        "disp" | "outputs"               => cmd_nvidia_disp(args),
        "gb206-log" | "gsplog"           => cmd_nvidia_gb206_log(args),
        "gb206-diff" | "rblock"          => cmd_nvidia_gb206_diff(),
        "log" | "logs"                   => cmd_nvidia_log(args),
        "bar1"                           => cmd_nvidia_bar1(args),
        "gb206-sym" | "sym"              => cmd_nvidia_gb206_sym(args),
        "screen"                         => cmd_nvidia_screen(args),
        // No "draw" alias here: `nvidia draw` is the boot splash, matched above
        "gb206-draw"                     => cmd_nvidia_gb206_draw(args),
        "compute-test"                   => cmd_nvidia_compute_test(),
        "blur"                           => cmd_nvidia_blur(args),
        "backdrop"                       => cmd_nvidia_backdrop(),
        "pramin-test" | "pramin"         => cmd_nvidia_pramin_test(),
        "gb206-scratch" | "scratch"      => cmd_nvidia_gb206_scratch(args),
        "help" | "?"                     => cmd_nvidia_help_local(),
        // No second list of subcommands here: a list nobody updates answers the
        // question wrongly
        _ => {
            cprintln!(255, 200, 80, "  no such subcommand: {}", sub);
            crate::println!("  'nvidia help' lists every one of them");
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
    crate::println!("    gsp unload|load|reset - GB20x: take GSP-RM down / boot it again / FLR between the two");
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
    crate::println!("    msgq-test, rpc-test - aliases for gsp-bootargs (the layout that is really used)");
    crate::println!("    next                - inspect state and prescribe the next concrete step");
    crate::println!("    gb206 (blackwell)   - RTX 5060 / 5060 Ti status: chip, FSP, firmware staging");
    crate::println!("    fsp                 - FSP ring pointers + secure-boot scratch (read-only)");
    crate::println!("    cot-dryrun (cot)    - build + verify the NVDM_PAYLOAD_COT without sending it");
    crate::println!("    gb206-reset (flr)   - PCIe Function Level Reset: re-run on-die devinit so");
    crate::println!("                           GFW boot progress reaches 0xff before a GSP-RM boot.");
    crate::println!("                           Refused when GFW is already 0xff, or when this GPU");
    crate::println!("                           drives the console (FLR kills scanout for good -");
    crate::println!("                           power-cycle instead); 'gb206-reset force' overrides");
    crate::println!("    gb206-boot [leds]   - full Blackwell GSP-RM boot: stage FMC/GSP-RM/bootloader,");
    crate::println!("                           send COT to the FSP, wait lockdown release, link GSP-RM's");
    crate::println!("                           queue and wait for its answer to GSP_INIT");
    crate::println!("                           'noscreen' parks the framebuffer console before the COT");
    crate::println!("                           (BAR1 gets remapped by GSP-RM's display init); output");
    crate::println!("                           continues on serial, screen stays black until reboot");
    crate::println!("                           GSP-RM's own log is decoded live during the handshake");
    crate::println!("                           (needs the logging ELF); 'log' shows every line, not");
    crate::println!("                           just warnings and errors");
    crate::println!("                           'noprimary' sends bIsPrimary=0 so GSP-RM does not try");
    crate::println!("                           to inherit the firmware console (display-init probe)");
    crate::println!("                           'dispvid' forces display instmem + state cache into");
    crate::println!("                           vidmem (RMInstLoc/RMInstLoc4) - probes the 0x27");
    crate::println!("                           INVALID_DMA_SPECIFIER the display subsystem reports");
    crate::println!("                           every run is mirrored into the persistent RAM log:");
    crate::println!("                           reboot, then 'pstore' shows what it printed after the");
    crate::println!("                           screen died");
    crate::println!("    chan-cycle N        - build and tear the copy engine's channel down N times,");
    crate::println!("                          printing what the driver still holds each round");
    crate::println!("    rm-tree (objects)   - every object RM is holding for this driver, as a tree:");
    crate::println!("                          what it is and whose child it is. A leak is a line here,");
    crate::println!("                          followed by the graphics context as RM describes it");
    crate::println!("    selftest            - what can be decided without the card: page-table");
    crate::println!("                          arithmetic, range checks, binding overlap");
    crate::println!("    vram (fbmap)        - the video-memory map: regions GSP-RM reported, what is");
    crate::println!("                           handed out and to whom, and what is left. Read-only");
    crate::println!("    disp (outputs)      - every display id the board reports: its connector");
    crate::println!("                           name, OR/SOR and protocol, whether a monitor is");
    crate::println!("                           plugged in, DPCD, HDMI caps, head, surface and");
    crate::println!("                           cursor. Read-only. Modes, sleep, refresh and colour");
    crate::println!("                           are 'display' (mode, on|off, vrr, color)");
    crate::println!("    disp underflow <id> - ask the raster generator whether it ran dry");
    crate::println!("                           since the last time it was asked. The fault that");
    crate::println!("                           shows up as horizontal smears rather than as a");
    crate::println!("                           frame that fails");
    crate::println!("    disp imp <id>       - RM's display-engine budget for each of the output's");
    crate::println!("                           modes, beside the link's answer");
    crate::println!("    disp poll           - drain GSP-RM's event queue now and act on any");
    crate::println!("                           hot-plug in it. A background thread does this");
    crate::println!("                           every 200 ms; this is for watching it happen");
    crate::println!("    intr (interrupts)   - the interrupt vector table GSP-RM assigned: which leaf");
    crate::println!("                           and bit of the VFN tree each engine lands in. Read-only");
    crate::println!("    gb206-log (gsplog)  - decode GSP-RM's libos log (file/line/text) if the");
    crate::println!("                           logging ELF is installed; 'all' uncaps, 'raw' forces ASCII");
    crate::println!("    gb206-sym <hex>     - resolve a GSP-RM image offset to a function name");
    crate::println!("    log [section] [err|debug] [-n N] [-f] [-g text] - the driver's log from memory,");
    crate::println!("                           no serial or pstore; sections boot gsp fw rpc rm disp");
    crate::println!("                           mem chan intr perf; 'err' = errors and warnings, 'debug' adds");
    crate::println!("                           every RPC and RM call; -f follows; 'log help' for all");
    crate::println!("    perf [clocks|lock|floor|boost] - P-state, clocks, load, power, temperatures as");
    crate::println!("                           GSP-RM writes them; 'clocks' the last seconds of them;");
    crate::println!("                           'lock tdp|turbo|off', 'floor tdp|turbo|off' fix them;");
    crate::println!("                           'boost max|1|off [seconds|forever]' asks for more");
    crate::println!("    pcie (bus)          - the GPU and its root port as 'lspci -vv' shows them, and");
    crate::println!("                           what GSP-RM was told about the chipset");
    crate::println!("    va [all]            - the GPU address spaces: reservations, owners, what is");
    crate::println!("                           mapped in each; 'all' lists every mapping");
    crate::println!("    bar1 probe|on|off   - BAR1 in physical mode: probe checks it against PRAMIN");
    crate::println!("                           and puts it back; on keeps it (the console writes");
    crate::println!("                           through it); off binds GSP-RM's binding back");
    crate::println!("    gb206-diff (rblock) - the lit head's R block: frame rate, SOR, armed state,");
    crate::println!("                           exceptions, and the diff against the shipping driver");
    crate::println!("    screen on|off       - re-attach / park the framebuffer console by hand");
    crate::println!("    backdrop            - thumbnail of what the effects pass reads, sampled");
    crate::println!("                           back through PRAMIN; wallpaper with a window-shaped hole");
    crate::println!("    gb206-draw [what]   - draw into the live scanout surface: test | fill RRGGBB |");
    crate::println!("                           clear | text <words>. Needs gb206-boot to have armed a window");
    crate::println!("    pramin-test         - write text into VRAM through BAR0, bypassing BAR1;");
    crate::println!("                           run before and after gb206-boot to prove it survives");
    crate::println!("    gb206-scratch [full]- one-screen boot state: GFW/PLM, FSP scratch + ucode ver,");
    crate::println!("                           GSP-FMC error code, falcon/RISC-V state (read-only).");
    crate::println!("                           'full' adds the raw 4 KiB NV_PGC6 hex dump");
    crate::println!("    splash              - redraw the boot splash via the framebuffer");
}

// GB206 (RTX 5060 / 5060 Ti, Blackwell) commands

/// Run the first program on the GPU and say whether it did what it was asked
fn cmd_nvidia_compute_test() {
    let shown = crate::drivers::gpu::nvidia::with_gb20x(|d| {
        crate::drivers::gpu::nvidia::blackwell::boot::compute_test(&d.bar0, &|line| {
            crate::println!("  {}", line);
        });
    });
    if shown.is_none() {
        cprintln!(255, 80, 80, "  no GB206 device");
    }
}

/// Blurs what is on the screen, once, in place
///
/// The whole surface and no blending, to prove the pass: if the screen goes
/// soft and stays readable, the kernels, the video-memory mappings and the
/// six-deep chain worked
fn cmd_nvidia_blur(args: &str) {
    use crate::drivers::gpu::nvidia::{blur, scanout};

    let radius: f32 = args.split_whitespace().next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(2) as f32;

    let Some(surface) = scanout::console_surface() else {
        cprintln!(255, 80, 80, "  nothing is being drawn into");
        return;
    };
    let say = &|line: &str| crate::println!("  {}", line);

    let shown = crate::drivers::gpu::nvidia::with_gb20x(|d| {
        if !blur::ready() && !crate::drivers::gpu::nvidia::blackwell::boot::blur_init(
            &d.bar0, surface.width, surface.height, surface.pitch, say,
        ) {
            return;
        }
        crate::drivers::gpu::nvidia::blackwell::boot::blur_surface(
            &d.bar0, surface.vram, surface.pitch, surface.width, surface.height, radius, say,
        );
    });
    if shown.is_none() {
        cprintln!(255, 80, 80, "  no GB206 device");
    }
}

/// What the effects pass reads, as a picture made of characters
///
/// The backdrop is video memory nothing scans out, so it has to be read back.
/// Copying it to the screen would last one frame: typing the command damages
/// the console and triggers that frame. A thumbnail survives and costs one
/// PRAMIN read per character, and shows whether the wallpaper is there with a
/// window-shaped hole where the terminal is not drawn into it
fn cmd_nvidia_backdrop() {
    use crate::drivers::gpu::nvidia::{blur, pramin, scanout};
    extern crate alloc;
    use alloc::string::String;

    let Some(dst) = blur::backdrop_surface() else {
        cprintln!(255, 200, 80,
            "  no backdrop buffer, `nobackdrop` on the boot line, or no video memory");
        return;
    };

    let shown = crate::drivers::gpu::nvidia::with_gb20x(|d| {
        cprintln!(118, 185, 0, "  effects backdrop");
        cprintln!(230, 240, 240, "    {}x{} at vram {:#x}, pitch {} B",
            dst.width, dst.height, dst.vram, dst.pitch);
        cprintln!(230, 240, 240, "    rectangles it would not take: {}",
            scanout::backdrop_refused());

        // Luminance to ink, darkest first. Rec. 601 weights in integers: there
        // is no libm here
        const INK: &[u8] = b" .:-=+*#%@";
        let (cols, rows) = (76usize, 20usize);
        let mut win = pramin::Window::new(&d.bar0);
        for r in 0..rows {
            let y = (r * dst.height as usize / rows) as u64;
            let mut line = String::with_capacity(cols);
            for c in 0..cols {
                let x = (c * dst.width as usize / cols) as u64;
                let px = win.read32(dst.vram + y * dst.pitch as u64 + x * 4);
                let (rr, gg, bb) = ((px >> 16) & 0xff, (px >> 8) & 0xff, px & 0xff);
                let lum = ((rr * 77 + gg * 151 + bb * 28) >> 8) as usize;
                line.push(INK[(lum.min(255) * (INK.len() - 1)) / 255] as char);
            }
            cprintln!(180, 200, 210, "    {}", line);
        }
    });
    if shown.is_none() {
        cprintln!(255, 80, 80, "  no GB206 device");
    }
}

fn cmd_nvidia_gb206() {
    let shown = crate::drivers::gpu::nvidia::with_gb20x(|d| {
        cprintln!(118, 185, 0, "  GB206 (Blackwell) device");
        cprintln!(230, 240, 240, "    model:     {}", crate::drivers::gpu::nvidia::blackwell::model_name(d.pci.device));
        cprintln!(230, 240, 240, "    pci:       {:04x}:{:04x} at {:02x}:{:02x}.{:x}",
            d.pci.vendor, d.pci.device, d.pci.bus, d.pci.dev, d.pci.func);
        cprintln!(230, 240, 240, "    chip:      {} (PMC_BOOT_0={:#010x} BOOT_42={:#010x})",
            d.chip.codename(), d.chip.raw, d.boot42);
        cprintln!(230, 240, 240, "    bar0:      {:#x} ({} MB)", d.bar0.virt_base(), d.bar0.size() / (1024 * 1024));
        // How the host booted, and how far the card's own GFW got, for
        // information only. BIOS through GRUB is the supported path (under UEFI
        // GRUB's multiboot2 faults), and GSP-RM boots on a card whose GFW
        // progress reads 0: it is not a gate
        if crate::arch::x86_64::grub::booted_via_uefi() {
            cprintln!(230, 240, 240, "    host boot: UEFI");
        } else {
            cprintln!(230, 240, 240, "    host boot: BIOS/CSM");
        }
        {
            use crate::drivers::gpu::nvidia::blackwell::regs;
            let gfw = d.bar0.read32(regs::PGC6_AON_SECURE_SCRATCH_GROUP_05_0);
            if gfw & 0xFF == regs::GFW_BOOT_PROGRESS_COMPLETED {
                cprintln!(230, 240, 240, "    gfw boot:  completed (progress 0xff)");
            } else {
                cprintln!(230, 240, 240, "    gfw boot:  progress {:#04x} (raw {:#010x}; not needed by GSP-RM)", gfw & 0xFF, gfw);
            }
        }
        let fsp_ok = d.fsp_boot_status == crate::drivers::gpu::nvidia::blackwell::regs::FSP_BOOT_COMPLETE_SUCCESS;
        if fsp_ok {
            cprintln!(100, 220, 150, "    fsp:       secure boot complete (scratch 0xff)");
        } else {
            cprintln!(255, 80, 80, "    fsp:       scratch {:#04x} (expected 0xff)", d.fsp_boot_status);
        }
        match d.fw.fmc_size {
            Some(sz) => cprintln!(100, 220, 150, "    fmc blob:  ok ({} KB)", sz / 1024),
            None => cprintln!(255, 80, 80, "    fmc blob:  missing/invalid: /lib/firmware/{}", d.gb.fmc),
        }
        match d.fw.gsp_size {
            Some(sz) => cprintln!(100, 220, 150, "    gsp-rm:    ok ({} MB)", sz / (1024 * 1024)),
            None => cprintln!(255, 80, 80, "    gsp-rm:    missing: /lib/firmware/{}", d.gb.gsp),
        }
        match d.fw.bl_size {
            Some(sz) => cprintln!(100, 220, 150, "    bootldr:   ok ({} KB)", sz / 1024),
            None => cprintln!(255, 80, 80, "    bootldr:   missing: /lib/firmware/{}", d.gb.bootloader),
        }
        match d.fw.ucodes_size {
            Some(sz) => cprintln!(100, 220, 150, "    ucodes:    ok ({} MB)", sz / (1024 * 1024)),
            None => cprintln!(255, 80, 80, "    ucodes:    missing: /lib/firmware/{}", d.gb.ucodes),
        }
        // What was compiled for this card at build time: a program not built is
        // worth knowing about before the channel is blamed
        if crate::drivers::gpu::nvidia::blackwell::boot::compute_ready() {
            cprintln!(100, 220, 150, "    compute:   channel up, context promoted");
        } else {
            cprintln!(200, 200, 100, "    compute:   not up (see the gb206-boot log)");
        }
        {
            use crate::drivers::gpu::nvidia::shaders;
            let line = shaders::describe();
            if shaders::AVAILABLE {
                cprintln!(100, 220, 150, "    programs:  {}", line);
            } else {
                cprintln!(200, 200, 100, "    programs:  {}", line);
            }
        }
        if crate::drivers::gpu::nvidia::blackwell::boot::is_booted() {
            cprintln!(100, 220, 150, "    gsp boot:  COT sent, state pinned (nvidia gb206-boot ran)");
        }
    });
    if shown.is_none() {
        cprintln!(120, 140, 140, "  no GB206 card registered (nvidia list to see what was found)");
    }
}

fn cmd_nvidia_fsp() {
    let shown = crate::drivers::gpu::nvidia::with_gb20x(|d| {
        use crate::drivers::gpu::nvidia::blackwell::{fsp::Fsp, regs};
        let fsp = Fsp::new(&d.bar0);
        cprintln!(118, 185, 0, "  FSP state (read-only)");
        cprintln!(230, 240, 240, "    boot scratch: {:#04x} (0xff = secure boot complete)", fsp.boot_status());
        cprintln!(230, 240, 240, "    cmdq  head/tail: {:#x} / {:#x}",
            d.bar0.read32(regs::PFSP_QUEUE_HEAD0), d.bar0.read32(regs::PFSP_QUEUE_TAIL0));
        cprintln!(230, 240, 240, "    msgq  head/tail: {:#x} / {:#x}",
            d.bar0.read32(regs::PFSP_MSGQ_HEAD0), d.bar0.read32(regs::PFSP_MSGQ_TAIL0));
        fsp.log_state("shell");
    });
    if shown.is_none() {
        cprintln!(120, 140, 140, "  no GB206 card registered");
    }
}

fn cmd_nvidia_gb206_reset(args: &str) {
    use crate::drivers::gpu::nvidia::blackwell::regs;
    use crate::drivers::gpu::nvidia::{fb, reset};

    let force = args.split_whitespace().any(|t| t == "force" || t == "-f");

    let shown = crate::drivers::gpu::nvidia::with_gb20x(|d| {
        cprintln!(118, 185, 0, "  GB206 Function Level Reset (re-run on-die devinit)");

        // A booted GSP-RM is taken down by its own unload first: engines,
        // client, the suspend and the halt, and everything that named it.
        // Resetting under it would leave all of that pinned to a GPU with none
        // of it (`dev/gsp-foundation.md`, section 14)
        if crate::drivers::gpu::nvidia::blackwell::boot::is_booted() {
            cprintln!(255, 200, 80, "    GSP-RM is loaded, 'nvidia gsp unload' first");
            return;
        }

        let gfw_before = d.bar0.read32(regs::PGC6_AON_SECURE_SCRATCH_GROUP_05_0) & 0xFF;
        cprintln!(230, 240, 240, "    GFW boot progress before: {:#04x}", gfw_before);

        // An FLR is a recovery tool, not a boot step. On a card whose GFW reads
        // 0xff there is nothing to re-run, and the reset costs the display
        // (below), so it is refused
        if gfw_before == regs::GFW_BOOT_PROGRESS_COMPLETED && !force {
            cprintln!(100, 220, 150,
                "    devinit already COMPLETED, no reset needed, run: nvidia gb206-boot");
            cprintln!(120, 140, 140,
                "    ('nvidia gb206-reset force' resets anyway)");
            return;
        }

        // The boot framebuffer lives in this card's BAR1 whenever the monitor
        // hangs off it. An FLR resets the display engine, and with no VBIOS
        // re-POST, scanout stops and the monitor does not wake. A cold power
        // cycle clears the PGC6 AON scratch as well and lets the firmware POST
        // the card, so it is the recommended move
        let fb_here = crate::arch::x86_64::grub::framebuffer()
            .and_then(|i| fb::find_in_bars(&d.pci, i.addr));
        if let Some(loc) = fb_here {
            cprintln!(255, 200, 80,
                "    this GPU drives the console (boot fb in BAR{} +{:#x})",
                loc.bar_index, loc.offset);
            if !force {
                cprintln!(255, 80, 80, "    refusing: FLR would kill scanout permanently");
                cprintln!(120, 140, 140,
                    "    the monitor does not come back, MikuOS cannot re-POST the VBIOS,");
                cprintln!(120, 140, 140,
                    "    and only the boot firmware ever ran a modeset on this card");
                cprintln!(120, 140, 140,
                    "    instead: full power cycle (not warm reboot) also clears the PGC6");
                cprintln!(120, 140, 140,
                    "    AON scratch, and POST leaves GFW at 0xff with the display alive");
                cprintln!(120, 140, 140,
                    "    override with: nvidia gb206-reset force");
                return;
            }
            cprintln!(255, 200, 80,
                "    'force' given - the screen dies here; keyboard LEDs report the result:");
            cprintln!(120, 140, 140, "      3 blinks then solid ON  = FLR ok, GFW back at 0xff");
            cprintln!(120, 140, 140, "      3 blinks then OFF       = FLR ran, GFW still not 0xff");
            cprintln!(120, 140, 140, "      no blinks at all        = kernel hung, power-cycle");
            crate::drivers::input::ps2::blink_leds(1, 0x00);
        }

        // Nothing of ours is on the display engine by now: the unload above
        // took the console, the vblank, the scanout and the output table down
        // (the display engine's `unload`)
        match reset::function_level_reset(&d.pci) {
            Ok(()) => {
                cprintln!(100, 220, 150, "    FLR complete, device back online");
                // Out of reset, the FSP holds a firewall over the BARs until it
                // has booted again; nothing may be asked of the card before it
                // drops (`kbifPollBarFirewallDisengage_GB202`)
                match crate::drivers::gpu::nvidia::blackwell::boot::poll_bar_firewall(&d.bar0) {
                    Ok(ms) => cprintln!(100, 220, 150, "    BAR firewall disengaged after {} ms", ms),
                    Err(v) => cprintln!(255, 200, 80,
                        "    BAR firewall still engaged after 500 ms (VSEC_DEBUG_SEC_2 {:#010x})", v),
                }
                let gfw_after = d.bar0.read32(regs::PGC6_AON_SECURE_SCRATCH_GROUP_05_0) & 0xFF;
                // Console is gone by now if the fb was on this card, so the
                // verdict goes out over the LEDs as well as the (dead) screen
                if fb_here.is_some() {
                    let ok = gfw_after == regs::GFW_BOOT_PROGRESS_COMPLETED;
                    crate::drivers::input::ps2::blink_leds(3, if ok { 0x07 } else { 0x00 });
                }
                if gfw_after == regs::GFW_BOOT_PROGRESS_COMPLETED {
                    cprintln!(100, 220, 150,
                        "    GFW boot progress after:  {:#04x} (COMPLETED) - now run: nvidia gb206-boot",
                        gfw_after);
                } else {
                    cprintln!(255, 200, 80,
                        "    GFW boot progress after:  {:#04x} (still not 0xff)", gfw_after);
                    cprintln!(120, 140, 140,
                        "    devinit may take longer; re-check with 'nvidia gb206' in a moment");
                }
            }
            Err(e) => {
                if fb_here.is_some() {
                    crate::drivers::input::ps2::blink_leds(3, 0x00);
                }
                cprintln!(255, 80, 80, "    FLR failed: {:?}", e);
                match e {
                    reset::ResetError::NoPcieCap => cprintln!(120, 140, 140,
                        "    no PCI Express capability found, unexpected for a modern GPU"),
                    reset::ResetError::NotFlrCapable => cprintln!(120, 140, 140,
                        "    device does not advertise FLR; a secondary-bus reset on the bridge is needed"),
                    reset::ResetError::DeviceGone => cprintln!(120, 140, 140,
                        "    device did not return after reset; a full power cycle is required"),
                }
            }
        }
    });
    if shown.is_none() {
        cprintln!(255, 80, 80, "  no RTX 5060 / 5060 Ti (GB206) found");
    }
}

/// 'args' may carry "leds" to enable the keyboard-LED status channel.
///
/// Off by default because driving it costs the keyboard: ps2::set_leds writes
/// 0xED plus a data byte and collects the ACKs with a bare port read while IRQ
/// 1 is live and the shell's handler reads the same port. The command state
/// machine desyncs and input stops for the session, permanently under USB
/// legacy emulation. For when the console is expected to die
fn cmd_nvidia_gb206_boot(args: &str) {
    use crate::drivers::gpu::nvidia::blackwell::boot;

    let leds = args.split_whitespace().any(|a| a == "leds");
    // Parks the framebuffer console before the COT and leaves it parked: the
    // boot fb is in this GPU's BAR1 and GSP-RM remaps that aperture during
    // display init, so every print after the COT is a store into a moving
    // target. Output continues on serial; the screen stays black until a reboot
    let noscreen = args
        .split_whitespace()
        .any(|a| a == "noscreen" || a == "serial" || a == "quiet");
    // GSP-RM's own log is decoded live during the handshake (see boot_opts). By
    // default only warnings and errors reach the screen; "log" shows every line
    let log_verbose = args
        .split_whitespace()
        .any(|a| a == "log" || a == "verbose" || a == "-v");
    // Tells GSP-RM this is not the primary display device. See BootFlags: this
    // boolean gates the display-takeover path
    let not_primary = args
        .split_whitespace()
        .any(|a| a == "noprimary" || a == "notprimary");
    // Forces the display's instance memory into vidmem, for
    // NV_ERR_INVALID_DMA_SPECIFIER in the ENGINE record
    let disp_vidmem = args
        .split_whitespace()
        .any(|a| a == "dispvid" || a == "dispvidmem");

    if boot::is_booted() {
        cprintln!(255, 200, 80, "  GSP-RM is loaded, 'nvidia gsp unload' first");
        return;
    }
    let ran = crate::drivers::gpu::nvidia::with_gb20x(|d| {
        use crate::drivers::gpu::nvidia::blackwell::regs;
        cprintln!(118, 185, 0, "  GB206 GSP-RM boot (COT via FSP)");
        // GFW progress before and after: the FMC re-runs devinit as part of the
        // COT boot; if it dies partway the AON scratch drops to a low value and
        // stays there across warm reboots
        let gfw_pre = d.bar0.read32(regs::PGC6_AON_SECURE_SCRATCH_GROUP_05_0) & 0xFF;
        cprintln!(230, 240, 240, "    gfw progress before boot: {:#04x}", gfw_pre);
        // Out-of-band status channel: GSP-RM display init may kill GOP scanout,
        // freezing the console mid-boot, and keyboard LEDs survive that. One
        // blink proves the channel works on this keyboard
        if leds {
            cprintln!(120, 140, 140, "    keyboard LEDs = status channel if the screen freezes:");
            // Needs a PS/2 keyboard: set_leds drives the i8042, so with a USB
            // keyboard (native xHCI HID path) nothing lights up and no blinks
            // means only that this channel is absent
            cprintln!(120, 140, 140, "      (PS/2 keyboards only - a USB keyboard will not light up)");
            cprintln!(120, 140, 140, "      scroll-lock toggling     = loop alive (every ~5 s)");
            cprintln!(120, 140, 140, "      3 blinks then solid ON  = GSP_INIT answered");
            cprintln!(120, 140, 140, "      3 blinks then OFF       = boot ended without an answer");
            cprintln!(120, 140, 140, "      no blinks at all        = kernel hung, power-cycle");
            crate::drivers::input::ps2::blink_leds(1, 0x00);
        }
        // Under the display layer's lock, as the unload is
        let result = crate::drivers::gpu::kms::exclusive(|| boot::boot_opts(&d.bar0, &d.pci, d.gb, boot::BootFlags {
            detach_console: noscreen,
            log_verbose,
            led_heartbeat: leds,
            not_primary,
            disp_vidmem,
        }));
        // Reaching this line proves the CPU survived the whole boot.
        //
        // The console does not come back here: a parked boot leaves the machine
        // alive (a blind 'reboot' works), then it wedges later. The only thing
        // touching BAR1 after the boot is the console returning to it, and
        // GSP-RM has remapped that aperture. Everything below goes to serial
        // only; 'nvidia screen on' re-attaches by hand
        if leds {
            match &result {
                Ok(r) if r.init_done => crate::drivers::input::ps2::blink_leds(3, 0x07),
                _ => crate::drivers::input::ps2::blink_leds(3, 0x00),
            }
        }
        let gfw_post = d.bar0.read32(regs::PGC6_AON_SECURE_SCRATCH_GROUP_05_0) & 0xFF;
        cprintln!(230, 240, 240, "    gfw progress after boot:  {:#04x}", gfw_post);
        match result {
            Ok(r) => {
                cprintln!(230, 240, 240, "    fb size:      {} MiB", r.fb_size >> 20);
                cprintln!(230, 240, 240, "    wpr heap:     {} MiB, frts offset-from-end {:#x}",
                    r.heap_size >> 20, r.frts_offset);
                cprintln!(230, 240, 240, "    fmc image:    @ {:#x}", r.fmc_image_phys);
                cprintln!(230, 240, 240, "    radix3 root:  @ {:#x}", r.radix3_root_phys);
                cprintln!(230, 240, 240, "    wpr meta:     @ {:#x}", r.wpr_meta_phys);
                cprintln!(230, 240, 240, "    boot params:  @ {:#x}, libos @ {:#x}",
                    r.boot_params_phys, r.libos_phys);
                cprintln!(100, 220, 150, "    FSP accepted the COT ({} response words)",
                    r.cot_response_words);
                if r.lockdown_released {
                    cprintln!(100, 220, 150, "    RISC-V lockdown released (mbox0={:#x})", r.mbox0);
                }
                if let Some(xid) = r.xid {
                    cprintln!(255, 80, 80,
                        "    GSP-RM reported Xid {} - see the line above / serial for its text", xid);
                }
                if r.init_done {
                    cprintln!(100, 220, 150,
                        "    GSP_INIT answered - GSP-RM is up, static info read");
                } else if !r.msgq_initialized {
                    cprintln!(255, 200, 80,
                        "    GSP-RM never wrote its queue header - check serial log for GSP-FMC's error");
                } else {
                    match r.gsp_first_msg {
                        Some((f, _)) => cprintln!(255, 200, 80,
                            "    events seen (first {:#x}) but GSP_INIT was not answered", f),
                        None => cprintln!(255, 200, 80,
                            "    queue linked, but nothing came back to GSP_INIT - check serial log"),
                    }
                }
                // Says what is on the monitor and where the shell went
                match boot::live_scanout() {
                    Some(l) => {
                        cprintln!(100, 220, 150,
                            "    picture is live: {}x{} at vram {:#x}, head {} window {}",
                            l.width, l.height, l.vram, l.head, l.window);
                    }
                    None => cprintln!(255, 200, 80,
                        "    no scanout armed - the monitor is showing nothing of ours"),
                }
                // And whether the pointer is the head's job or the
                // compositor's, which decides what a mouse move costs
                match boot::live_cursor() {
                    Some(c) => cprintln!(100, 220, 150,
                        "    cursor plane armed: {}x{} at vram {:#x}, head {}",
                        c.size, c.size, c.vram, c.head),
                    None => cprintln!(120, 140, 140,
                        "    no cursor plane - the compositor draws the pointer itself"),
                }
            }
            Err(e) => {
                cprintln!(255, 80, 80, "    boot failed: {:?}", e);
                match e {
                    boot::BootError::SecureBootTimeout(s) => cprintln!(120, 140, 140,
                        "    FSP scratch is {:#04x}, not 0xff: card not POSTed or wrong scratch reg", s),
                    boot::BootError::FirmwareMissing(p) => cprintln!(120, 140, 140,
                        "    copy the blob to /lib/firmware/{} and rebuild the disk image", p),
                    boot::BootError::FirmwareRead(p, fe) => cprintln!(120, 140, 140,
                        "    {} exists but reading failed ({:?}): heap pressure or disk I/O error", p, fe),
                    boot::BootError::GfwNotCompleted(_) => {
                        cprintln!(120, 140, 140,
                            "    GFW devinit did not finish (progress != 0xff); GSP-RM cannot init.");
                        cprintln!(120, 140, 140,
                            "    Fully power off the machine (PSU switch off ~10 s), boot, retry.");
                        cprintln!(120, 140, 140,
                            "    If it is still 0x00 after a cold boot: disable CSM / boot via UEFI.");
                    }
                    boot::BootError::Fsp(_) => cprintln!(120, 140, 140,
                        "    FSP NACKed or timed out; see serial log for the NVDM error code"),
                    boot::BootError::TargetMaskTimeout { .. } => {
                        cprintln!(120, 140, 140,
                            "    the BAR0 decoupler over NV_PGSP was never lifted: the GSP-FMC");
                        cprintln!(120, 140, 140,
                            "    never handed the falcon back, so it died before starting GSP-RM.");
                        cprintln!(120, 140, 140,
                            "    Check the GSP-FMC error code with: nvidia gb206-scratch");
                    }
                    boot::BootError::FmcErrorCode { .. } => cprintln!(120, 140, 140,
                        "    the FMC posted a fatal code; decoded above (OpenRM gpuReportGspFmcErrorCode)"),
                    boot::BootError::RiscvNotActive { .. } => cprintln!(120, 140, 140,
                        "    lockdown lifted but the RISC-V core never started - FMC handoff failed"),
                    _ => {}
                }
            }
        }
        // Last, and through the console rather than PRAMIN: what `boot_opts`
        // prints goes into video memory directly, and by then the framebuffer
        // console owns that surface and repaints it from its own character
        // grid. Printed even when empty, so it is clear the step ran
        let rm = boot::take_rm_log();
        cprintln!(230, 240, 240, "=== RM ===");
        if rm.is_empty() {
            cprintln!(255, 200, 80, "  nothing - the RM client step did not run");
        }
        for l in &rm {
            cprintln!(230, 240, 240, "{}", l);
        }
        cprintln!(230, 240, 240, "=== RM end ===");
        // What the last repaint cost, and who did it. The PRAMIN path is one
        // MMIO write per pixel and a full screen takes about 42 ms; the copy
        // engine moves the same rectangle as one 2D blit
        //
        // Pushes everything above to the panel now, so the number below is
        // about this repaint: the flush hook runs from the shell loop after a
        // command returns
        crate::io::console::flush();
        let (us, by_ce, why) = crate::drivers::gpu::nvidia::scanout::last_flush();
        if by_ce {
            cprintln!(150, 210, 255, "  last console repaint: {} us via the copy engine", us);
        } else {
            cprintln!(255, 200, 80, "  last console repaint: {} us via PRAMIN - {}", us, why);
        }
    });
    if ran.is_none() {
        cprintln!(120, 140, 140, "  no GB206 card registered");
    }
}

/// Prints the RM object tree
///
/// Every object this driver has allocated and not freed, nested under its
/// parent, to read against OpenRM: a channel sitting directly under the device,
/// where the reference has a timeslice group in between, shows here
fn cmd_nvidia_rm_tree() {
    use crate::drivers::gpu::nvidia::gsp_common::rmapi;
    let lines = rmapi::tree_lines();
    if lines.is_empty() {
        crate::println!("  nvidia rm-tree: no RM objects - the card was never booted");
        return;
    }
    crate::println!("  RM objects: {}", rmapi::object_count());
    for l in &lines {
        crate::println!("    {}", l);
    }

    // The graphics context is memory rather than objects, and the one thing
    // here the card describes itself. It goes past on the boot log in under a
    // second, so it is kept and printed here
    let ectx = crate::drivers::gpu::nvidia::gsp_common::chan::ECTX_LOG.lock();
    if !ectx.is_empty() {
        crate::println!("  engine context pointer in the instance block, per binding:");
        for l in ectx.iter() {
            crate::println!("    {}", l);
        }
    }
    drop(ectx);

    let ctx = crate::drivers::gpu::nvidia::gsp_common::compute::context_report();
    if ctx.is_empty() {
        crate::println!("  graphics context: none, the compute channel did not come up");
    } else {
        crate::println!("  graphics context: {} buffer(s), as RM describes them", ctx.len());
        for l in &ctx {
            crate::println!("    {}", l);
        }
    }
}

/// Builds and tears the copy engine's channel down, N times, printing what the
/// driver holds after each round.
///
/// A leak is a count that climbs; a use-after-free is a machine that stops
fn cmd_nvidia_chan_cycle(args: &str) {
    use crate::drivers::gpu::nvidia::blackwell::boot;

    let rounds: u32 = args.trim().parse().unwrap_or(1);
    if rounds == 0 || rounds > 64 {
        crate::print_error!("  usage: nvidia chan-cycle [1..64]");
        return;
    }
    let pdb = *boot::CHAN_PDB.lock();
    let say = |s: &str| {
        cprintln!(230, 240, 240, "  {}", s);
        crate::serial_println!("[nvidia] {}", s);
    };
    match boot::with_rm(|bar0, rpc, client| {
        boot::chan_cycle(bar0, rpc, client, pdb, rounds, &say);
    }) {
        Ok(()) => {}
        Err(e) => crate::print_error!("  nvidia chan-cycle: {}", e),
    }
}

/// Everything about this driver that can be decided without the card.
///
/// The GB206 is not modelled by QEMU, so this costs a boot rather than a reboot
/// cycle. A wrong level index in the page-table arithmetic is a cleared PDE
/// under a live compositor
fn cmd_nvidia_selftest() {
    let report = crate::drivers::gpu::nvidia::selftest::run();
    let (pass, fail) = (report.passed(), report.failed());

    for c in &report.cases {
        if c.skipped {
            cprintln!(220, 220, 100, "  skip  {}", c.name);
        } else if c.ok {
            cprintln!(100, 220, 150, "  ok    {}", c.name);
        } else {
            crate::print_error!("  fail  {}", c.name);
        }
        crate::println!("          {}", c.detail);
    }

    if fail == 0 {
        cprintln!(100, 220, 150, "  {} of {} passed", pass, pass);
    } else {
        crate::print_error!("  {} passed, {} failed", pass, fail);
    }
    let skip = report.skipped();
    if skip > 0 {
        cprintln!(220, 220, 100, "  {} skipped", skip);
    }
}

/// The bus as `lspci -vv` shows it, for the GPU and the root port above it, and
/// what GSP-RM was told about the machine (`dev/gsp-foundation.md`, step 1.17),
/// so a boot here and a boot of Linux on the same machine can be compared line
/// by line
fn cmd_nvidia_pcie() {
    use crate::drivers::bus::pci;
    use crate::drivers::gpu::nvidia::chipset;
    let Some(d) = crate::drivers::gpu::nvidia::device() else {
        cprintln!(255, 80, 80, "  no NVIDIA GPU");
        return;
    };
    let gpu = &d.pci;
    let speed = |code: u32| match code {
        1 => "2.5GT/s (Gen1)", 2 => "5GT/s (Gen2)", 3 => "8GT/s (Gen3)",
        4 => "16GT/s (Gen4)", 5 => "32GT/s (Gen5)", 6 => "64GT/s (Gen6)", _ => "unknown speed",
    };
    let pm = |b: bool| if b { "+" } else { "-" };
    let port = |label: &str, bus: u8, dev: u8, func: u8| {
        let id = pci::pci_read32(bus, dev, func, 0);
        cprintln!(118, 185, 0, "  {} {:02x}:{:02x}.{} {:04x}:{:04x}", label, bus, dev, func, id & 0xFFFF, id >> 16);
        let Some(cap) = pci::find_cap(bus, dev, func, pci::CAP_ID_PCIE) else {
            crate::println!("    no PCI Express capability");
            return;
        };
        let rd = |off: u8| pci::pci_read32(bus, dev, func, cap + off);
        let (devcap, devctl, lnkcap, lnk, devcap2, devctl2, lnk2) =
            (rd(0x04), rd(0x08), rd(0x0C), rd(0x10), rd(0x24), rd(0x28), rd(0x30));
        let (devsta, lnkctl, lnksta) = (devctl >> 16, lnk & 0xFFFF, lnk >> 16);
        let aspm = |v: u32| match v & 3 { 0 => "off", 1 => "L0s", 2 => "L1", _ => "L0s L1" };
        crate::println!("    LnkCap  {} x{}, ASPM {}", speed(lnkcap & 0xF), (lnkcap >> 4) & 0x3F, aspm(lnkcap >> 10));
        crate::println!("    LnkSta  {} x{}{}", speed(lnksta & 0xF), (lnksta >> 4) & 0x3F,
                        if lnksta & (1 << 11) != 0 { ", training" } else { "" });
        crate::println!("    LnkCtl  ASPM {}, CommClk{}; target {}", aspm(lnkctl), pm(lnkctl & (1 << 6) != 0),
                        speed(lnk2 & 0xF));
        let sta2 = lnk2 >> 16;
        crate::println!("    LnkSta2 equalization complete{} phase1{} phase2{} phase3{}",
                        pm(sta2 & 2 != 0), pm(sta2 & 4 != 0), pm(sta2 & 8 != 0), pm(sta2 & 16 != 0));
        crate::println!("    DevCap  MaxPayload {} bytes, ExtTag{}, FLR{}",
                        128u32 << (devcap & 7), pm(devcap & (1 << 5) != 0), pm(devcap & (1 << 28) != 0));
        crate::println!("    DevCtl  MaxPayload {} bytes, MaxReadReq {} bytes, RlxdOrd{} ExtTag{} NoSnoop{}",
                        128u32 << ((devctl >> 5) & 7), 128u32 << ((devctl >> 12) & 7),
                        pm(devctl & (1 << 4) != 0), pm(devctl & (1 << 8) != 0), pm(devctl & (1 << 11) != 0));
        crate::println!("    DevSta  CorrErr{} NonFatalErr{} FatalErr{} UnsupReq{} TransPend{}",
                        pm(devsta & 1 != 0), pm(devsta & 2 != 0), pm(devsta & 4 != 0),
                        pm(devsta & 8 != 0), pm(devsta & 32 != 0));
        crate::println!("    DevCap2 LTR{}, 10BitTag completer{} requester{}, atomics route{} 32{} 64{} 128CAS{}",
                        pm(devcap2 & (1 << 11) != 0), pm(devcap2 & (1 << 16) != 0), pm(devcap2 & (1 << 17) != 0),
                        pm(devcap2 & (1 << 6) != 0), pm(devcap2 & (1 << 7) != 0),
                        pm(devcap2 & (1 << 8) != 0), pm(devcap2 & (1 << 9) != 0));
        crate::println!("    DevCtl2 LTR{}, 10BitTag{}, atomics requester{}, completion timeout {:#x}",
                        pm(devctl2 & (1 << 10) != 0), pm(devctl2 & (1 << 12) != 0),
                        pm(devctl2 & (1 << 6) != 0), devctl2 & 0xF);
        // Errors on the link are the first sign of a bus that is not stable,
        // and they live in extended config space
        match pci::find_ext_cap(bus, dev, func, pci::EXT_CAP_AER) {
            Some(aer) => {
                let ue = pci::read32_ext(bus, dev, func, aer + 4).unwrap_or(0);
                let ce = pci::read32_ext(bus, dev, func, aer + 0x10).unwrap_or(0);
                // Bit 13 is Advisory Non-Fatal: a request the function answered
                // Unsupported Request, logged as correctable because the
                // requester was told. Probing a function number the device does
                // not have is enough (every enumeration does it, Linux's
                // included), so alone it says nothing about the link
                const ADVISORY: u32 = 1 << 13;
                let line = alloc::format!("    AER     uncorrectable {:#010x}, correctable {:#010x} \
                    (RxErr{} BadTLP{} BadDLLP{} Rollover{} Timeout{} AdvNonFatalErr{})",
                    ue, ce, pm(ce & 1 != 0), pm(ce & (1 << 6) != 0), pm(ce & (1 << 7) != 0),
                    pm(ce & (1 << 8) != 0), pm(ce & (1 << 12) != 0), pm(ce & ADVISORY != 0));
                if ue == 0 && ce == 0 {
                    cprintln!(100, 220, 150, "    AER     no uncorrectable or correctable errors recorded");
                } else if ue == 0 && ce == ADVISORY {
                    crate::println!("{}", line);
                    crate::println!("            advisory only: an Unsupported Request answered, not a link error");
                } else {
                    cprintln!(255, 200, 80, "{}", line);
                }
            }
            None if !pci::has_ecam() => crate::println!("    AER     not reachable: no MCFG, no extended config space"),
            None => crate::println!("    AER     the function has no AER capability"),
        }
    };
    port("GPU", gpu.bus, gpu.dev, gpu.func);
    match pci::root_port_of(gpu.bus) {
        Some(r) => port("root port", r.bus, r.dev, r.func),
        None => cprintln!(255, 200, 80, "  no root port found above the GPU"),
    }
    let Some(cl) = chipset::last() else {
        crate::println!("  the bus was not set up this boot - GSP-RM has not been booted");
        return;
    };
    let dev = |d: Option<chipset::Device>| match d {
        Some(d) => alloc::format!("{:02x}:{:02x}.{} {:04x}:{:04x} (subsystem {:04x}:{:04x}, rev {:02x})",
                                  d.bus, d.dev, d.func, d.vendor, d.device, d.subvendor, d.subdevice, d.revision),
        None => alloc::string::String::from("none"),
    };
    cprintln!(118, 185, 0, "  told GSP-RM{}", if crate::arch::x86_64::grub::cmdline_flag("gspbase") {
        " - nothing: gspbase sends the chipset half as zeros" } else { "" });
    crate::println!("    chipset \"{}\" ({:#06x}), recognised by {}", cl.name, cl.chipset, dev(cl.chipset_id));
    crate::println!("    first host bridge {}", dev(cl.fhb));
    crate::println!("    upstream port {}, behind a bridge {}", dev(cl.upstream), if cl.behind_bridge { "yes" } else { "no" });
    crate::println!("    upstream L0s unsupported {}, L1 unsupported {}, L1 POR {}{}",
                    cl.upstream_l0s_unsupported, cl.upstream_l1_unsupported, cl.upstream_l1_por_supported,
                    if cl.upstream_l1_por_mobile_only { " (mobile only)" } else { "" });
    crate::println!("    L1 substates control {:#010x}, atomics requester {:#x}, completer {:#x}",
                    cl.l1ss_control1, cl.atomics_op_mask, cl.atomics_cpl_mask);
    crate::println!("    properties {:#x}:", cl.props);
    for name in cl.prop_names() {
        crate::println!("      {}", name);
    }
    crate::println!("    Device Control {:#06x} at boot, {:#06x} after{}", cl.devctl_before, cl.devctl_after,
                    if chipset::base_devctl() { " - pciebase left it alone" } else { "" });
}

/// P-states and clocks (`gsp_common/perf`, `dev/gpu-perf.md`): what the
/// firmware chose and the numbers it writes into the shared page; `clocks` is
/// the PMU's last seconds, and `lock`, `floor` and `boost` ask it for more
fn cmd_nvidia_perf(args: &str) {
    let mut words = args.split_whitespace();
    match words.next() {
        None => perf_summary(),
        Some("boost") => perf_boost(words),
        Some("clocks") => perf_clocks(),
        Some("lock") => perf_lock(words.next(), false),
        Some("floor") => perf_lock(words.next(), true),
        Some(other) => cprintln!(255, 200, 80,
            "  nvidia perf [clocks | lock tdp|turbo|off | floor tdp|turbo|off | boost ...] - not '{}'", other),
    }
}

/// `nvidia perf boost max|1|off [seconds|forever]`: `PERF_BOOST`, the way
/// `nvidia-smi` and CUDA ask for it
fn perf_boost(mut words: core::str::SplitWhitespace) {
    use crate::drivers::gpu::nvidia::gsp_common::perf::{self, Boost};
    let what = match words.next().unwrap_or("max") {
        "max" => Boost::ToMax,
        "1" | "one" | "level" => Boost::OneLevel,
        "off" | "clear" => Boost::Clear,
        other => {
            cprintln!(255, 200, 80, "  nvidia perf boost max|1|off [seconds] - not '{}'", other);
            return;
        }
    };
    let seconds = match words.next() {
        Some("forever") => perf::BOOST_UNTIL_CLEARED,
        Some(n) => match n.parse::<u32>() {
            Ok(v) if v >= 1 && v <= perf::BOOST_MAX_SECONDS => v,
            _ => {
                cprintln!(255, 200, 80, "  seconds is 1..{} or 'forever'", perf::BOOST_MAX_SECONDS);
                return;
            }
        },
        None => 60,
    };
    match crate::drivers::gpu::nvidia::blackwell::boot::with_rm(|_bar0, rpc, client| perf::boost(rpc, client, what, seconds)) {
        Ok(Ok(())) if what == Boost::Clear => cprintln!(100, 220, 150, "  boost cleared"),
        Ok(Ok(())) => cprintln!(100, 220, 150, "  boost {:?} for {}", what,
            if seconds == perf::BOOST_UNTIL_CLEARED { alloc::string::String::from("as long as it is not cleared") }
            else { alloc::format!("{} s", seconds) }),
        Ok(Err(e)) => cprintln!(255, 80, 80, "  GSP-RM refused the boost: {:?}", e),
        Err(e) => cprintln!(255, 80, 80, "  {}", e),
    }
}

/// What the firmware chose and the numbers it writes into the shared page
/// (`dev/gpu-perf.md`), then what was asked of it and what limits it
fn perf_summary() {
    use crate::drivers::gpu::nvidia::blackwell::boot;
    use crate::drivers::gpu::nvidia::gsp_common::perf::{self, limits, rusd};
    use crate::drivers::gpu::nvidia::gsp_common::modeset::imp;

    match boot::with_rm(|_bar0, rpc, client| perf::current_pstate(rpc, client)) {
        Ok(Ok(Some(p))) => cprintln!(118, 185, 0, "  P-state: P{}{}", p, if p == 0 { " (full speed)" } else { "" }),
        Ok(Ok(None)) => cprintln!(255, 200, 80, "  P-state: GSP-RM says it is undefined"),
        Ok(Err(e)) => cprintln!(255, 200, 80, "  P-state: GSP-RM would not say ({:?})", e),
        Err(e) => {
            cprintln!(255, 80, 80, "  {}", e);
            return;
        }
    }

    // The first read asks GSP-RM to poll (through the event thread), so the
    // page can take one interval to come alive
    let mut snap = boot::perf_snapshot();
    let mut waited_ms = 0;
    while snap.as_ref().is_some_and(|s| !s.fresh()) && waited_ms < rusd::FRESH_MS {
        crate::sched::sleep(25);
        waited_ms += 100;
        snap = boot::perf_snapshot();
    }
    match snap {
        Some(s) => perf_show(&s),
        None => {
            let (_, _, refused) = rusd::state();
            cprintln!(255, 200, 80, "  shared data: GSP-RM has no page to write into{}",
                if refused { " (it refused one)" } else { " - 'nvidia log perf'" });
        }
    }

    match boot::with_rm(|_bar0, rpc, client| limits::power_source(rpc, client)) {
        Ok(Ok(limits::PowerSource::Ac)) => cprintln!(230, 240, 240, "  power source: AC"),
        Ok(Ok(other)) => cprintln!(255, 200, 80, "  power source: {:?} - a desktop is never told anything but AC", other),
        Ok(Err(e)) => cprintln!(255, 200, 80, "  power source: GSP-RM would not say ({:?})", e),
        Err(e) => cprintln!(255, 80, 80, "  {}", e),
    }
    match boot::with_rm(|_bar0, rpc, client| limits::get(rpc, client)) {
        Ok(Ok((Some(limits::Action::Default), _))) => cprintln!(230, 240, 240, "  clock lock (client OS): none"),
        Ok(Ok((Some(a), vp))) => cprintln!(255, 200, 80, "  clock lock (client OS): {:?} at {}", a,
            if vp == limits::VPstate::TurboBoost as u32 { "turbo boost" } else { "rated TDP" }),
        Ok(Ok((None, vp))) => cprintln!(255, 200, 80, "  clock lock (client OS): action this driver has no name for (vpstate {})", vp),
        Ok(Err(e)) => cprintln!(255, 200, 80, "  clock lock: GSP-RM would not say ({:?})", e),
        Err(e) => cprintln!(255, 80, 80, "  {}", e),
    }
    for (head, _) in crate::drivers::gpu::nvidia::gsp_common::output::lit_heads() {
        match imp::floor(head) {
            Some(p) => cprintln!(230, 240, 240, "  display: head {} needs P{} or faster (IMP at its modeset)", head, p),
            None => cprintln!(120, 140, 140, "  display: head {} - IMP said nothing about its P-state", head),
        }
    }

    // The link every read of system memory crosses. The card brings it down at
    // idle, and at Gen1 a frame reading windows from system memory cannot be on
    // time
    if let Some(d) = crate::drivers::gpu::nvidia::device() {
        match crate::drivers::bus::msi::pcie_link(d.pci.pci_fn(), &d.caps) {
            Some(l) => cprintln!(118, 185, 0, "  PCIe: Gen{} x{} now, Gen{} x{} at best", l.gen, l.width, l.max_gen, l.max_width),
            None => cprintln!(255, 200, 80, "  PCIe: the card has no PCI Express capability to read"),
        }
    }
    cprintln!(120, 140, 140, "  'nvidia perf clocks' - the last seconds of clocks; 'lock', 'floor', 'boost' ask for more");
}

/// One read of the shared page, as it was
fn perf_show(s: &crate::drivers::gpu::nvidia::gsp_common::perf::Snapshot) {
    use crate::drivers::gpu::nvidia::gsp_common::perf::{self, rusd};
    let watts = |mw: u32| alloc::format!("{}.{} W", mw / 1000, mw % 1000 / 100);
    let age = match s.age {
        Some(rusd::Age::Ptimer(ms)) => alloc::format!("{} ms old by PTIMER", ms),
        Some(rusd::Age::SinceMoved(ms)) => alloc::format!("{} ms since it last moved", ms),
        None => alloc::string::String::from("age unknown"),
    };
    if s.fresh() {
        cprintln!(118, 185, 0, "  shared data, GSP-RM's ({}):", age);
    } else {
        cprintln!(255, 200, 80, "  shared data, NOT fresh ({}) - GSP-RM is not polling into it:", age);
    }
    // A section left behind by the rest of the page is flagged, with its own
    // age
    let line = |name: &str, text: &str, own: Option<u64>| {
        if s.current(own) {
            cprintln!(230, 240, 240, "    {:<12} {}", name, text);
        } else {
            cprintln!(255, 200, 80, "    {:<12} {}  - {} ms old, behind the clocks", name, text, own.unwrap_or(0));
        }
    };
    let a = s.ages;
    if let Some(p) = s.pstate {
        line("P-state", &alloc::format!("P{}", p), a.pstate);
    }
    match s.clocks {
        Some(c) => {
            let mhz = |k: u32| alloc::format!("{} MHz", k / 1000);
            // SM reads 0 on this card: not reported, rather than stopped
            let sm = if c[perf::SM].actual_khz != 0 { alloc::format!(", SM {}", mhz(c[perf::SM].actual_khz)) }
                else { alloc::string::String::new() };
            line("clocks", &alloc::format!("graphics {} (aims {}){}, memory {} (aims {}), video {}",
                mhz(c[perf::GRAPHICS].actual_khz), c[perf::GRAPHICS].target_mhz, sm,
                mhz(c[perf::MEMORY].actual_khz), c[perf::MEMORY].target_mhz, mhz(c[perf::VIDEO].actual_khz)), None);
        }
        None => cprintln!(120, 140, 140, "    clocks       never written"),
    }
    if let Some((gpu, mem)) = s.util {
        line("busy", &alloc::format!("graphics {}%, memory {}%", gpu, mem), a.util);
    }
    let mut power = alloc::vec::Vec::new();
    if let Some(p) = s.power_mw { power.push(alloc::format!("{} average", watts(p))); }
    if let Some(p) = s.inst_power_mw { power.push(alloc::format!("{} now", watts(p))); }
    if let Some(p) = s.tgp_mw { power.push(alloc::format!("{} board", watts(p))); }
    if let Some((asked, enforced)) = s.limit_mw {
        power.push(alloc::format!("limit {} (asked {})", watts(enforced), watts(asked)));
    }
    if !power.is_empty() {
        line("power", &power.join(", "), a.power);
    }
    let temps: alloc::vec::Vec<_> = (0..rusd::TEMP_TYPES)
        .filter_map(|k| s.temps_dc[k].map(|t| alloc::format!("{} {}.{} C", perf::TEMP_SENSORS[k], t / 10, (t % 10).abs())))
        .collect();
    if !temps.is_empty() {
        line("temperature", &temps.join(", "), a.temps[0]);
    }
    if let Some(f) = s.fans() {
        let rpm: alloc::vec::Vec<_> = f.iter().map(|r| alloc::format!("{}", r)).collect();
        line("fans", &alloc::format!("{} rpm", rpm.join(" / ")), a.fans);
    }
    if let Some(mask) = s.throttle {
        let names = perf::throttle_names(mask);
        line("held down by",
            &if names.is_empty() { alloc::string::String::from("nothing") } else { names.join(", ") }, a.throttle);
    }
}

/// `nvidia perf clocks`: the PMU's last seconds of GPC and DRAM clocks, one
/// line per change
fn perf_clocks() {
    use crate::drivers::gpu::nvidia::gsp_common::perf::clocks;
    let samples = match crate::drivers::gpu::nvidia::blackwell::boot::with_rm(|_bar0, rpc, client| clocks::query(rpc, client)) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            cprintln!(255, 200, 80, "  clocks: GSP-RM would not say ({:?})", e);
            return;
        }
        Err(e) => {
            cprintln!(255, 80, 80, "  {}", e);
            return;
        }
    };
    let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
        cprintln!(255, 200, 80, "  clocks: the PMU's clock queue is empty");
        return;
    };
    let span = last.timestamp.saturating_sub(first.timestamp) / 1_000_000;
    let mhz = |k: u32| if k == clocks::INVALID { alloc::string::String::from("-") } else { alloc::format!("{}", k / 1000) };
    cprintln!(118, 185, 0, "  {} clock sample(s) over {} ms (the PMU's, oldest first):", samples.len(), span);
    for c in clocks::changes(&samples) {
        cprintln!(230, 240, 240, "    +{:>5} ms  gpc {:>5} MHz  dram {:>5} MHz  ({} sample(s))",
            c.after_ms, mhz(c.gpc_khz), mhz(c.dram_khz), c.samples);
    }
    cprintln!(120, 140, 140, "    newest: gpc {} MHz (active {}), dram {} MHz, nvd {} MHz",
        mhz(last.gpc_khz), mhz(last.active_gpc_khz), mhz(last.dram_khz), mhz(last.nvd_khz));
}

/// `nvidia perf lock tdp|turbo|off`, `nvidia perf floor tdp|turbo|off`:
/// RATED_TDP as client OS - the clocks fixed at a vPstate, or held no lower
/// than it
fn perf_lock(what: Option<&str>, floor: bool) {
    use crate::drivers::gpu::nvidia::gsp_common::perf::limits::{self, Action, VPstate};
    let (action, vpstate) = match what.unwrap_or("tdp") {
        "tdp" => (if floor { Action::ForceFloor } else { Action::ForceLock }, VPstate::RatedTdp),
        "turbo" => (if floor { Action::ForceFloor } else { Action::ForceLock }, VPstate::TurboBoost),
        "off" => (Action::Default, VPstate::RatedTdp),
        other => {
            cprintln!(255, 200, 80, "  nvidia perf {} tdp|turbo|off - not '{}'", if floor { "floor" } else { "lock" }, other);
            return;
        }
    };
    let r = crate::drivers::gpu::nvidia::blackwell::boot::with_rm(|_bar0, rpc, client| {
        limits::set(rpc, client, action, vpstate)?;
        limits::get(rpc, client)
    });
    match r {
        Ok(Ok((now, vp))) => cprintln!(100, 220, 150, "  asked {:?} at {:?}; GSP-RM now says {:?} (vpstate {}) - lost at a GSP reload",
            action, vpstate, now, vp),
        Ok(Err(e)) => cprintln!(255, 80, 80, "  GSP-RM refused it: {:?}", e),
        Err(e) => cprintln!(255, 80, 80, "  {}", e),
    }
}

/// The GPU address spaces this driver owns the tables of (`vaspace.rs`,
/// `dev/gpu-memory-compute.md` 3.2): each reservation with its owner and how
/// much is mapped in it, and with `all` every mapping
fn cmd_nvidia_va(args: &str) {
    use crate::drivers::gpu::nvidia::gmmu::Aperture;
    let all = args.split_whitespace().any(|a| a == "all" || a == "-v");
    let spaces = crate::drivers::gpu::nvidia::vaspace::spaces();
    if spaces.is_empty() {
        cprintln!(120, 140, 140, "  no GPU address space - the channels are not up");
        return;
    }
    for sp in &spaces {
        cprintln!(118, 185, 0, "  {} (pdb {:#x}): {} reservation(s), {} mapping(s)",
            sp.name, sp.pdb, sp.blocks.len(), sp.mapping_count());
        for b in &sp.blocks {
            let mapped: u64 = b.maps.iter().map(|m| m.bytes).sum();
            let mut pages = crate::drivers::gpu::nvidia::gmmu::Pages::default();
            for m in &b.maps {
                pages.huge += m.pages.huge;
                pages.big += m.pages.big;
                pages.small += m.pages.small;
            }
            let (r, g, bl) = if b.implicit { (255, 200, 80) } else { (230, 240, 240) };
            cprintln!(r, g, bl, "    {:#013x}..{:#013x}  {:<19} {:>3} mapping(s), {:>9} mapped  {}{}",
                b.va, b.va + b.bytes, b.owner, b.maps.len(), size_str(mapped), pages_str(&pages),
                if b.implicit { "  - in no reservation" } else { "" });
            if all {
                let mut maps = b.maps.clone();
                maps.sort_by_key(|m| m.va);
                for m in &maps {
                    cprintln!(150, 170, 170, "      {:#013x} -> {:#014x} {:>9} {}{}  {}",
                        m.va, m.phys, size_str(m.bytes),
                        if m.aper == Aperture::Vram { "vram" } else { "sysmem" },
                        if m.privileged { " priv" } else { "" },
                        pages_str(&m.pages));
                }
            }
        }
    }
    if !all {
        cprintln!(120, 140, 140, "  'nvidia va all' lists every mapping");
    }
}

/// The GPU pages a mapping took, largest first and only the sizes it used:
/// `2M x4  64K x30  4K x9` (`dev/gpu-memory-compute.md`, 3.8)
fn pages_str(p: &crate::drivers::gpu::nvidia::gmmu::Pages) -> alloc::string::String {
    let mut s = alloc::string::String::new();
    for (name, n) in [("2M", p.huge), ("64K", p.big), ("4K", p.small)] {
        if n != 0 {
            if !s.is_empty() {
                s.push_str("  ");
            }
            s.push_str(&alloc::format!("{} x{}", name, n));
        }
    }
    s
}

/// The video-memory map: what the board has, what has been handed out, and to
/// whom.
///
/// Read-only. A block nobody recognises is a leak, and two owners over the same
/// bytes shows up as a picture rather than a fault
fn cmd_nvidia_vram() {
    use crate::drivers::gpu::nvidia::gsp_common::vram;

    if !vram::seeded() {
        cprintln!(255, 200, 80,
            "  the framebuffer layout is not known yet; run nvidia gb206-boot first");
        return;
    }
    let m = vram::map();
    let (free, used) = vram::totals();

    cprintln!(118, 185, 0, "  regions GSP-RM reported");
    for (r, usable) in m.regions.iter() {
        // A region not usable is one RM has spoken for: reserved, protected, or
        // without the isochronous support scanout needs. Printed anyway, to
        // show where the firmware console went
        let (cr, cg, cb) = if *usable { (230, 240, 240) } else { (120, 140, 140) };
        cprintln!(cr, cg, cb, "    {:#014x}..{:#014x}  {:>9}  {}",
            r.base, r.base + r.size, size_str(r.size),
            if *usable { "usable" } else { "reserved by RM" });
    }

    cprintln!(118, 185, 0, "  handed out ({} in {} block(s))", size_str(used), m.used.len());
    for b in m.used.iter() {
        cprintln!(230, 240, 240, "    {:#014x}  {:>9}  {}", b.base, size_str(b.size), b.owner);
    }
    if m.used.is_empty() {
        cprintln!(120, 140, 140, "    nothing");
    }

    cprintln!(118, 185, 0, "  free ({} in {} range(s))", size_str(free), m.free.len());
    for r in m.free.iter() {
        cprintln!(230, 240, 240, "    {:#014x}..{:#014x}  {:>9}",
            r.base, r.base + r.size, size_str(r.size));
    }

    // The GPU's own address space, which is system memory and so in none of the
    // numbers above
    let (tables, spare) = crate::drivers::gpu::nvidia::gmmu::table_counts();
    cprintln!(118, 185, 0, "  gpu address space");
    cprintln!(230, 240, 240,
        "    {} range(s) mapped, {} page table(s) in system memory ({} spare)",
        crate::drivers::gpu::nvidia::vaspace::mapping_count(), tables, spare);
    // The number deciding whether nouveau's ALL_PDB hack is needed on this
    // chip. ALL_PDB throws away every address space's translations, GSP-RM's
    // included, and a bind is a per-frame operation, so if the targeted form is
    // never refused, the second write buys nothing
    let (inv, skipped) = crate::drivers::gpu::nvidia::gmmu::invalidate_counts();
    let (inv_us, inv_max) = crate::drivers::gpu::nvidia::gmmu::invalidate_time();
    cprintln!(230, 240, 240,
        "    {} TLB invalidate(s), {} us in all, the longest {} us; the targeted form was refused {} time(s)",
        inv, inv_us, inv_max, skipped);
    if inv != 0 && skipped == 0 {
        cprintln!(120, 140, 140,
            "    (never refused - `targeted_is_enough` in gmmu.rs is what flips)");
    }
    let chids = crate::drivers::gpu::nvidia::gsp_common::chan::chids_in_use();
    cprintln!(230, 240, 240,
        "    channel ids in use: {:#b} ({} channel(s); bit 0 is RM's)",
        chids, chids.count_ones().saturating_sub(1));
    let unbound = crate::mayland::server::shm::unbound();
    if unbound != 0 {
        cprintln!(230, 240, 240,
            "    {} mapping(s) dropped because their memory was given back", unbound);
    }
}

/// Every output the board reports, and what has been done with each.
///
/// The table is filled by `enumerate_displays` and updated by the modeset, so a
/// monitor missing here was missing from `GET_SUPPORTED`. The unconnected
/// outputs are listed for that reason
fn cmd_nvidia_disp(args: &str) {
    use crate::drivers::gpu::nvidia::gsp_common::output;

    // `nvidia disp underflow <id>` and the rest take a display id, printed by
    // the bare listing: a single bit, so `0x800` and `800` are the same output
    let (verb, rest) = match args.split_once(char::is_whitespace) {
        Some((v, r)) => (v, r.trim()),
        None => (args, ""),
    };
    if verb == "underflow" {
        // The one display fault invisible from this side unless asked. Reading
        // clears the latch, so each call reports the window since the last
        let Some(id) = parse_display_id(rest) else {
            cprintln!(255, 200, 80, "  usage: nvidia disp underflow <display-id>");
            return;
        };
        match crate::drivers::gpu::nvidia::blackwell::boot::underflow(id) {
            Ok(s) => {
                cprintln!(230, 240, 240, "  {}", s);
                cprintln!(120, 140, 140,
                    "  'under 0' means the raster kept up since the last call");
            }
            Err(e) => cprintln!(255, 80, 80, "  {}", e),
        }
        return;
    }
    if verb == "imp" {
        // RM's display-engine budget for each of an output's modes, beside the
        // link's answer. A second opinion only: nothing is refused on it
        // (`dev/display-nvkms.md`)
        let Some(id) = parse_display_id(rest) else {
            cprintln!(255, 200, 80, "  usage: nvidia disp imp <display-id>");
            return;
        };
        match crate::drivers::gpu::nvidia::blackwell::boot::imp_opinion(id) {
            Ok(lines) => {
                for l in &lines {
                    let warn = l.contains("DISAGREE") || l.contains("not answered");
                    if warn { cprintln!(255, 200, 80, "  {}", l); } else { cprintln!(230, 240, 240, "  {}", l); }
                }
            }
            Err(e) => cprintln!(255, 80, 80, "  {}", e),
        }
        return;
    }
    if verb == "poll" {
        // The event thread does this every 200 ms; this is the same pass on
        // demand
        let n = crate::drivers::gpu::nvidia::blackwell::boot::poll_events();
        cprintln!(100, 220, 150, "  {} event(s) drained", n);
        return;
    }
    if !args.is_empty() {
        cprintln!(255, 200, 80,
            "  unknown: nvidia disp [underflow <id>] [imp <id>] [poll] - changing a \
             display is 'display' now (mode, on, off, vrr, color)");
        return;
    }

    if output::count() == 0 {
        cprintln!(255, 200, 80,
            "  no outputs enumerated yet; the display bring-up has not run");
        return;
    }
    let primary = output::primary_id();
    let (connected, lit) = output::with_all(|outs| (
        outs.iter().filter(|o| o.connected).count(),
        outs.iter().filter(|o| o.lit()).count(),
    ));
    cprintln!(118, 185, 0, "  {} outputs, {} connected, {} scanning out",
        output::count(), connected, lit);
    output::with_all(|outs| {
        for o in outs {
            // Dim the ones with nothing plugged in: they are here so a hot-plug
            // has an entry to fill in
            let (cr, cg, cb) = if o.connected { (230, 240, 240) } else { (120, 140, 140) };
            // A SOR not assigned prints as a dash. There are fewer output
            // resources than connectors and RM binds one at modeset time, so
            // none is normal for a connector nothing is driven on
            let or = match o.or.or_assigned() {
                Some(i) => alloc::format!("{} #{}", o.or.type_name(), i),
                None => alloc::format!("{} -", o.or.type_name()),
            };
            cprintln!(cr, cg, cb, "    {:#010x}  {:<9} {:<7} {:<10} {}{}",
                o.display_id,
                crate::drivers::gpu::nvidia::kms::name_of(o.display_id)
                    .unwrap_or_else(|| alloc::string::String::from("-")),
                or, o.or.protocol_name(),
                if o.connected { "connected" } else { "nothing plugged in" },
                if primary == Some(o.display_id) { "  (primary)" } else { "" });
            if !o.connected {
                continue;
            }
            if !o.name.is_empty() {
                cprintln!(cr, cg, cb, "        monitor \"{}\", {} B edid", o.name, o.edid.len());
            }
            // What the monitor said about HDMI, which it can only say in a CEA
            // extension. Silence here on a DisplayPort panel is right
            if o.hdmi.hdmi || o.hdmi.max_tmds_khz != 0 {
                cprintln!(cr, cg, cb, "        hdmi sink{}{}{}",
                    if o.hdmi.max_tmds_khz != 0 {
                        alloc::format!(", max TMDS {} kHz", o.hdmi.max_tmds_khz)
                    } else {
                        alloc::string::String::new()
                    },
                    if o.hdmi.scdc { ", SCDC" } else { "" },
                    if o.hdmi.scramble_below_340 { ", scrambles below 340 MHz" } else { "" });
            }
            if let Some(d) = o.dpcd {
                cprintln!(cr, cg, cb, "        dpcd rev {}.{}, {} lanes x {} = {} Mbps",
                    d.rev >> 4, d.rev & 0xf, d.max_lane_count, d.rate_name(), d.link_mbps());
            }
            // The monitor's modes, its refresh range and its colour are the
            // display layer's to show: 'display'
            match (o.head, o.sor) {
                (Some(h), Some(s)) => cprintln!(cr, cg, cb, "        head {} SOR #{}", h, s),
                (Some(h), None) => cprintln!(cr, cg, cb, "        head {}", h),
                _ => cprintln!(120, 140, 140, "        not driven"),
            }
            match o.scanout {
                Some(l) => cprintln!(cr, cg, cb,
                    "        scanout {}x{} at vram {:#x}, pitch {}, window {}{}",
                    l.width, l.height, l.vram, l.pitch, l.window,
                    if !o.lit() { "  (asleep - nothing reaches the monitor)" } else { "" }),
                None => cprintln!(120, 140, 140, "        nothing scanned out"),
            }
            if let Some(c) = o.cursor {
                cprintln!(cr, cg, cb, "        cursor plane {}x{} at vram {:#x}",
                    c.size, c.size, c.vram);
            }
        }
    });
}

/// A display id as typed: hex with or without the `0x`, or decimal.
///
/// Hex first because the listing prints them so and RM thinks of them so: a
/// display id is a bit position. A bare `800` is read as hex too, matching what
/// was read off the screen
fn parse_display_id(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    let id = u32::from_str_radix(hex, 16).ok()?;
    // Exactly one bit, or it is not a display id however it was typed
    (id != 0 && id.is_power_of_two()).then_some(id)
}

/// Bytes as something readable at a glance. Whole units only: a block is always
/// a round number of pages
fn size_str(bytes: u64) -> alloc::string::String {
    if bytes >= 1 << 20 && bytes % (1 << 20) == 0 {
        alloc::format!("{} MiB", bytes >> 20)
    } else if bytes >= 1 << 10 && bytes % (1 << 10) == 0 {
        alloc::format!("{} KiB", bytes >> 10)
    } else {
        alloc::format!("{} B", bytes)
    }
}

/// The interrupt vector table GSP-RM assigned, and what it means for the VFN
/// tree.
///
/// Read-only: nothing is enabled and no interrupt is unmasked
fn cmd_nvidia_intr() {
    use crate::drivers::gpu::nvidia::blackwell::boot;
    use crate::drivers::gpu::nvidia::gsp_common::intr;
    use crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc;

    use crate::drivers::gpu::nvidia::intr as tree;

    let (served, spurious, unclaimed, vector) = tree::stats();
    match vector {
        Some(v) => {
            let (other_vblank, disp_low) = boot::intr_stats();
            let gsp = crate::drivers::gpu::nvidia::gsp_common::kgsp::service::stats();
            cprintln!(118, 185, 0, "  MSI vector {:#x}: {} serviced, {} empty, {} unclaimed",
                v, served, spurious, unclaimed);
            cprintln!(230, 240, 240, "  frame loop: {}",
                crate::drivers::gpu::nvidia::scanout::vsync_path());
            cprintln!(230, 240, 240, "  vblanks counted: {}{}",
                crate::drivers::gpu::nvidia::scanout::vblank_count(),
                if other_vblank == 0 {
                    alloc::string::String::new()
                } else {
                    alloc::format!(" (+{} on a head nothing is pacing on)", other_vblank)
                });
            cprintln!(230, 240, 240, "  GSP falcon: {} serviced, {} with nothing pending, {} HALT",
                gsp.serviced, gsp.spurious, gsp.halts);
            // What the falcon itself holds: a bit pending, enabled and routed
            // to the host that the counts above never saw is an interrupt the
            // tree was not given
            if let Some((stat, mask, dest)) =
                crate::drivers::gpu::nvidia::with_gb20x(|d| boot::gsp_falcon_irq(&d.bar0))
            {
                let to_host = stat & mask & dest;
                let color = if to_host != 0 { (255, 200, 80) } else { (230, 240, 240) };
                cprintln!(color.0, color.1, color.2,
                    "    irqstat {:#010x} mask {:#010x} dest {:#010x} -> to host {:#010x}",
                    stat, mask, dest, to_host);
            }
            cprintln!(230, 240, 240, "  display low-latency line (DISP_LOW): {} interrupt(s)", disp_low);
            // Fences that wake their waiters (3.5a). A backstop count growing
            // with the frames is an interrupt not arriving. 'nononstall' on the
            // boot line puts the spin back
            {
                use crate::drivers::gpu::nvidia::gsp_common::chan;
                let f = chan::fence_stats();
                if chan::fences_wake() {
                    let (r, g, b) = if f.missed > f.woken / 10 + 5 { (255, 200, 80) } else { (230, 240, 240) };
                    cprintln!(r, g, b,
                        "  fences: {} interrupt(s); sleeping waits woken by one {}, outlived a tick {}, passed with no interrupt {}",
                        f.irqs, f.woken, f.long, f.missed);
                } else {
                    cprintln!(230, 240, 240, "  fences: waits spin (no fence interrupts, 'nononstall', or not set up)");
                }
                // Where submissions stored GP_PUT (3.5b): BAR1 when it is
                // physical, PRAMIN otherwise ('nobar1', or 'nvidia bar1 off').
                // A read-back that disagrees is a BAR1 window not on USERD
                let (bar1, pramin, wrong) = chan::gp_put_counts();
                let (r, g, b) = if wrong != 0 { (255, 200, 80) } else { (230, 240, 240) };
                cprintln!(r, g, b, "  GP_PUT: {} through BAR1 ({} read back wrong), {} through PRAMIN",
                    bar1, wrong, pramin);
            }
            if gsp.other != 0 {
                cprintln!(255, 200, 80,
                    "  {} unexpected GSP falcon interrupt(s), bits {:#010x} - each was disabled as it arrived",
                    gsp.other, gsp.other_bits);
            }
            if crate::drivers::gpu::nvidia::gsp_common::kgsp::fatal() {
                cprintln!(255, 80, 80, "  GSP-RM is DEAD - no RPCs go out and every channel has stopped");
            }
            {
                use crate::drivers::gpu::nvidia::gsp_common::kgsp::crashcat;
                let (reports, last) = crashcat::last();
                if reports > 0 {
                    cprintln!(255, 80, 80, "  GSP-CrashCat: {} report(s); the last:", reports);
                    for l in last.iter().take(12) {
                        cprintln!(255, 200, 80, "    {}", l);
                    }
                } else if crashcat::configured() {
                    cprintln!(230, 240, 240, "  GSP-CrashCat: queue armed, no crash reports");
                } else {
                    cprintln!(230, 240, 240, "  GSP-CrashCat: no queue this boot (gspbase) - a HALT is judged by the core's state");
                }
            }
            if unclaimed != 0 {
                cprintln!(255, 200, 80,
                    "  {} leaf read(s) had a bit nobody is registered for - cleared and dropped",
                    unclaimed);
            }
            // What GSP-RM reported rather than what the tree delivered. A card
            // that has never faulted shows zeroes
            let (rc, faults) =
                boot::trouble_counts();
            if rc == 0 && faults == 0 {
                cprintln!(230, 240, 240,
                    "  no robust-channel errors and no MMU faults reported");
            } else {
                cprintln!(255, 200, 80,
                    "  GSP-RM reported {} robust-channel error(s) and {} MMU fault notification(s)",
                    rc, faults);
            }
            let (ce_ok, forced) = boot::ce_health();
            if !ce_ok {
                cprintln!(255, 120, 80,
                    "  the copy engine's channel is marked dead - it takes no further work");
            }
            if forced != 0 {
                cprintln!(255, 200, 80,
                    "  {} copy-engine fence(s) written by the processor - those frames \
                     were reported complete without being observed to be",
                    forced);
            }
        }
        None => cprintln!(255, 200, 80,
            "  no MSI programmed - every wait is still a poll (nointr on the command line?)"),
    }

    if !boot::is_booted() {
        cprintln!(255, 200, 80, "  GSP-RM is not booted; run nvidia gb206-boot first");
        return;
    }
    // The one integrity check the 615 ring has: a record still marked SEEN
    match crate::drivers::gpu::nvidia::gsp_common::rpc::stale_records() {
        0 => cprintln!(230, 240, 240, "  message queue: no stale records"),
        n => cprintln!(255, 200, 80, "  message queue: {} stale record(s) skipped", n),
    }
    let Some(internal) = boot::internal_client() else {
        cprintln!(255, 200, 80, "  no internal RM handles - the display bring-up did not reach them");
        return;
    };
    let ran = crate::drivers::gpu::nvidia::with_gb20x(|_d| {
        let res = boot::with_rm(|_bar0, rpc, _client| intr::kernel_table(rpc, &internal));
        let table = match res {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => {
                cprintln!(255, 80, 80, "  INTR_GET_KERNEL_TABLE failed: {:?}", e);
                return;
            }
            Err(e) => {
                cprintln!(255, 80, 80, "  {}", e);
                return;
            }
        };
        // The shape the tree was enabled with, or the one it would be
        let (geometry, armed) = tree::shape()
            .unwrap_or((boot::INTR_GEOMETRY, boot::INTR_GEOMETRY.subtree_mask() & !table.nonstall_subtrees()));
        cprintln!(118, 185, 0, "  GSP-RM interrupt table: {} entries; tree {} leaves, subtrees {:#x} armed",
            table.entries.len(), geometry.leaves, armed);
        let map: alloc::vec::Vec<_> = (0..table.subtrees.len())
            .filter(|&c| table.category_mask(c) != 0)
            .map(|c| alloc::format!("{}:{:#x}", c, table.category_mask(c)))
            .collect();
        cprintln!(120, 140, 140, "    subtrees by category: {}", map.join(" "));
        cprintln!(120, 140, 140, "    engine        stall  leaf/bit   nonstall  pmc mask");
        for e in &table.entries {
            let name = intr::engine_name(e.engine_idx);
            let where_ = match geometry.split(e.vector_stall) {
                Some((leaf, bit)) => alloc::format!("{}/{:<2}", leaf, bit),
                // Not an error: an engine with no stalling interrupt leaves
                // the field alone, past the leaves the tree has
                None => alloc::string::String::from("-"),
            };
            let (cr, cg, cb) = if name.is_empty() { (170, 180, 180) } else { (230, 240, 240) };
            cprintln!(cr, cg, cb, "    {:<5}{:<8} {:>5}  {:<9} {:>8}  {:#010x}",
                e.engine_idx,
                if name.is_empty() { "" } else { name },
                e.vector_stall,
                where_,
                e.vector_nonstall,
                e.pmc_intr_mask);
        }
        // The two this driver uses, named, since the table above is long
        for (idx, what) in [
            (intr::MC_ENGINE_IDX_GSP, "GSP messages"),
            (intr::MC_ENGINE_IDX_DISP, "display / vblank"),
            (intr::MC_ENGINE_IDX_DISP_LOW, "display, low priority"),
        ] {
            match table.stall_vector(idx, geometry) {
                Some(v) => {
                    // `stall_vector` only answers with a vector the tree can
                    // split
                    let (leaf, bit) = geometry.split(v).unwrap_or((0, 0));
                    cprintln!(100, 220, 150, "  {} -> vector {} (leaf {} bit {})", what, v, leaf, bit);
                }
                None => cprintln!(255, 200, 80,
                    "  {} -> not in the table, or its vector is outside the {} leaves the tree has",
                    what, geometry.leaves),
            }
        }
    });
    if ran.is_none() {
        cprintln!(120, 140, 140, "  no GB206 card registered");
    }
}

/// The whole driver, for `nvidia log` with no section named: its modules, and
/// what these commands printed on the console
fn log_driver() -> alloc::vec::Vec<&'static str> {
    let mut v = crate::drivers::gpu::nvidia::LOG_MODULES.to_vec();
    v.push("commands::nvidia_cmds");
    v
}

/// `nvidia log <section>`: which modules each one is. A line belongs to the
/// module it was written in (`module_path!()`), so these are path fragments,
/// not prefixes of the text
const LOG_SECTIONS: &[(&str, &str, &[&str])] = &[
    ("boot", "bring-up: chip, firmware, FSP/FMC, GSP-RM boot, engines", &[
        "blackwell::bringup", "blackwell::boot", "blackwell::gspboot", "blackwell::fsp",
        "blackwell::fmc", "blackwell::hal_gb20x", "blackwell::init", "blackwell::engines",
        "blackwell::chips", "blackwell::device", "nvidia::chip", "gsp_common::gspfw",
        "gsp_common::bootargs", "gsp_common::sysinfo", "gsp_common::staticinfo",
        "nvidia::reset", "nvidia::pci", "nvidia::falcon", "nvidia::vbios", "nvidia::generic",
        "nvidia::turing", "gpu::nvidia$"]),
    ("gsp", "GSP-RM: transport, events, health, sequencer, unload", &[
        "gsp_common::kgsp", "gsp_common::rpc", "gsp_common::logdecode"]),
    ("fw", "GSP-RM's own log and its crash reports", &["kgsp::logs", "kgsp::crashcat"]),
    ("rpc", "every message to and from GSP-RM", &["gsp_common::rpc"]),
    ("rm", "RM objects and controls, with their status", &["gsp_common::rmapi"]),
    ("disp", "display: outputs, modeset, DP, EDID, IMP, scanout", &[
        "blackwell::display", "blackwell::modeset", "blackwell::control", "blackwell::window",
        "blackwell::apply", "nvidia::kms", "gpu::kms",
        "gsp_common::modeset", "gsp_common::disp", "gsp_common::output", "gsp_common::refstate",
        "gsp_common::pushbuf", "nvidia::scanout", "nvidia::surface", "gpu::edid"]),
    ("mem", "video memory, page tables, BAR1, PRAMIN", &[
        "gsp_common::vram", "nvidia::gmmu", "nvidia::vaspace", "blackwell::bar1", "nvidia::pramin",
        "blackwell::memsys", "blackwell::chanmem", "nvidia::fb"]),
    ("chan", "channels: copy, compute, blur, GPU programs", &[
        "blackwell::channel", "blackwell::compute_chan", "gsp_common::compute", "gsp_common::chan",
        "gsp_common::qmd", "nvidia::blur", "nvidia::cubin", "nvidia::shaders"]),
    ("intr", "interrupts and the event thread", &[
        "blackwell::interrupts", "nvidia::intr", "gsp_common::intr", "kgsp::service",
        "kgsp::worker", "nvidia::msi"]),
    ("perf", "P-states and clocks: the shared data page, its polling, locks", &["gsp_common::perf"]),
];

/// The driver's log from the kernel log (`dev/klog.md`): every line it wrote
/// this boot at its level, every RPC and RM call at DEBUG, GSP-RM's own
/// records, with no serial cable or pstore
fn cmd_nvidia_log(args: &str) {
    use crate::kcore::klog::view;
    let mut picked: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
    let parsed = view::parse(args, &mut |w| match LOG_SECTIONS.iter().position(|(n, _, _)| *n == w) {
        Some(i) => { picked.push(i); true }
        None => false,
    });
    let mut o = match parsed {
        Ok(o) => o,
        Err(e) => {
            cprintln!(255, 80, 80, "  nvidia log: {}", e);
            return;
        }
    };
    if o.action == view::Action::Help {
        cprintln!(57, 197, 187, "  nvidia log [section...] [options] - the driver's log, kept in memory since boot");
        for (name, what, _) in LOG_SECTIONS {
            cprintln!(128, 222, 217, "  {:<6} {}", name, what);
        }
        view::run(&o, "nvidia log");
        cprintln!(120, 140, 140, "  e.g. 'nvidia log err', 'nvidia log rm debug -n 50', 'nvidia log disp -f',");
        cprintln!(120, 140, 140, "       'nvidia log mark', change something, 'nvidia log new'");
        return;
    }
    // A module the person named is taken as meant; otherwise the sections,
    // otherwise the whole driver
    if o.filter.modules.is_empty() {
        let frags: alloc::vec::Vec<&str> = if picked.is_empty() {
            log_driver()
        } else {
            picked.iter().flat_map(|&i| LOG_SECTIONS[i].2.iter().copied()).collect()
        };
        o.filter.modules = frags.into_iter().map(alloc::string::String::from).collect();
    }
    view::run(&o, "nvidia log");
    if o.action == view::Action::Stat {
        cmd_nvidia_log_sections();
    }
}

/// Errors and warnings per section, and what GSP-RM's own log has said
fn cmd_nvidia_log_sections() {
    use crate::kcore::klog::{view, Level};
    let f = view::Filter {
        least: Level::Warn,
        modules: log_driver().into_iter().map(alloc::string::String::from).collect(),
        ..Default::default()
    };
    let bad = view::collect(&f);
    cprintln!(57, 197, 187, "  errors / warnings kept, by section:");
    for (name, _, frags) in LOG_SECTIONS {
        let mine = bad.iter().filter(|l| frags.iter().any(|m| view::module_matches(l.module, m)));
        let (e, w) = mine.fold((0, 0), |(e, w), l| if l.level == Level::Error { (e + 1, w) } else { (e, w + 1) });
        if e + w > 0 {
            let g = if e > 0 { 90 } else { 200 };
            cprintln!(255, g, 80, "    {:<6} {} error(s), {} warning(s)", name, e, w);
        }
    }
    let s = crate::drivers::gpu::nvidia::gsp_common::kgsp::logs::summary();
    if s.installed || s.info + s.notice + s.warning + s.error > 0 {
        cprintln!(128, 222, 217, "  GSP-RM's own log: {} info, {} notice, {} warning, {} error; {} pass(es) overflowed",
            s.info, s.notice, s.warning, s.error, s.overflows);
    }
}

/// BAR1 in physical mode (`dev/gpu-memory-compute.md`, 3.1): `probe` binds it,
/// checks both directions against PRAMIN on a scratch block and binds the old
/// value back; `on` keeps it and the console writes through it; `off` puts
/// GSP-RM's binding back
fn cmd_nvidia_bar1(args: &str) {
    use crate::drivers::gpu::nvidia::blackwell::bar1;
    let verb = args.split_whitespace().next().unwrap_or("probe");
    let Some(bar0) = crate::drivers::gpu::nvidia::with_gb20x(|d| d.bar0) else {
        cprintln!(255, 80, 80, "  no GB20x in this machine");
        return;
    };
    if !crate::drivers::gpu::nvidia::blackwell::boot::is_booted() {
        cprintln!(255, 200, 80, "  GSP-RM is not booted - BAR1 is still the firmware's, physical");
        return;
    }
    match verb {
        "probe" => {
            let p = bar1::probe(&bar0);
            for l in &p.lines {
                cprintln!(230, 240, 240, "  {}", l);
            }
            if p.ok {
                cprintln!(100, 220, 150, "  BAR1 answers in physical mode - 'nvidia bar1 on' keeps it");
            } else {
                cprintln!(255, 200, 80, "  BAR1 does not answer in physical mode on this boot");
            }
        }
        "on" => match bar1::on(&bar0) {
            Ok(a) => cprintln!(100, 220, 150,
                "  BAR1 physical: {} MiB of video memory at {:#x}; the console writes through it",
                a.size >> 20, a.phys),
            Err(e) => cprintln!(255, 80, 80, "  {}", e),
        },
        "off" => match bar1::off(&bar0) {
            Ok(()) => cprintln!(100, 220, 150, "  BAR1 bound back as GSP-RM left it"),
            Err(e) => cprintln!(255, 80, 80, "  {}", e),
        },
        _ => cprintln!(255, 200, 80, "  usage: nvidia bar1 probe|on|off"),
    }
}

/// The `R` block, and the diff against the shipping driver's armed state
/// (`dev/display-nvkms.md`, decision 5)
fn cmd_nvidia_gb206_diff() {
    let say = |l: &str| cprintln!(230, 240, 240, "  {}", l);
    if let Err(e) = crate::drivers::gpu::nvidia::blackwell::boot::settle_report(&say) {
        cprintln!(255, 80, 80, "  {}", e);
    }
}

/// Dumps the GSP-RM log buffers.
///
/// With the logging ELF installed this is GSP-RM's real log, with file and
/// line: the buffers hold u64 token records whose metadata pointers resolve
/// inside gsp_log_*.bin. Without it, falls back to scraping ASCII, which shows
/// only string literals passed by pointer.
///
/// 'args' may carry "all" to lift the per-buffer line cap, "raw" to force the
/// ASCII fallback.
fn cmd_nvidia_gb206_log(args: &str) {
    use crate::drivers::gpu::nvidia::gsp_common::logdecode::{
        decode_buffer, level_name, DecodeStatus, LogElf,
    };
    use crate::drivers::gpu::nvidia::blackwell::boot;
    use crate::drivers::gpu::nvidia::gsp_common::kgsp::logs;

    // What the live decode has seen: it outlives the ring wrapping, and the
    // unload, so it says what the last GSP-RM complained about too
    let seen = logs::summary();
    if seen.installed || seen.warning + seen.error > 0 {
        cprintln!(118, 185, 0,
            "  since GSP-RM boot: {} info, {} notice, {} warning, {} error{}",
            seen.info, seen.notice, seen.warning, seen.error,
            if seen.overflows > 0 { alloc::format!(", {} pass(es) overflowed", seen.overflows) }
            else { alloc::string::String::new() });
        for line in &seen.kept {
            cprintln!(255, 200, 80, "    {}", line);
        }
    }

    if !boot::is_booted() {
        cprintln!(255, 200, 80, "  no GSP boot state; run nvidia gb206-boot first");
        return;
    }
    let Some(log_elf) = crate::drivers::gpu::nvidia::with_gb20x(|d| d.gb.gsp_log) else {
        cprintln!(255, 200, 80, "  no GB20x card registered");
        return;
    };
    let want_all = args.split_whitespace().any(|a| a == "all" || a == "-a");
    let force_raw = args.split_whitespace().any(|a| a == "raw");
    let limit = if want_all { 4096 } else { 64 };

    if !force_raw {
        match crate::kcore::firmware::load::request(log_elf) {
            Ok(fw) => {
                match LogElf::parse(fw.bytes()) {
                    Some(elf) => {
                        cprintln!(118, 185, 0, "  decoding with {} ({} KiB)",
                            log_elf, fw.len() / 1024);
                        boot::with_rm_state(|st| {
                            for (name, buf) in [
                                ("INIT", st.bootargs.loginit_slice()),
                                ("INTR", st.bootargs.logintr_slice()),
                                ("RM", st.bootargs.logrm_slice()),
                            ] {
                                let (records, status, _put) = decode_buffer(&elf, buf, 0, limit);
                                match status {
                                    DecodeStatus::Empty => cprintln!(120, 140, 140,
                                        "  {}: empty (GSP-RM never logged here)", name),
                                    DecodeStatus::Unusable => cprintln!(255, 200, 80,
                                        "  {}: put pointer set but no record resolved - \
                                         wrong gsp_log build for this firmware?", name),
                                    DecodeStatus::Ok => {
                                        cprintln!(118, 185, 0, "  {} ({} records)", name, records.len());
                                        for r in &records {
                                            let (cr, cg, cb) = match r.level {
                                                4 => (255, 80, 80),
                                                3 => (255, 200, 80),
                                                _ => (230, 240, 240),
                                            };
                                            cprintln!(cr, cg, cb, "    {}({}) [{}] {}",
                                                r.file, r.line, level_name(r.level), r.text);
                                        }
                                        if records.len() == limit && !want_all {
                                            cprintln!(120, 140, 140,
                                                "    ... capped at {}; 'nvidia gb206-log all' for everything",
                                                limit);
                                        }
                                    }
                                }
                            }
                        });
                        return;
                    }
                    None => cprintln!(255, 200, 80,
                        "  {} is not a 64-bit ELF; falling back to ASCII", log_elf),
                }
            }
            Err(crate::kcore::firmware::load::FwError::NotFound) => {
                cprintln!(255, 200, 80, "  no logging ELF - showing raw ASCII only");
                cprintln!(120, 140, 140,
                    "  install {} to get GSP-RM's real log (file, line, assert text)", log_elf);
                cprintln!(120, 140, 140,
                    "  it is not in linux-firmware; extract firmware/gsp_log_ga10x.bin from");
                cprintln!(120, 140, 140,
                    "  NVIDIA-Linux-x86_64-{}.run (sh ...run -x) - the version must match exactly",
                    crate::drivers::gpu::nvidia::gsp_common::GSP_RM_VERSION);
            }
            Err(e) => cprintln!(255, 200, 80, "  reading {} failed ({:?})", log_elf, e),
        }
    }

    // Fallback: the buffers are token streams, but string literals passed as
    // %s arguments sit in them as plain ASCII. Pull out printable runs
    boot::with_rm_state(|st| {
        for (name, buf) in [
            ("LOGINIT", st.bootargs.loginit_slice()),
            ("LOGINTR", st.bootargs.logintr_slice()),
            ("LOGRM", st.bootargs.logrm_slice()),
        ] {
            let used = buf.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
            cprintln!(118, 185, 0, "  {} ({} of {} bytes written)", name, used, buf.len());
            let mut shown = 0;
            let mut run: alloc::string::String = alloc::string::String::new();
            for &b in &buf[..used] {
                if (0x20..0x7f).contains(&b) {
                    run.push(b as char);
                } else {
                    if run.len() >= 6 {
                        cprintln!(230, 240, 240, "    {}", run);
                        shown += 1;
                        if shown >= 30 { break; }
                    }
                    run.clear();
                }
            }
            if run.len() >= 6 && shown < 30 {
                cprintln!(230, 240, 240, "    {}", run);
                shown += 1;
            }
            if shown == 0 && used > 0 {
                // No text: show the head words so the put-pointer and first
                // tokens are visible
                let mut hex = alloc::string::String::new();
                for &b in &buf[..used.min(32)] {
                    hex.push_str(&alloc::format!("{:02x} ", b));
                }
                cprintln!(120, 140, 140, "    (no ASCII runs; head: {})", hex);
            }
            if shown >= 30 {
                cprintln!(120, 140, 140, "    ... (truncated at 30 lines)");
            }
        }
    });
}

/// Turns a GSP-RM image offset into a function name.
///
/// The ASSERT records GSP-RM posts through NOCAT carry only an offset
/// (errorCode low 32 bits), and the logging ELF has the symbol table for the
/// same build
fn cmd_nvidia_gb206_sym(args: &str) {
    use crate::drivers::gpu::nvidia::blackwell::chips;
    use crate::drivers::gpu::nvidia::gsp_common::logdecode::LogElf;

    // The card's own, or with no card the first GB20x whose file is there
    let log_elf = crate::drivers::gpu::nvidia::with_gb20x(|d| d.gb.gsp_log).unwrap_or_else(|| {
        chips::CHIPS.iter()
            .map(|c| c.gsp_log)
            .find(|p| crate::kcore::firmware::load::size_of(p).is_some())
            .unwrap_or(chips::CHIPS[0].gsp_log)
    });

    let arg = args
        .split_whitespace()
        .find(|a| *a != "gb206-sym" && *a != "sym")
        .unwrap_or("");
    let trimmed = arg.trim_start_matches("0x");
    let addr = match u64::from_str_radix(trimmed, 16) {
        Ok(v) => v,
        Err(_) => {
            cprintln!(255, 200, 80, "  Usage: nvidia gb206-sym <hex address>");
            cprintln!(120, 140, 140, "    e.g. the assert offset from a NOCAT record: 132d3b2");
            return;
        }
    };
    let fw = match crate::kcore::firmware::load::request(log_elf) {
        Ok(f) => f,
        Err(e) => {
            cprintln!(255, 200, 80, "  need {} ({:?})", log_elf, e);
            return;
        }
    };
    let Some(elf) = LogElf::parse(fw.bytes()) else {
        cprintln!(255, 80, 80, "  {} is not a 64-bit ELF", log_elf);
        return;
    };
    match elf.symbolize(addr) {
        Some((name, base)) => cprintln!(100, 220, 150,
            "  {:#x} = {}+{:#x}", addr, name, addr - base),
        None => cprintln!(255, 200, 80,
            "  {:#x}: no symbol covers it (stripped ELF, or wrong build)", addr),
    }
}

/// Draws into the surface the display engine is scanning right now.
///
/// The point is that the picture changes without anything being re-armed: the
/// window re-fetches its surface every frame, so a write plus a flush is a
/// whole update, with no flip unless the surface address changes
fn cmd_nvidia_gb206_draw(args: &str) {
    use crate::drivers::gpu::nvidia::blackwell::boot;
    use crate::drivers::gpu::nvidia::scanout::{rgb, Surface};

    let Some(live) = boot::live_scanout() else {
        cprintln!(255, 200, 80, "  nothing is being scanned out");
        cprintln!(120, 140, 140, "  run 'nvidia gb206-boot' first; it arms the window");
        return;
    };
    let surface = Surface {
        vram: live.vram,
        pitch: live.pitch,
        width: live.width,
        height: live.height,
    };

    let mut it = args.split_whitespace();
    let what = it.next().unwrap_or("test");
    // Colour as RRGGBB: six hex digits are easier to type blind on a serial
    // line
    let color = it
        .next()
        .and_then(|s| u32::from_str_radix(s.trim_start_matches('#'), 16).ok())
        .map(|v| 0xff00_0000 | (v & 0x00ff_ffff));

    let t0 = crate::kcore::time::rdtsc();
    let ran = crate::drivers::gpu::nvidia::with_gb20x(|d| match what {
        "fill" => {
            surface.fill(&d.bar0, color.unwrap_or(rgb(0, 120, 180)));
            surface.flush(&d.bar0)
        }
        "clear" => {
            surface.fill(&d.bar0, rgb(0, 0, 0));
            surface.flush(&d.bar0)
        }
        "text" => {
            // Everything after the subcommand, colour and all, so a sentence
            // does not have to be quoted
            let msg = args.strip_prefix("text").unwrap_or("").trim();
            let msg = if msg.is_empty() { "MikuOS on GB206" } else { msg };
            surface.fill_rect(&d.bar0, 0, 0, surface.width, 20, rgb(0, 0, 0));
            surface.text(&d.bar0, 8, 2, msg, rgb(118, 185, 0));
            surface.flush(&d.bar0)
        }
        _ => {
            surface.draw_test_pattern(&d.bar0);
            surface.flush(&d.bar0)
        }
    });
    let khz = crate::kcore::time::tsc_khz().max(1);
    let ms = crate::kcore::time::rdtsc().wrapping_sub(t0) / khz;

    match ran {
        Some(flushed) => {
            cprintln!(100, 220, 150,
                "  drew '{}' into {}x{} at vram {:#x} in {} ms{}",
                what, surface.width, surface.height, surface.vram, ms,
                if flushed { "" } else { " (FLUSH TIMED OUT)" });
            cprintln!(120, 140, 140,
                "  every pixel went through the 1 MiB PRAMIN window, one MMIO write each");
        }
        None => cprintln!(255, 80, 80, "  no RTX 5060 Ti found"),
    }
}

/// Proves BAR0/PRAMIN reaches the screen.
///
/// Run once before `nvidia gb206-boot` and once after. Before, both consoles
/// work and the band appears. After, the normal console is silent because
/// GSP-RM remapped BAR1; if the band still updates, PRAMIN is unaffected by
/// that remap and the boot log has a way out
fn cmd_nvidia_pramin_test() {
    use crate::drivers::gpu::nvidia::pramin::VramConsole;

    let ran = crate::drivers::gpu::nvidia::with_gb20x(|d| {
        match VramConsole::locate(&d.pci, &d.bar0) {
            Some(mut vc) => {
                cprintln!(118, 185, 0, "  writing into VRAM through BAR0/PRAMIN");
                cprintln!(120, 140, 140, "  look at the bottom of the screen, not here");
                vc.probe(&d.bar0);
                vc.line(&d.bar0, "PRAMIN reaches the screen - this text came through BAR0");
                vc.line(&d.bar0, "run this again after gb206-boot; if it still updates, BAR1 is bypassed");
            }
            None => {
                cprintln!(255, 200, 80, "  could not locate the scanout surface in VRAM");
                // The search failing proves nothing alone: shows whether the
                // window reads VRAM at all, on screen rather than serial
                let p = crate::drivers::gpu::nvidia::pramin::probe_vram(&d.bar0);
                cprintln!(118, 185, 0, "  PRAMIN read check ({} MiB VRAM):", p.vram_size_mb);
                for (off, v) in p.dumps.iter() {
                    cprintln!(230, 240, 240, "    {:#012x}: {:08x} {:08x} {:08x} {:08x}",
                        off, v[0], v[1], v[2], v[3]);
                }
                match p.vbios_at {
                    Some(a) => cprintln!(100, 220, 150,
                        "    VBIOS signature found at {:#x} - the read path works", a),
                    None => cprintln!(255, 80, 80,
                        "    no VBIOS signature in 64 MiB - the window is not reading VRAM"),
                }
                cprintln!(120, 140, 140,
                    "  a signature was planted through BAR1 and not found anywhere in VRAM;");
                cprintln!(120, 140, 140,
                    "  BAR1 writes are landing somewhere PRAMIN cannot see - the surface");
                cprintln!(120, 140, 140,
                    "  address has to come from the display registers instead");
            }
        }
    });
    if ran.is_none() {
        cprintln!(255, 80, 80, "  no RTX 5060 / 5060 Ti (GB206) found");
    }
}

/// Manual control of the framebuffer console.
///
/// 'nvidia gb206-boot noscreen' parks the console and leaves it parked, because
/// re-attaching after GSP-RM remapped BAR1 wedges the machine. This is the way
/// to try anyway
fn cmd_nvidia_screen(args: &str) {
    let on = args.split_whitespace().any(|a| a == "on" || a == "attach");
    let off = args.split_whitespace().any(|a| a == "off" || a == "detach");
    if on {
        crate::serial_println!("[gb206] re-attaching framebuffer console on request");
        if crate::io::console::attach_framebuffer() {
            cprintln!(100, 220, 150, "  console re-attached");
        } else {
            cprintln!(255, 200, 80, "  nothing was parked");
        }
    } else if off {
        if crate::io::console::detach_framebuffer() {
            crate::serial_println!("[gb206] framebuffer console parked on request");
        } else {
            cprintln!(255, 200, 80, "  no console to park");
        }
    } else {
        cprintln!(120, 140, 140, "  Usage: nvidia screen on|off   (currently {})",
            if crate::io::console::is_framebuffer_attached() { "attached" } else { "parked" });
    }
}

/// Read-only dump of every scratch window carrying boot state on Blackwell.
///
/// OpenRM's Hopper+ path reduces "is GFW boot ok" to "did FSP report secure
/// boot" (kgspWaitForGfwBootOk_GH100 -> kfspWaitForSecureBoot), and NVIDIA
/// ships no dev_gc6_island.h newer than Ampere, so the only published address
/// for GFW_BOOT_PROGRESS is the Turing one,
/// NV_PGC6_AON_SECURE_SCRATCH_GROUP_05(0) = 0x118234. On GB206 that reads
/// 0x00000000 while GROUP_42 (0x1183a4) in the same window returns a correct FB
/// size: the window is live but the progress byte is not where the Turing
/// header says. GSP-RM asserts on that register during its init. Dumping the
/// whole 4 KiB PGC6 aperture looks for it
fn cmd_nvidia_gb206_scratch(args: &str) {
    use crate::drivers::gpu::nvidia::blackwell::regs;

    // The 4 KiB hex dump is off by default: every word outside the few live
    // scratch groups is a PRI error or a floating 0xffffffff. The summary below
    // carries the information
    let full = args.split_whitespace().any(|a| a == "full" || a == "-v");

    let shown = crate::drivers::gpu::nvidia::with_gb20x(|d| {
        cprintln!(118, 185, 0, "  GB206 boot scratch");

        let plm = d.bar0.read32(regs::PGC6_AON_SECURE_SCRATCH_GROUP_05_PLM);
        cprintln!(230, 240, 240,
            "    gfw {:#010x}  plm {:#010x} (L0 read {})  fb {} MiB",
            d.bar0.read32(regs::PGC6_AON_SECURE_SCRATCH_GROUP_05_0),
            plm, if plm & 1 != 0 { "on" } else { "off" },
            d.bar0.read32(regs::FB_USABLE_SIZE_MB));
        // GROUP_05(1..3): the neighbours of the progress word. On GB206 the
        // group goes PRI-blocked partway through, and where it stops shows how
        // much of the Turing layout survives
        cprintln!(230, 240, 240,
            "    grp05(1..3) {:#010x} {:#010x} {:#010x}",
            d.bar0.read32(regs::PGC6_AON_SECURE_SCRATCH_GROUP_05_0 + 4),
            d.bar0.read32(regs::PGC6_AON_SECURE_SCRATCH_GROUP_05_0 + 8),
            d.bar0.read32(regs::PGC6_AON_SECURE_SCRATCH_GROUP_05_0 + 12));

        // Whole NV_PGC6 aperture, 8 words per line, zero runs collapsed.
        // Any word whose low byte is 0xff is a candidate for the real
        // progress register, so flag those explicitly
        if full {
            cprintln!(118, 185, 0, "    NV_PGC6 {:#x}..{:#x}",
                regs::PGC6_BASE, regs::PGC6_BASE + regs::PGC6_SIZE);
        }
        let mut skipped = 0u32;
        let mut pri_err = 0u32;
        let mut candidates: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
        let mut off = 0u32;
        while off < regs::PGC6_SIZE {
            let mut w = [0u32; 8];
            let mut any = false;
            for (i, v) in w.iter_mut().enumerate() {
                *v = d.bar0.read32(regs::PGC6_BASE + off + (i as u32) * 4);
                if *v != 0 {
                    any = true;
                }
                // A 0xbadfxxxx read-back is a PRI error, not data
                // (NV_PPRIV_SYS_PRI_ERROR_CODE_*: the register is priv-level
                // blocked, floorswept or its block is in reset), and an
                // all-ones word is an unimplemented address floating high.
                // Neither is a candidate for a progress byte
                if *v >> 16 == 0xBADF {
                    pri_err += 1;
                } else if *v != u32::MAX && *v & 0xFF == regs::GFW_BOOT_PROGRESS_COMPLETED {
                    candidates.push(regs::PGC6_BASE + off + (i as u32) * 4);
                }
            }
            if !any {
                skipped += 1;
            } else if full {
                if skipped > 0 {
                    cprintln!(120, 140, 140, "      ... {} zero lines", skipped);
                    skipped = 0;
                }
                cprintln!(230, 240, 240,
                    "      {:#08x}: {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x}",
                    regs::PGC6_BASE + off,
                    w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]);
            }
            off += 32;
        }
        if full && skipped > 0 {
            cprintln!(120, 140, 140, "      ... {} zero lines", skipped);
        }
        if candidates.is_empty() {
            cprintln!(230, 240, 240,
                "    pgc6: {} pri-err words, no 0xff candidate", pri_err);
        } else {
            let mut s = alloc::string::String::new();
            for a in candidates.iter().take(8) {
                s.push_str(&alloc::format!("{:#x} ", a));
            }
            cprintln!(100, 220, 150,
                "    pgc6: {} pri-err, gfw candidates: {}", pri_err, s);
        }

        // FSP: secure-boot scratch, ucode version, debug scratch group
        let ver = d.bar0.read32(regs::GFW_FSP_UCODE_VERSION);
        cprintln!(230, 240, 240,
            "    fsp boot {:#04x}  ucode v{}.{}",
            d.bar0.read32(regs::THERM_I2CS_SCRATCH) & 0xFF,
            (ver >> 8) & 0xF, ver & 0xFF);
        cprintln!(230, 240, 240,
            "    fsp scratch {:#x} {:#x} {:#x} {:#x}",
            d.bar0.read32(regs::PFSP_COMMON_SCRATCH_GROUP_2_0),
            d.bar0.read32(regs::PFSP_COMMON_SCRATCH_GROUP_2_0 + 4),
            d.bar0.read32(regs::PFSP_COMMON_SCRATCH_GROUP_2_0 + 8),
            d.bar0.read32(regs::PFSP_COMMON_SCRATCH_GROUP_2_0 + 12));

        // GSP-FMC error channel + falcon state. HWCFG2/CPUCTL/MAILBOX read as 0
        // / 0xbadf41xx whenever the BAR0 decoupler is in place, which shows
        // whether the FMC has handed over yet
        let raw = d.bar0.read32(regs::PBUS_SW_SCRATCH_GSP_FMC_ERROR);
        match regs::decode_fmc_error(raw) {
            None => cprintln!(230, 240, 240, "    fmc-err none"),
            Some(regs::FmcErrorCode::Sk { partition, error, phase }) => cprintln!(255, 80, 80,
                "    fmc-err {:#010x} SK part {:#x} err {:#x} phase {:#x}",
                raw, partition, error, phase),
            Some(regs::FmcErrorCode::Generic { partition, code, info }) => cprintln!(255, 80, 80,
                "    fmc-err {:#010x} part {:#x} code {:#x} info {:#x}",
                raw, partition, code, info),
            Some(regs::FmcErrorCode::Unknown(v)) => cprintln!(255, 80, 80,
                "    fmc-err {:#010x} (unknown variant)", v),
        }
        let hwcfg2 = d.bar0.read32(regs::PGSP_FALCON_BASE + regs::FALCON_HWCFG2);
        let cpuctl = d.bar0.read32(regs::PRISCV_RISCV_CPUCTL);
        let locked = hwcfg2 == 0
            || crate::drivers::gpu::nvidia::pri::is_target_locked(hwcfg2);
        cprintln!(230, 240, 240,
            "    gsp hwcfg2 {:#x} ({})  cpuctl {:#x}  mbox {:#x}/{:#x}",
            hwcfg2,
            if locked { "target-masked" }
            else if hwcfg2 & regs::HWCFG2_RISCV_BR_PRIV_LOCKDOWN != 0 { "lockdown" }
            else { "released" },
            cpuctl,
            d.bar0.read32(regs::PGSP_FALCON_BASE + regs::FALCON_MAILBOX0),
            d.bar0.read32(regs::PGSP_FALCON_BASE + regs::FALCON_MAILBOX1));
    });
    if shown.is_none() {
        cprintln!(255, 80, 80, "  no RTX 5060 / 5060 Ti (GB206) found");
    }
}

fn cmd_nvidia_cot_dryrun() {
    use crate::drivers::gpu::nvidia::blackwell::{chips, fmc};

    // Struct layout self-test always runs, card or not
    fmc::self_test();
    cprintln!(100, 220, 150, "  NVDM_PAYLOAD_COT layout self-test passed (860 bytes, v2)");

    // If a real FMC blob is on disk, parses and verifies it too: the card's
    // own, or with no card the first GB20x whose file is there (one image for
    // the whole family)
    let path = crate::drivers::gpu::nvidia::with_gb20x(|d| d.gb.fmc).unwrap_or_else(|| {
        chips::CHIPS.iter()
            .map(|c| c.fmc)
            .find(|p| crate::kcore::firmware::load::size_of(p).is_some())
            .unwrap_or(chips::CHIPS[0].fmc)
    });
    match crate::kcore::firmware::load::request(path) {
        Ok(blob) => match fmc::FmcImage::parse(blob.bytes()) {
            Ok(img) => {
                img.log();
                match img.verify_gb20x() {
                    Ok(()) => cprintln!(100, 220, 150,
                        "  FMC blob ok: image={} KB, crypto sizes match GB20x COT v2 (48/97/96)",
                        img.image.len() / 1024),
                    Err(_) => cprintln!(255, 80, 80, "  FMC crypto sizes do NOT match GB20x; wrong blob?"),
                }
            }
            Err(e) => cprintln!(255, 80, 80, "  FMC ELF parse failed: {:?}", e),
        },
        Err(_) => cprintln!(120, 140, 140,
            "  FMC blob not on disk ({}); dry run used synthetic data only", path),
    }
}

// nvidia list: enumerates every NVIDIA GPU the driver recognized, across all
// families: the GB20x (RTX 5060 / 5060 Ti) the full driver runs, the Turing
// card that gets the TU116 bring-up, and any other card, brought up host-side
// only and listed from the generic registry
fn cmd_nvidia_list() {
    use crate::drivers::gpu::nvidia::{blackwell, generic, with_gb20x, with_gtx1650};

    cprintln!(118, 185, 0, "  recognized NVIDIA GPUs");

    let mut total = 0usize;

    // The card this machine drives, in neither of the two registries below
    let gb = with_gb20x(|d| {
        crate::println!(
            "    [{:04x}:{:04x}] {} ({}) - full driver (GSP-RM 615)",
            d.pci.vendor, d.pci.device, blackwell::model_name(d.pci.device), d.chip.codename()
        );
    });
    if gb.is_some() { total += 1; }

    let gtx = with_gtx1650(|dev| {
        crate::println!(
            "    [{:04x}:{:04x}] {} ({}) - Turing bring-up (GSP-RM boot on request)",
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
    use crate::drivers::gpu::nvidia::{turing, with_gb20x, with_gtx1650};

    // A GB20x has its own summary, which `nvidia` with no verb shows on that
    // machine
    if with_gtx1650(|_| ()).is_none() && with_gb20x(|_| ()).is_some() {
        cmd_nvidia_gb206();
        return;
    }

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
            "    chip:        arch={:?} impl={:#x} rev={}.{} boot42={:#010x}",
            dev.chip.arch, dev.chip.implementation,
            dev.chip.major_rev, dev.chip.minor_rev,
            dev.boot42
        );
        crate::println!(
            "    bar0:        phys={:#x} size={} KB",
            dev.bar0.virt_base() - crate::arch::x86_64::grub::hhdm(), dev.bar0.size() / 1024
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

        use crate::drivers::gpu::nvidia::pmc;
        let enable = pmc::read_enable(&dev.bar0);
        let straps = pmc::read_straps(&dev.bar0);
        crate::println!("    pmc_enable:  {:#010x}", enable);
        crate::println!("    pmc_boot_2:  {:#010x}  (straps)", straps);

        let freq = pmc::read_ptimer_freq(&dev.bar0);
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
        crate::println!("  (`nvidia list` shows what PCI found; the full driver is for the GB20x)");
    }
}

// nvidia debug: BAR0 register dump
//
// Dumps every register the host can read without engaging an engine. On QEMU,
// with the card not bound, 'info' prints "no supported NVIDIA GPU" and this
// prints nothing; on real hardware it gives raw values to compare against
// nouveau's expected ranges
fn cmd_nvidia_debug() {
    use crate::drivers::gpu::nvidia::{turing, with_gtx1650};
    use crate::drivers::gpu::nvidia::turing::regs::{
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

        let _ = crate::drivers::gpu::nvidia::pmc::read_straps(&dev.bar0); // touch to keep import live
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound (expected on QEMU without VFIO passthrough)");
    }
}

// nvidia firmware: inventories the embedded TU116 blob bundle
//
// Works with or without a bound card: the blobs are embedded into the kernel
// image at compile time
fn cmd_nvidia_firmware() {
    use crate::drivers::gpu::nvidia::turing::tu116_fw::{self, NvfwBinHdr, Engine as FwEngine};

    cprintln!(118, 185, 0, "  TU116 firmware bundle (on firmware store, loaded on demand)");
    crate::println!("    blobs:        {}", tu116_fw::TU116_FIRMWARE.len());
    crate::println!("    total size:   {} bytes", tu116_fw::total_size());
    crate::println!("    gsp line:     {}", tu116_fw::GSP_DEFAULT_LINE);
    crate::println!("    store:        {}", if crate::kcore::firmware::load::available() { "mounted" } else { "NOT FOUND" });
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
        let fw = match crate::kcore::firmware::load::request(b.path) {
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

// nvidia falcon: liveness probe of every Falcon engine
fn cmd_nvidia_falcon() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::falcon::{
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

// nvidia ungate: re-applies PMC_ENABLE bits for PGRAPH + CE0 from the shell.
// init.rs does this once during boot; this re-triggers it without rebooting
fn cmd_nvidia_ungate() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::pmc;

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

// nvidia pmc-scan: read-only sweep of the PMC register area (BAR0 +
// 0x000..0x1000) for plausible device-enable and power-gating registers, to
// find what gates NVDEC on TU116 without guessing an offset
//
// Heuristics:
//     skip zeros and the 0xFFFF_FFFF "no decode" pattern
//     flag values that look like bitmasks (popcount <= 8) or sentinels
//     (0xBADF_xxxx is a PRI bus reject, never a device enable)
//     annotate well-known offsets
//     collapse runs of identical reads into a single line
//
// Never writes
fn cmd_nvidia_pmc_scan() {
    use crate::drivers::gpu::nvidia::with_gtx1650;

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
            dev.bar0.read32(crate::drivers::gpu::nvidia::turing::regs::PMC_ENABLE));
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
    use crate::drivers::gpu::nvidia::falcon::is_pri_sentinel;
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
    use crate::drivers::gpu::nvidia::turing::regs::*;
    match off {
        x if x == PMC_BOOT_0       => "PMC_BOOT_0 (chip ID)",
        x if x == PMC_BOOT_1       => "PMC_BOOT_1",
        x if x == PMC_BOOT_2       => "PMC_BOOT_2 (straps)",
        x if x == PMC_INTR_0       => "PMC_INTR_0 (pending)",
        x if x == PMC_INTR_EN_0    => "PMC_INTR_EN_0",
        x if x == PMC_ENABLE       => "PMC_ENABLE (engine gates)",
        x if x == PMC_DEBUG_1      => "PMC_DEBUG_1",
        x if x == PMC_BOOT_42      => "PMC_BOOT_42 (extended chip ID)",
        // Frequently-discussed Turing power/enable offsets. Not asserted
        // correct on TU116; the scan verifies
        0x0140                     => "(?) ELPG_ENABLE candidate",
        0x0148                     => "(?) ELPG_ENABLE_1 candidate",
        0x0600                     => "(?) DEVICE_ENABLE candidate",
        0x0604                     => "(?) DEVICE_ENABLE_1 candidate",
        0x088C                     => "(?) DEVICE_ENABLE candidate (alt offset)",
        0x0C00                     => "(?) DEVICE_ENABLE candidate (alt offset)",
        _ => "",
    }
}

// nvidia dma-state: per-engine DMATRF snapshot + IDLE/ERROR decoding.
// Read-only. Confirms the Falcon DMA register window is reachable before the
// first DMA transfer through Engine::dma_load
fn cmd_nvidia_dma_state() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::falcon::{
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

// nvidia fbif-scan: read-only sweep of each live falcon's FBIF window
//
// Finds the offset of FBIF_TRANSCFG (per-context-DMA aperture descriptor) and
// FBIF_REGIONCFG (WPR region selector) on TU116. Sources (nouveau branches,
// envytools, open-gpu-kernel-modules) place FBIF differently across NV
// generations: typical Turing layouts are at engine_base + 0x600 or +0x800 with
// FBIF registers in the first 0x100 bytes
//
// Heuristics:
//     TRANSCFG entries (8 of them, 4 bytes each) read as 0 at boot if
//     no driver has programmed them, or as a bitmask with low bits set
//     (target type + access mode) if hw POST set defaults
//     REGIONCFG is a 4-bit-per-context array (32 bits, 8 contexts)
//     PRI sentinels (0xBADF_xxxx) flag dead/gated regions and bound
//     the decoded part of the FBIF window
//
// Read-only
fn cmd_nvidia_fbif_scan() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::falcon::{
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
    use crate::drivers::gpu::nvidia::falcon::is_pri_sentinel;
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

// nvidia fbif-decode: decodes all 8 TRANSCFG slots for each live engine.
// Read-only. Confirms the bit layout before it is programmed
fn cmd_nvidia_fbif_decode() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::falcon::{
        Engine, Liveness, PSEC_BASE, PGSP_BASE, PFECS_BASE,
    };
    use crate::drivers::gpu::nvidia::fbif::{
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

// nvidia dma-test: end-to-end loopback against SEC2 (alive and HALTed at boot,
// so safe to drive)
//
// Sequence:
//   1) allocate a single 4 KiB phys-contiguous page from the kernel
//   2) fill it with a 32-bit pattern that encodes the source offset
//   3) issue an sfence so the writes are visible to a DMA reader
//   4) program SEC2 TRANSCFG[7] for COHERENT_SYSMEM + physical addressing
//   5) call Engine::dma_load to move 256 B from the buffer into SEC2 DMEM
//      (offset 0), through ctxdma=7
//   6) read DMEM back through the PIO DMEM_C/DMEM_D port and compare to the
//      original buffer
//   7) report which 32-bit slots match / mismatch
//
// Failure modes recovered from cleanly:
//     DMATRFCMD_ERROR set: layout mismatch -> reported, buffer freed
//     dma_load returns Timeout: GPU hung in DMA -> diagnostic, buffer freed
//     read-back mismatch: aperture/address-translation issue -> diagnostic
fn cmd_nvidia_dma_test() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::falcon::{
        Engine, Liveness, PSEC_BASE, FALCON_DMEM_C0, FALCON_DMEM_D0,
        DmaTransfer, DmaTarget, DmaDirection, MEM_C_AINCR,
    };
    use crate::drivers::gpu::nvidia::fbif::{self, FbifTarget, FbifMemType};
    use crate::mm::dma::DmaBuffer;

    const CTX:        u8  = 7;     // last TRANSCFG slot
    const CHUNK_BYTES: u32 = 256;   // size_log2=6
    const PATTERN_BASE: u32 = 0xCAFE_0000;

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  DMA loopback test (SEC2 ctxdma={})", CTX);

        // 0. SEC2 must be alive + halted: a running falcon is never driven
        let sec2 = Engine::new(&dev.bar0, PSEC_BASE, "sec2");
        if !matches!(sec2.liveness(), Liveness::Alive) {
            crate::print_warn!("    sec2 not alive ({:?}), aborting", sec2.liveness());
            return;
        }
        if !sec2.is_halted() {
            crate::print_warn!("    sec2 not halted (cpuctl={:#x}), aborting...", sec2.read(crate::drivers::gpu::nvidia::falcon::FALCON_CPUCTL));
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

        // 4b) Set FBIF_CTL.ALLOW_PHYS_NO_CTX. Without this bit the Falcon drops
        // fetches in TRANSCFG physical mode (kick echo returns, but idle=0
        // error=0 stays). Nouveau/OpenGPU set it during initialization for
        // host-driven ucode-load
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
                use crate::drivers::gpu::nvidia::falcon::{
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

        // Always restores TRANSCFG + FBIF_CTL, so the engine is not left with a
        // sysmem aperture armed or ALLOW_PHYS_NO_CTX on
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

// nvidia imem-test: IMEM variant of dma-test. Checks that dma_load(target=Imem,
// set_imem_tag=true) moves bytes from sysmem to SEC2 IMEM, the path for
// uploading ACR HS ucode. IMEM may be PRIV-locked for readback on some SKUs;
// the bus then returns 0xBADFxxxx sentinels, reported apart from a mismatch
// because dma_load reporting OK proves the write side worked
fn cmd_nvidia_imem_test() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::falcon::{
        Engine, Liveness, PSEC_BASE, FALCON_IMEM_C0, FALCON_IMEM_D0,
        DmaTransfer, DmaTarget, DmaDirection, MEM_C_AINCR,
    };
    use crate::drivers::gpu::nvidia::fbif::{self, FbifTarget, FbifMemType};
    use crate::mm::dma::DmaBuffer;

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
                sec2.read(crate::drivers::gpu::nvidia::falcon::FALCON_CPUCTL));
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

        // Pattern 0xC0DE_xxxx, distinct from dma-test's 0xCAFE so a stale DMEM
        // fragment does not look like an IMEM-test pass
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
                use crate::drivers::gpu::nvidia::falcon::{
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

// nvidia acr-info: read-only structural dump of every SEC2 ACR blob
//
// For each blob:
//     raw size, container kind (NVFW vs raw)
//     if NVFW-wrapped: the parsed bin-hdr fields, then a 64-byte hex dump
//     starting at hdr.header_offset (the per-image HS header) and a 32-byte
//     hex dump starting at hdr.data_offset (the ucode payload prefix)
//     if raw: a 64-byte hex dump from offset 0
//
// Touches no MMIO and no GPU state. The bytes compare against
// open-gpu-kernel-modules HS-header structs
fn cmd_nvidia_acr_info() {
    use crate::drivers::gpu::nvidia::turing::tu116_fw::{self, NvfwBinHdr, Engine as FwEngine};
    use crate::drivers::gpu::nvidia::nvfw_hs::{NvfwHsHeader, NvfwHsLoadHeader};

    cprintln!(118, 185, 0, "  SEC2 ACR firmware structure");
    let mut shown = 0u32;
    for b in tu116_fw::TU116_FIRMWARE {
        if !matches!(b.engine, FwEngine::Sec2) { continue; }
        if !b.name.starts_with("acr/") { continue; }
        shown += 1;

        // Fetch from the firmware store; freed when fw drops at loop end.
        let fw = match crate::kcore::firmware::load::request(b.path) {
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

fn cmd_nvidia_wpr_state() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::turing::regs::{
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
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::turing::therm;

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

// nvidia gsp-rm: walks the GSP-RM bring-up scaffolding (VRAM size, WPR2 layout,
// ABI sizes). Stops at MissingFirmware; no GSP-RM blob is shipped
fn cmd_nvidia_gsprm() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::turing::gsprm::{self, GsprmError};

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

// nvidia gsp-rm-dryrun: self-tests the radix3 + WPR-meta path with a synthetic
// in-memory ELF (default 256 pages = 1 MiB)
fn cmd_nvidia_gsprm_dryrun() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::turing::gsprm;

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

// nvidia gsp-rm-load: parses the GSP-RM ELF (fetched from the firmware store on
// demand), stages .fwimage, builds the radix3 + WPR-meta.
fn cmd_nvidia_gsprm_load() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::turing::{gsprm, tu116_fw};
    use crate::drivers::gpu::nvidia::turing::gsprm::LoadError;

    let shown = with_gtx1650(|dev| {
        cprintln!(118, 185, 0, "  GSP-RM load (TU116, line 570.144)");
        let gsp_rm_fw = tu116_fw::gsp_rm_570();
        let blob = gsp_rm_fw.bytes();
        if blob.is_empty() {
            crate::print_warn!("    GSP-RM firmware not available on the firmware store.");
            // Re-issue the request directly so the precise failure reaches the
            // console (request_or_empty only logs it to serial, invisible on
            // real hardware). Also stat the path to separate "cannot resolve"
            // from "resolves but cannot read".
            let rel = "nvidia/tu116/gsp/gsp-570.144.bin";
            match crate::kcore::firmware::load::size_of(rel) {
                Some(sz) => crate::println!("    stat: resolves, size = {} bytes", sz),
                None     => crate::println!("    stat: path does NOT resolve on the store"),
            }
            match crate::kcore::firmware::load::request(rel) {
                Ok(fw) => crate::println!("    read: unexpectedly OK, {} bytes", fw.len()),
                Err(e) => crate::print_warn!("    read: failed with {:?}", e),
            }
            crate::println!("    Expected at /nvidia/tu116/gsp/gsp-570.144.bin on the store.");
            crate::println!("    The builder stages it from src/nvidia/turing/tu116/gsp/gsp-570.144.bin");
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

// nvidia gsp-rm-boot: hands the pinned WPR-meta to the GSP booter and kicks
// booter_load. Requires 'nvidia gsp-rm-load' first
fn cmd_nvidia_gsprm_boot() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::turing::gsprm::{self, BooterError};

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

// nvidia gsp-bootargs: builds and verifies the GSP-RM boot arguments in sysmem
//
// Allocates the libos init args, GSP_ARGUMENTS_CACHED and the CMDQ/MSGQ shared
// region as the boot pipeline does, reads every field back from sysmem, and
// checks it against the firmware ABI. No Falcon is started, so this is safe on
// a live card. All buffers are freed on exit
fn cmd_nvidia_gsp_bootargs() {
    use crate::drivers::gpu::nvidia::gsp_common::bootargs::GspBootArgs;

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
                cprintln!(100, 220, 150, "  all boot-arg structures match the {} ABI",
                    crate::drivers::gpu::nvidia::gsp_common::GSP_RM_VERSION);
            } else {
                crate::print_warn!("  one or more structures FAILED - see serial log");
            }
        }
        Err(e) => crate::print_warn!("  boot-args alloc failed: {:?}", e),
    }
}

// nvidia gsp-rm-boot-full: drives the entire GSP-RM boot pipeline end to end
//
// Runs scrubber -> load -> SEC2 ACR -> WPR2 verify -> booter_load -> MSGQ
// handshake in order, stopping at the first stage whose hardware precondition
// is unmet and reporting where and why
fn cmd_nvidia_gsprm_boot_full() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::turing::gsprm::{self, BootStage};

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
                "    GSP-RM came up: status queue version {}, {} slots",
                rep.gsp_queue_major, rep.gsp_queue_slots),
            BootStage::Booter => {
                crate::println!("    booter ran (mb1={:#010x}); libos boot args handed to GSP, FALCON_OS set,",
                    rep.booter_mb1);
                crate::println!("    but GSP-RM has not written its status queue header (see serial log for the poll result)");
            }
            _ => {}
        }
    });
    if shown.is_none() {
        crate::print_warn!("  no NVIDIA GPU bound - GSP boot only runs against real silicon");
        crate::println!("  (on QEMU without VFIO this is expected)");
    }
}

// nvidia sec2-acr: first-contact SEC2 ACR boot
//
// Stages ucode_ahesasc into sysmem, points FBIF ctx0 at it, loads acr/bl into
// SEC2 IMEM (SECURE), hands the ahesasc phys address via MAILBOX0/MAILBOX1,
// kicks CPUCTL and reports the halt status. Without the full DMEM scratch
// layout the ACR bl halts early; this shows the engine engaging the HS-signed
// upload
fn cmd_nvidia_sec2_acr() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::turing::sec2;

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

// nvidia sec2-acr-v2: SEC2 ACR boot with a real flcn_bl_dmem_desc
//
// Parses ahesasc HS layers (NVFW -> HS header -> HS load_header), builds a
// flcn_bl_dmem_desc pointing at the sysmem-staged image, uploads it to SEC2
// DMEM, then kicks bl. A correct desc moves bl past the early-exit mb1=0 path
// into DMA-ing the HS image
fn cmd_nvidia_sec2_acr_v2() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::turing::sec2;

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

// nvidia nvdec-scrub: first-contact boot of the NVDEC scrubber
fn cmd_nvidia_nvdec_scrub() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::turing::nvdec;

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

/// `nvidia gsp` is the Turing first-contact boot;
/// `nvidia gsp unload|load|reset`
/// takes a GB20x's GSP-RM down and boots it again without a reboot
/// (`dev/gsp-foundation.md`, section 14)
fn cmd_nvidia_gsp(args: &str) {
    match args {
        "" => {}
        "unload" => return cmd_nvidia_gsp_unload(),
        // The boot again, as the command doing it
        "load" => return cmd_nvidia_gb206_boot(""),
        "reset" => return cmd_nvidia_gsp_reset(),
        _ => {
            crate::println!("  usage: nvidia gsp [unload | load | reset]");
            return;
        }
    }
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::turing::gsp;

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

/// Engines down in reverse, the RM client freed, GSP-RM told and waited for,
/// WPR2 checked, and everything that named it forgotten, so `nvidia gsp load`
/// can boot it again
fn cmd_nvidia_gsp_unload() {
    use crate::drivers::gpu::nvidia::blackwell::boot;
    if !boot::is_booted() {
        cprintln!(255, 200, 80, "  GSP-RM is not loaded");
        return;
    }
    // Said before rather than after: the console is on the card's scanout,
    // and the display engine is one of the things going down
    cprintln!(255, 200, 80,
        "  unloading GSP-RM: the screen goes dark, and this command's result is on serial");
    cprintln!(120, 140, 140, "  (and in pstore after a reboot); 'nvidia gsp load' brings it back");
    let say = |s: &str| {
        crate::serial_println!("[gsp unload] {}", s);
        crate::pstore_println!("{}", s);
        crate::println!("    {}", s);
    };
    // Under the display layer's lock: no 'display' request lands halfway
    // through the display being taken apart
    let done = crate::drivers::gpu::kms::exclusive(|| {
        crate::drivers::gpu::nvidia::with_gb20x(|d| boot::unload(&d.bar0, &d.pci, &say))
    });
    match done {
        None => cprintln!(255, 80, 80, "  no GB20x card registered"),
        Some(Err(e)) => cprintln!(255, 80, 80, "  not unloaded: {}", e),
        Some(Ok(u)) if u.freed => cprintln!(100, 220, 150,
            "  GSP-RM unloaded (halted, WPR2 down) - 'nvidia gsp load' boots it again"),
        Some(Ok(u)) => cprintln!(255, 200, 80,
            "  GSP-RM unloaded, but halted {} / WPR2 hi {:#010x}: reset the GPU ('nvidia gb206-reset') before loading",
            u.gsp.halted, u.wpr2_hi),
    }
}

/// Unload, function-level reset, boot again, desktop back: what `gsprecover`
/// does on its own when GSP-RM dies (`dev/gsp-foundation.md`, 16). By hand it
/// tests that path on a healthy card
fn cmd_nvidia_gsp_reset() {
    cprintln!(255, 200, 80,
        "  resetting the GPU: the screen goes dark and comes back from cold (a few seconds)");
    cprintln!(120, 140, 140, "  the steps are in 'mikudo nvidia log boot' / serial");
    let say = |s: &str| {
        crate::kinfo!("[gsp reset] {}", s);
        crate::pstore_println!("{}", s);
    };
    match crate::drivers::gpu::nvidia::blackwell::boot::reset_and_reload(&say) {
        Ok(()) => cprintln!(100, 220, 150, "  GPU reset and GSP-RM booted again"),
        Err(e) => cprintln!(255, 80, 80, "  reset failed: {}", e),
    }
}

// nvidia next: inspects live state and prescribes the next concrete step
//
// Walks the prerequisite chain (engine alive -> WPR set up -> GSP-RM staged ->
// booter runs) and stops at the first unmet precondition, saying what to
// implement next
fn cmd_nvidia_next() {
    use crate::drivers::gpu::nvidia::with_gtx1650;
    use crate::drivers::gpu::nvidia::falcon::{Engine, PSEC_BASE, PGSP_BASE, PFECS_BASE};

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
            mark(true), crate::drivers::gpu::nvidia::turing::tu116_fw::TU116_FIRMWARE.len());
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
