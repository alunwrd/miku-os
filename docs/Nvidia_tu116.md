# NVIDIA TU116 / TU117 (Turing): the GTX 1650 / 1660 driver

The Turing driver, in `kernel/drivers/gpu/nvidia/turing/`, recognises the GeForce GTX 1650 and 1660 family (TU116 and TU117 silicon) and brings it up as far as the host registers allow: it maps BAR0, identifies the chip, reads the VBIOS, masks interrupts, ungates PGRAPH and CE0, reads PTIMER and the temperature sensor, and checks which Falcon engines are alive. On a TU116 it also stages the GSP-RM image in system memory at every boot. The GSP-RM boot itself (NVDEC scrubber, FWSEC FRTS, SEC2 booter, the GSP-RM status queue) is a shell command, `nvidia gsp-rm-boot-full`, and does not finish today: its firmware is the 570.144 line, while the boot arguments it shares with the Blackwell driver now follow 615.78.08. Turing has no display, no compute, no channels and no RM objects. The project's decision is "Turing stays as it is" ([../dev/gsp-foundation.md](../dev/gsp-foundation.md)): it gets no new code. Everything the Blackwell driver does is in [Nvidia_gb206.md](Nvidia_gb206.md); this page covers only what is specific to Turing.

## Contents

- [State in short](#state-in-short)
- [Supported cards](#supported-cards)
- [What runs at boot](#what-runs-at-boot)
- [Firmware](#firmware)
- [The GSP-RM boot pipeline](#the-gsp-rm-boot-pipeline)
- [The WPR2 layout](#the-wpr2-layout)
- [What is shared with the Blackwell driver](#what-is-shared-with-the-blackwell-driver)
- [Commands](#commands)
- [Boot flags](#boot-flags)
- [Verification status](#verification-status)
- [Code map](#code-map)
- [Known gaps](#known-gaps)

## State in short

| Area | State |
|---|---|
| PCI bind, BAR0 map, chip id, MSI/MSI-X discovery, PTOP walk, VBIOS read | Done, at every boot |
| Interrupts | All top-level interrupts masked, INTx disabled; MSI capabilities are read and logged, never programmed |
| VBIOS devinit (the init script) | Not done. The script table (BIT token `I`) and the DCB (token `D`) are found and logged, never run |
| PTIMER, temperature | Read. PTIMER's scale and the thermal limits read zero unless the card was POSTed by the system firmware |
| Falcon engines (SEC2, GSP, NVDEC, FECS, GPCCS0/1) | Liveness probed (HWCFG, CPUCTL), uploads by PIO and DMA exist |
| GSP-RM image staging (TU116 only) | Done at every boot: ELF parsed, `.fwimage` staged, radix3 built, WPR meta written |
| GSP-RM boot | On demand only (`nvidia gsp-rm-boot-full`); not expected to complete (see [the pipeline](#the-gsp-rm-boot-pipeline)) |
| RPC, RM objects, video memory, display, compute | None |

## Supported cards

`turing::matches` accepts the device ids below; `turing::model_name` names them. The chip is then decoded from `PMC_BOOT_0`: architecture 0x16 is Turing, implementation 0x8 is TU116 and 0x7 is TU117 (`chip.rs`). A Turing card with another implementation (TU102, TU104, TU106) does not match these ids and goes to the host-side-only path in `generic.rs`.

| Silicon | Device ids | Names |
|---|---|---|
| TU116 (`turing/tu116.rs`) | 0x2182, 0x2183, 0x2184, 0x2187, 0x2188, 0x2189, 0x2191, 0x2192, 0x21C4, 0x21D1 | GTX 1660 Ti, 1660, 1650 SUPER, 1650 (GDDR6 refresh, 0x2188, the module's primary target), 1660 SUPER, 1660 Ti Mobile and Max-Q, 1650 Ti Mobile |
| TU117 (`turing/tu117.rs`) | 0x1F82, 0x1F91, 0x1F92, 0x1F94, 0x1F95, 0x1F96, 0x1F97, 0x1F98, 0x1F99, 0x1FB0, 0x1FB1, 0x1FB2, 0x1FB8, 0x1FB9, 0x1FBB | GTX 1650 desktop and mobile, 1650 Ti Mobile, MX450, Quadro T400/T500/T600/T1000/T2000 Mobile |

Both chips expect a 16 MiB BAR0. A TU117 gets the host bring-up only; the firmware probe and the GSP-RM staging run for TU116 ids alone (`tu116::matches` in `init.rs`). `turing/quirks.rs` holds a per-chip record (SEC2 and GSP reset bits in `NV_PMC_ENABLE`: bit 14 and bit 13; `needs_acr_wpr2`); the boot prints it and nothing else reads it.

## What runs at boot

`nvidia::init` (the "NVIDIA GPU probe" boot step) scans PCI for NVIDIA cards, prints the `MIKU_FW=` line for each ([firmware](#firmware)), and calls `turing::init` for a matching id. `turing::init` (`turing/init.rs`) does, in order:

1. Checks BAR0 (memory, assigned, at least 16 MiB) and notes BAR1 (the framebuffer aperture) and BAR3 (USER).
2. Enables memory decoding and bus mastering, disables INTx.
3. Maps BAR0 through the HHDM.
4. Reads `PMC_BOOT_0` and `PMC_BOOT_42`, decodes the chip, warns if it is not TU116 or TU117, prints the quirks record and the model name.
5. Reads the PCI MSI and MSI-X capabilities.
6. Logs up to 16 non-zero `PTOP_DEVICE_INFO` entries.
7. Reads the VBIOS from the expansion ROM, parses its images, finds the BIT header and logs the init-script table and the DCB pointer.
8. Masks every top-level interrupt (`pmc::mask_all_interrupts`).
9. Sets `PMC_ENABLE` for GR and CE0, so FECS and GPCCS answer with a real HWCFG instead of the `0xBADF_xxxx` PRI sentinel.
10. Reads the PTIMER scale ("not programmed" when devinit has not run) and checks the counter advances.
11. Reads the on-die temperature and the slowdown and shutdown limits (`turing/therm.rs`).
12. Checks whether GRUB's framebuffer lies inside one of the card's BARs, which means the console is already being scanned out by this card.
13. TU116 only: lists every firmware file with its size and NVFW header, and probes the six Falcon engines.
14. TU116 only: stages the GSP-RM image (`gsprm::load`, below). No Falcon is started.
15. Registers the device in the Turing slot (`set_active_gtx1650`), which the `nvidia` subcommands and the boot splash read.

Every line goes to the serial port with the prefix `[turing]` or `[turing/tu116]`. A failure is logged and the device stays registered.

## Firmware

The files come from linux-firmware's `nvidia/tu116/` set (570.144 line). They are not in git. They live in `kernel/drivers/gpu/nvidia/firmware/gtx1650/`, and the builder stages that folder to `/lib/firmware/nvidia/tu116/` inside `boot/firmware.img` (the `NVIDIA_FW_CARDS` table in `builder/src/main.rs`). The driver fetches each file on demand from the firmware store and drops it after use; see [Nvidia_gb206.md, Firmware](Nvidia_gb206.md#firmware) for the store and the image. The folder is about 28 MB, nearly all of it the GSP-RM image, and GRUB reads all of `firmware.img` at every boot: on a machine without a Turing card, build with `MIKU_FW=` set to the line the kernel prints (for example `MIKU_FW=nvidia/gb206`) to leave it out ([wiki/building.md](wiki/building.md)).

`turing/tu116_fw.rs` lists the files in the order they are used:

| Folder | Files | Used by |
|---|---|---|
| `nvdec/` | `scrubber.bin` (NVFW) | Stage 1, the VRAM scrubber |
| `sec2/` | `desc.bin`, `image.bin`, `sig.bin` (raw) | Listed and probed only |
| `acr/` | `bl.bin`, `unload_bl.bin`, `ucode_ahesasc.bin`, `ucode_asb.bin`, `ucode_unload.bin` (NVFW) | `bl.bin` is the generic SEC2 bootloader the booter runs under; `ucode_ahesasc.bin` is used by the `sec2-acr` experiments; the rest are listed only |
| `gsp/` | `booter_load-570.144.bin`, `booter_unload-570.144.bin` (NVFW) | `booter_load` in stage 5; `booter_unload` listed only |
| `gsp/` | `bootloader-570.144.bin` | The GSP RISC-V bootloader, staged by `gsprm::load` |
| `gsp/` | `gsp-570.144.bin` (about 28.5 MB ELF64) | The GSP-RM image; sections `.fwimage`, `.fwversion` and `.fwsignature_tu11x` |
| `gr/` | `fecs_*`, `gpccs_*` (bl, inst, data, sig) and five `sw_*` tables (raw) | Listed only; nothing loads FECS or GPCCS |

`TU116_FIRMWARE` holds the 24 files that `nvidia firmware` lists; the GSP-RM image and the RISC-V bootloader are fetched separately, so 26 files in all. The folder also holds `booter_load-535.113.01.bin` and `booter_unload-535.113.01.bin`, which no code asks for. FWSEC is not a file: it is read from the VBIOS.

## The GSP-RM boot pipeline

`gsprm::boot` (`turing/gsprm.rs`) runs six stages and stops at the first one that fails, returning how far it got (`FullBootReport`). It follows OpenRM's `kgspBootstrap_TU102` and nouveau's `tu102`/`r535` code. Run it with `mikudo nvidia gsp-rm-boot-full`. It is not run at boot on purpose: the Falcon halt polls would add seconds to every boot while the WPR2 gate is unsolved.

| Stage | What it does | Stops when |
|---|---|---|
| 1. NVDEC scrubber | Uploads `nvdec/scrubber.bin` to NVDEC and waits for the halt (100 ms). The scrub descriptor is not modelled, so the region is passed in the mailboxes and the scrubber probably exits early | Never (a failure is logged and the boot continues) |
| 2. Stage GSP-RM | `gsprm::load`: parse the ELF, read the VRAM size from `NV_PFB_PRI_MMU_LOCAL_MEMORY_RANGE`, copy `.fwimage` into physically contiguous system memory, build the radix3 table over it, stage the bootloader and the signature, compute the WPR2 layout, write the 256-byte WPR meta block into its own page. All of it stays pinned (`GSP_RM`) | No firmware, ELF error, VRAM reads zero (`NoVram`: devinit has not run), allocation failure |
| 3. FWSEC FRTS | Reads the VBIOS (the PRAMIN shadow first, because the PCI ROM lacks the image with FWSEC; the PCI ROM as fallback), finds the FWSEC ucode (PMU table type 0x85), patches the FRTS command (DMEM mapper command 0x15) into its DMEM image with the FRTS region from the layout, and runs it on the GSP Falcon (300 ms). FWSEC is what locks WPR2 | VBIOS not readable, FWSEC parse or run failure |
| 4. WPR2 gate | `frts_err` is the top 16 bits of scratch 0x1438; WPR2 is locked when `NV_PFB_PRI_MMU_WPR2_ADDR_LO/HI` decode to a non-empty window | WPR2 not locked |
| 5. Boot arguments and booter | Builds the boot arguments (`gsp_common::bootargs::GspBootArgs`, pinned in `BOOT_ARGS`), writes the libos table address to GSP `MAILBOX0/1`, runs `booter_load` on SEC2 under the generic bootloader with the WPR meta address in SEC2 `MAILBOX0/1` (300 ms), then writes the bootloader's app version to GSP `FALCON_OS`. `mb0 == 0` from the booter is success | The booter cannot be parsed or times out |
| 6. Status queue | Waits up to 500 ms of PTIMER time for GSP-RM to write its status queue header (MSGQ 2.0) in the shared region. 615 has no `GSP_INIT_DONE` event, so the header is all it looks for | Nothing written in time |

The command prints the stage reached, the WPR2 window or the FRTS error, the booter's mailbox and, at stage 6, the queue's version and slot count. The details are on the serial port under `[gsprm]`, `[sec2/booter]`, `[fwsec]` and `[nvdec]`.

Why it does not finish today:

- **Firmware line.** The boot arguments are shared with the Blackwell driver and follow 615.78.08 (eight log regions, a 184-byte `GSP_ARGUMENTS_CACHED`, MSGQ 2.0); the Turing booter and image are 570.144's, which no longer match ([../dev/gsp-615.md](../dev/gsp-615.md), "Turing").
- **Devinit.** The VBIOS init script is never run. On a card the system firmware did not POST (a secondary card), VRAM size, PTIMER and the thermal limits read as zero, and stage 2 stops at `NoVram`.
- **WPR2.** The FWSEC FRTS path has no recorded run on a TU116, so whether stage 4 passes is unknown.
- **After the header.** There is no Turing transport: no receive path, no RPC, no `GSP_INIT`. Even a stage 6 success would leave GSP-RM unused.

## The WPR2 layout

`gsprm::compute_layout` lays the firmware out from the top of VRAM down, as nouveau's `r535_gsp_wpr_meta_init` does. Every boundary is aligned down to 64 KiB.

| Region, top to bottom | Size | Inside WPR2 |
|---|---|---|
| VGA workspace | 256 KiB | No (WPR2 ends at its base) |
| FRTS | 1 MiB | Yes |
| GSP RISC-V bootloader | Its file size | Yes |
| GSP-RM `.fwimage` | Its section size | Yes |
| WPR heap | 8 MiB | Yes (WPR2 starts at its base) |
| Non-WPR heap | 1 MiB | No |

The WPR meta block (`GspFwWprMeta`, `gsp_common/gspfw.rs`) is 256 bytes, magic `0xdc3aae21371a60b3`, revision 1, with a compile-time size check. `GspFwWprMeta::from_layout` in `gsprm.rs` fills it from this layout; Blackwell fills the same struct from the FMC's numbers instead.

## What is shared with the Blackwell driver

The rule in `kernel/drivers/gpu/nvidia/mod.rs` holds: `turing/` may use the chip-neutral layer and `gsp_common/`, and nothing there uses `turing/` (one exception: `generic.rs` reads the Turing temperature sensor for unknown Turing cards). See [Nvidia_gb206.md, How the driver is layered](Nvidia_gb206.md#how-the-driver-is-layered).

| Shared module | What Turing uses it for |
|---|---|
| `mmio.rs`, `chip.rs`, `pci.rs`, `regs.rs`, `le.rs` | BAR0 access, chip decode, PCI config, PTIMER |
| `falcon.rs`, `fbif.rs`, `pri.rs` | Falcon reset, IMEM/DMEM upload by PIO and DMA, halt waits, the DMA interface, PRI sentinels |
| `nvfw_hs.rs` | The NVFW container header and the HS headers of signed images |
| `vbios.rs` | ROM and PRAMIN-shadow reads, images, BIT tokens |
| `pmc.rs` | Interrupt masking, `PMC_ENABLE`, the PTIMER scale |
| `fb.rs` | Is GRUB's framebuffer inside a BAR |
| `gsp_common/gspfw.rs` | The ELF section parser, radix3, the WPR meta block, the bootloader container |
| `gsp_common/bootargs.rs` | The libos table, `GSP_ARGUMENTS_CACHED`, the CMDQ/MSGQ shared region, and `poll_status_queue`, written for this path |
| `crate::mm::dma::DmaBuffer` | Physically contiguous system memory for every staged image |

The RPC transport, the RM object layer, video memory, channels, display and compute in `gsp_common/` are all GB20x-only; their class numbers are GB20x's ([Nvidia_gb206.md](Nvidia_gb206.md#the-rpc-transport)).

## Commands

These `nvidia` subcommands act on the Turing card (`kernel/shell/commands/nvidia_cmds.rs`). Without a Turing card they say none is bound, except the three marked "no card". From msh, `info`, `list`, `temp`, `firmware` and `next` run directly; the rest need `mikudo nvidia ...`. The GB20x subcommands are in [Nvidia_gb206.md, Commands](Nvidia_gb206.md#commands).

**State:**

| Command | Output |
|---|---|
| `nvidia` / `nvidia info` | Model, PCI id, subsystem, chip fields and `PMC_BOOT_42`, BARs, `PMC_ENABLE`, straps, PTIMER, MSI/MSI-X, IRQ line, whether the console is scanned out through the card |
| `nvidia list` | The Turing card ("full driver") and every card brought up host-side |
| `nvidia debug` | BAR0 register dump: PMC, PBUS, PFIFO, PTOP |
| `nvidia temp` | `TEMP_SENSOR`, the temperature, the slowdown and shutdown limits |
| `nvidia falcon` | Liveness, IMEM/DMEM size, CPUCTL and mailboxes of SEC2, GSP, NVDEC, FECS, GPCCS |
| `nvidia wpr-state` | The PFB WPR1 and WPR2 windows and the MMU read/write allow masks (read-only) |
| `nvidia firmware` (no card) | Every file in `TU116_FIRMWARE`: engine, size, NVFW header; total size, GSP line, whether the store is mounted |
| `nvidia acr-info` (no card) | For each ACR file: container, header fields, hex of the HS header and the payload start |
| `nvidia next` | A bring-up checklist with the Falcon liveness filled in, and a suggested next step (its later steps are out of date, see [Known gaps](#known-gaps)) |
| `nvidia splash` | Redraws the boot splash, whose footer names the Turing card |

**Probes of the register map** (read-only): `nvidia pmc-scan` (PMC area 0x000..0x1000, looking for enable and gating registers), `nvidia dma-state` (each engine's DMA transfer registers), `nvidia fbif-scan` (each live Falcon's FBIF window), `nvidia fbif-decode` (the eight TRANSCFG slots, decoded). `nvidia ungate` sets `PMC_ENABLE` GR and CE0 again, as the boot does.

**DMA checks:** `nvidia dma-test` fills one page with a pattern, DMAs 256 bytes of it into SEC2 DMEM through TRANSCFG slot 7 and compares; `nvidia imem-test` does the same into IMEM with tags, the path HS ucode uses.

**Boot steps, one at a time:**

| Command | What it does |
|---|---|
| `nvidia gsp-rm` | VRAM size, the WPR2 layout and the ABI sizes (`gsprm::prepare`) |
| `nvidia gsp-rm-dryrun` | Builds radix3 and the WPR meta over a synthetic 256-page image and checks they resolve |
| `nvidia gsp-rm-load` | Stage 2 alone, pinning the state |
| `nvidia gsp-bootargs` (no card) | Builds the boot arguments, reads every field back and checks it, frees them. `msgq-test` and `rpc-test` are aliases |
| `nvidia nvdec-scrub` | Stage 1 alone |
| `nvidia sec2-acr`, `nvidia sec2-acr-v2` | SEC2 runs `acr/bl` with `ucode_ahesasc` staged in system memory; v2 also writes a real `flcn_bl_dmem_desc` to DMEM. Both report the halt status. Not part of the pipeline |
| `nvidia gsp` | `booter_load` on the GSP Falcon with cleared mailboxes ("first contact") and an interpretation of `mb1`. `nvidia gsp unload|load|reset` are GB20x-only |
| `nvidia gsp-rm-boot` | `booter_load` on the GSP Falcon with the pinned WPR meta address; needs `gsp-rm-load` first. The pipeline runs the booter on SEC2 instead |
| `nvidia gsp-rm-boot-full` | The whole pipeline above |

## Boot flags

The Turing path reads no boot flag. `nogpu` skips the GB20x display boot only (`nvidia::autoboot`); `turing::init` runs in the "NVIDIA GPU probe" step either way. All GPU flags are in [Nvidia_gb206.md, Boot flags](Nvidia_gb206.md#boot-flags) and [wiki/boot.md](wiki/boot.md).

## Verification status

- QEMU has no NVIDIA card, so it runs none of this. In QEMU the commands say no card is bound; `nvidia firmware`, `nvidia acr-info` and `nvidia gsp-bootargs` work without a card.
- The developer's machine has an RTX 5060 Ti and no Turing card. No run of the Turing path on a TU116 or TU117 is recorded in the repository or the project notes, so every hardware statement here comes from reading the code, OpenRM and nouveau.
- The driver selftest (`run=nvidia_selftest`) builds and reads back the boot arguments Turing shares with GB20x; it has no Turing case ([wiki/testing.md](wiki/testing.md)).

## Code map

| Path | What is there |
|---|---|
| `turing/mod.rs` | `matches`, `model_name`, `FW_DIR` (`nvidia/tu116`), `init` |
| `turing/tu116.rs`, `tu117.rs` | Device-id tables, model names, expected BAR sizes |
| `turing/device.rs` | `Gtx1650`, the registered device record |
| `turing/init.rs` | The boot-time bring-up, the firmware probe, the PTOP walk |
| `turing/quirks.rs` | Per-chip reset bits and flags |
| `turing/regs.rs` | PTOP, PFB local-memory-range and WPR registers, PTHERM |
| `turing/therm.rs` | The temperature sensor and limits |
| `turing/tu116_fw.rs` | The firmware list and one fetch function per file |
| `turing/gsprm.rs` | VRAM size, the WPR2 layout, `load`, the pinned state, the six-stage `boot` |
| `turing/nvdec.rs` | The NVDEC scrubber |
| `turing/fwsec.rs` | FWSEC from the VBIOS and the FRTS command |
| `turing/sec2.rs` | HS image parsing, the generic bootloader descriptor, the ACR experiments, `attempt_booter_load` |
| `turing/gsp.rs` | `booter_load` on the GSP Falcon (the first-contact path) |
| `kernel/drivers/gpu/nvidia/mod.rs` | `init` (dispatch by device id), the Turing device slot |
| `kernel/drivers/gpu/nvidia/firmware/gtx1650/` | The firmware files (git-ignored) |
| `kernel/shell/commands/nvidia_cmds.rs` | The commands |

## Known gaps

- No VBIOS devinit; a card the system firmware did not POST cannot be staged.
- The GSP-RM boot is unfinished and stuck between firmware lines: 570.144 booter and image against 615 boot arguments.
- The FWSEC FRTS lock, the SEC2 booter and the scrubber descriptor have never been run on a card.
- No transport, RPC or RM objects after the boot, no MSI, no interrupt handling.
- No video memory allocator, GPU page tables, channels, display or compute. FECS and GPCCS firmware is listed but never loaded.
- TU117 gets the host bring-up only; TU102, TU104 and TU106 get the generic path.
- Out-of-date text in the code: `nvidia next`'s later steps say the firmware is embedded, there is no DMA allocator and no GSP-RM image; the comments in `nvidia/mod.rs` and `generic.rs` say Turing has an embedded bundle and runs the full pipeline; `init.rs` describes a three-step SEC2 ACR boot; the pipeline comment in `gsprm.rs` names SEC2 ACR as stage 3 and `GSP_INIT_DONE` as stage 6; `quirks.rs` records no GSP-RM or booter path though `tu116_fw.rs` has them.
