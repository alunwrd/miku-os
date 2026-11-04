# NVIDIA GB206 (Blackwell): the RTX 5060 Ti driver

MikuOS has its own driver for the GeForce RTX 5060 / 5060 Ti (GB206, Blackwell), in `kernel/drivers/gpu/nvidia/`. It boots the card's GSP-RM firmware through the FSP chain of trust and then does what OpenRM's kernel half does: it talks to GSP-RM over the shared message queue, allocates RM objects, builds its own GPU page tables, runs copy and compute channels, drives the display through the core and window channels, and reads the clocks the firmware chose. It was written by reading the published nvidia-open source (OpenRM 615.78.08, with NVKMS for the display) function by function; it is not a port and has no C or Linux shims. QEMU does not model the card, so everything below that touches hardware was checked only on the developer's machine, an RTX 5060 Ti with one DisplayPort panel. This page describes how the driver works, how to use and diagnose it, and what has and has not been run on the card.

## Contents

- [The hardware and the scope](#the-hardware-and-the-scope)
- [How the driver is layered](#how-the-driver-is-layered)
- [The boot, in order](#the-boot-in-order)
- [Firmware](#firmware)
- [The RPC transport](#the-rpc-transport)
- [The RM object tree](#the-rm-object-tree)
- [GSP-RM's health, logs and crash reports](#gsp-rms-health-logs-and-crash-reports)
- [Interrupts](#interrupts)
- [Memory](#memory)
- [Channels, the copy engine and fences](#channels-the-copy-engine-and-fences)
- [Compute](#compute)
- [The display](#the-display)
- [Window buffers in video memory](#window-buffers-in-video-memory)
- [Power and clocks](#power-and-clocks)
- [Unload, load, reset and recovery](#unload-load-reset-and-recovery)
- [Commands](#commands)
- [The selftest](#the-selftest)
- [Boot flags](#boot-flags)
- [Checking on the card](#checking-on-the-card)
- [Verification status](#verification-status)
- [Code map](#code-map)
- [Known gaps](#known-gaps)

## The hardware and the scope

| PCI device id | Name |
|---|---|
| 0x2D04 | GeForce RTX 5060 Ti (8 GB and 16 GB share the id) |
| 0x2D05 | GeForce RTX 5060 |
| 0x2D18 | GeForce RTX 5070 Laptop |
| 0x2D19 | GeForce RTX 5060 Laptop |

The device id only gives the marketing name. The chip is decided the way OpenRM decides it, by `PMC_BOOT_42`: architecture 0x1B is GB20x, and the implementation field picks GB202, GB203, GB205, GB206 or GB207 (`blackwell/chips.rs`). The family shares one firmware set and one implementation of every OpenRM function the driver copies, so the driver targets all five; only GB206 has been tested.

**In scope:** GSP-RM boot and lifecycle, RM objects and controls, video memory, GPU page tables, copy and compute channels, GPU programs for the compositor and for user programs, DisplayPort and HDMI output, the colour pipeline, the cursor plane, P-state and clock reporting. **Out of scope, by the user's decision:** 3D and the video engines. The other NVIDIA path in the tree is Turing ([Nvidia_tu116.md](Nvidia_tu116.md)); any other NVIDIA card gets host-side recognition only (`generic.rs`).

## How the driver is layered

`kernel/drivers/gpu/nvidia/mod.rs` states the rule: three layers, and dependencies point only up this list.

| Layer | Contents | Knows the chip? |
|---|---|---|
| chip-neutral | `mmio`, `intr`, `regs`, `chip`, `pci`, `vbios`, `falcon`, `fbif`, `pmc`, `nvfw_hs`, `profile`, `fb`, `pramin`, `gmmu`, `vaspace`, `user_vram`, `scanout`, `surface`, `blur`, `kms`, `cubin`, `shaders`, `chipset` | no |
| `gsp_common/` | the GSP-RM world once the RISC-V core runs: boot arguments, the message queue and RPC, the RM object and control layer, display and compute channels, firmware-container parsers, perf | no (but the class numbers in `disp.rs`, `chan.rs` and `compute.rs` are GB20x) |
| `blackwell/` (and `turing/`) | per-architecture bring-up and the chip's halves | yes |

Driver lines go to the kernel log with levels (`klog`); each subsystem signs its lines with its own site, so `nvidia log <section>` can filter by subsystem ([wiki/diagnostics.md](wiki/diagnostics.md)).

## The boot, in order

`nvidia::init` scans PCI for vendor 0x10DE and, for a GB20x, runs `blackwell::init` (recognition only), after which `nvidia::autoboot` runs the whole GSP-RM boot automatically (since 2026-09-06). `nogpu` on the boot line skips the boot and leaves the card as the firmware set it; `nvidia gb206-boot` still runs it by hand. On the card the display is up about 5 s after power-on (measured 2026-10-01, down from about 60 s).

**Recognition** (`blackwell/init.rs`; every stage non-fatal, the card stays registered and inspectable):

1. BAR0 validated and mapped, memory decode and bus mastering on, legacy INTx masked.
2. Chip identified from `PMC_BOOT_42` (and `PMC_BOOT_0` printed beside it).
3. MSI and MSI-X capabilities walked (config reads only).
4. PTIMER checked to be ticking.
5. FSP state read: the secure-boot scratch and the queue pointers. On a card the VBIOS posted, the FSP finished its own secure boot long before the OS.
6. The four firmware files probed under `/lib/firmware/nvidia/<chip>/gsp/`; the FMC is parsed and its crypto material checked against the GB20x chain-of-trust sizes (hash 48 bytes, public key 97, signature 96: ECDSA P-384).

**The GSP-RM boot** (`blackwell/bringup/`, after OpenRM's `kgspInitRm` and `kgspBootstrap_GH100`):

| Step | Where | What happens |
|---|---|---|
| Bus | `chipset/` | The chipset and root port identified from OpenRM's tables; the GPU's PCIe Device Control set before any DMA, as `clUpdatePcieConfig` does (`pciebase` leaves it alone) |
| Wait for the FSP | `hal_gb20x.rs` | `kfspWaitForSecureBoot_GB202`: up to 5 s for the FSP's secure boot to report complete. GFW boot progress is read but is not a gate on FSP-managed chips |
| WPR2 check | `hal_gb20x.rs` | A write-protected region still up means a GSP-RM was booted and never unloaded; OpenRM refuses to boot over it, and so does this |
| Staging | `bringup/stage.rs` | Sysmem flush buffer and GPU time set; the GSP-RM image and the ucodes container staged in system memory, each behind a radix3 page table; the WPR heap sized from the framebuffer; the libos boot arguments and log buffers built; our command queue announced (`msgqTxCreate`: its header written, `QUEUE_HEAD = 0`); the system information and registry encoded as NVKV for `GSP_INIT`; the WPR metadata; the 256-byte `GSP_FMC_BOOT_PARAMS`; the chain-of-trust payload |
| Chain of trust | `fsp.rs`, `fmc.rs`, `hal_gb20x.rs` | The FMC error scratch cleared; one `NVDM_TYPE_COT` message (`NVDM_PAYLOAD_COT` version 2) sent to the FSP over MCTP-over-EMEM, pointing at the staged FMC and its hash, key and signature. The FSP verifies the FMC; the FMC carves the FRTS region and WPR2, loads GSP-RM and releases the RISC-V lockdown |
| Start | `hal_gb20x.rs` | Lockdown release polled (mailboxes decoded: busy, boot-parameter address, or an FMC error code); the checks that GSP-RM really started; `FALCON_OS` written |
| Handshake | `bringup/handshake.rs` | GSP-RM writes its status queue header; the host links it (`msgqRxLink`) and sends GMCAPI `GSP_INIT`, which carries the system information and registry in and the static information back. Boot events (libos prints, lockdown notices, NOCAT records, Xids) are handled while it waits, in half-second slices, two minutes at most |
| RM | `bringup/rm.rs` | The ucodes radix3 freed; the RM client chain built; the static information read (framebuffer regions seed the VRAM allocator, the internal client's handles kept) |
| Engines | `engines.rs` | Brought up in OpenRM's `gpuStateLoad` order: memsys (the flush buffer checked), GSP interrupt (the MSI, the tree, the GSP's vector), perf (the shared-data page), display (outputs, modeset, window, display vectors), channels (page directory, copy channel, compute channel). `ENGINES: N of 5 up` is printed. Unload runs the list backwards |

There is no `GSP_INIT_DONE` any more: 615 replaced it, the prefilled `SET_SYSTEM_INFO`/`SET_REGISTRY` and `GET_GSP_STATIC_INFO` with the one GMCAPI `GSP_INIT` exchange. The serial log says `RESULT: GSP_INIT answered - GSP-RM is up`. The whole boot is stamped into the persistent RAM log so a recovered dump says which run wrote it. Design and history: [../dev/gsp-foundation.md](../dev/gsp-foundation.md), [../dev/gsp-615.md](../dev/gsp-615.md).

**BARs.** BAR0 is the register space (with PRAMIN's 1 MiB window into video memory inside it); BAR1 is the aperture into video memory, switched to physical mode at boot (see [Memory](#memory)); the 64-bit instance BAR after it is GSP-RM's, reported to it in the system information and not used by the driver.

## Firmware

The driver speaks exactly one GSP-RM release, 615.78.08, and the four files must be from that release (`crate::gsp_rm_version!()` names it). Since 2026-10-15; before that it was 570.144.

| File, under `/lib/firmware/nvidia/gb206/gsp/` | What it is | Source |
|---|---|---|
| `gsp-615.78.08.bin` | GSP-RM, about 27 MB; ga102's image carrying the GB20x signature in `.fwsignature_gb20x` | `gsp_ga10x.bin` from the driver package (`/usr/lib/firmware/nvidia/615.78.08/` on Arch) |
| `ucodes-615.78.08.bin` | The `NVUCODES` container GSP-RM reads its ucodes from (new in r610), about 58 MB | `ucodes_ga10x.bin` from the same package |
| `fmc-615.78.08.bin` | GSP-FMC, the chain-of-trust stage: an ELF32 with `image`, `hash`, `signature`, `publickey` sections, about 0.3 MB | built from OpenRM 615's bindata |
| `bootloader-615.78.08.bin` | The RISC-V boot ucode: NVFW header, `RM_RISCV_UCODE_DESC` (92 bytes in 615), image, about 0.3 MB | built from OpenRM 615's bindata |
| `gsp_log-615.78.08.bin` (optional) | GSP-RM's logging ELF, for decoding its log | NVIDIA's `.run` package; 615 does not ship it either, so the log is mostly undecoded |

linux-firmware still has only 570.144 for GB20x, so `dev/nvidia/fw_extract.py` builds the set: it copies the two package files and builds the FMC and bootloader from the bindata arrays in OpenRM's `src/nvidia/generated/g_bindata_*.c` (NVIDIA's own extractor cannot read 615's 64-bit-word arrays). The tree and the package must be the same release:

```
python3 -I dev/nvidia/fw_extract.py --tree refsrc/openrm \
    --driver /usr/lib/firmware/nvidia/615.78.08 \
    --out kernel/drivers/gpu/nvidia/firmware/rtx5060ti/gsp
```

The files live in `kernel/drivers/gpu/nvidia/firmware/rtx5060ti/gsp/`, which git ignores. The builder stages each firmware folder under `/lib/firmware/` (`rtx5060ti` goes to `nvidia/gb206` through `NVIDIA_FW_CARDS` in `builder/src/main.rs`) into `boot/firmware.img`, the ext2 image GRUB loads as a module; the kernel mounts it read-only. Build with `MIKU_FW=nvidia/gb206` to leave other cards' firmware out: GRUB reads the whole image before the kernel starts, and on a USB stick its size is boot time ([wiki/building.md](wiki/building.md)). Without the firmware the image still builds and the card reports "firmware unavailable". Another GB20x needs its own folder (`nvidia/gb202`, `gb203`, `gb205`, `gb207`, the files identical) and a line in `NVIDIA_FW_CARDS`. An image built with these files is for the owner's machine; publishing one needs the driver package's licence read first.

## The RPC transport

`gsp_common/rpc/` is the 615 message queue, MSGQ 2.0 ([../dev/gsp-615.md](../dev/gsp-615.md), sections 1-3):

- **Two rings** of 4 KiB slots, each behind a 32-byte header its producer writes: ours is the command queue, GSP-RM's the status queue. The read and write pointers are GSP registers, not shared memory: `QUEUE_HEAD`/`QUEUE_TAIL` (0x110c00/0x110c04) for ours and `MSGQ_HEAD`/`MSGQ_TAIL` (0x110c80/0x110c84) for GSP-RM's. They are monotonic counters; a slot is the counter modulo the ring. Writing `QUEUE_HEAD` is the doorbell.
- **A record** is MCTP-framed: magic `MCTP`, the size, an MCTP transport header (version 1, start and end of message), an NVDM header naming the payload (0x25 RM RPC or 0x26 GMCAPI), the payload size, then the payload (24 bytes of framing). A record may take up to 16 consecutive slots; an RM RPC longer than a record is sent as further `CONTINUATION_RECORD`s.
- **Integrity.** MSGQ 2.0 has no checksum and no per-element sequence number. The reader overwrites a record's magic with `SEEN` after reading it and skips a record that still says `SEEN` (at most 16 times); receive checks MCTP version 1, vendor 0x10de, the magic, and a size between 24 bytes and 64 KiB. The 32-byte RPC header (`VRPC` signature, function, length, results) carries a sequence number from one counter shared with GMCAPI, and **a reply is the record with the same function and sequence**.
- **One receive path** (`rpc/poll.rs`, after OpenRM's `_kgspRpcRecvPoll`): reading the ring until the awaited reply arrives, handing every other message to the event dispatcher on the way. The dispatcher (`kgsp/events.rs`) decides what each event means, records and returns, and never makes an RPC; a poll started from inside one is refused (`NestedPoll`). During boot only an allow-list of events is accepted. Every drained event can say which call was in flight (`read while waiting on X, after Y`).
- **Before every RPC**, `kgsp::health::sanity` refuses to send if GSP-RM is known dead or the GPU has fallen off the bus (`PMC_BOOT_0` reading all ones).
- **Timeouts.** A single RM call waits up to 6 s, as `_kgspRpcRecvPoll` allows (it was 2 s until a reloaded GSP-RM took longer than that to allocate a core channel).
- **Waiting** sleeps on the GSP's interrupt when it is up (`rpc/wait.rs`) and polls otherwise. RPCs go one at a time under the RM lock.
- **GMCAPI** (`gmcapi.rs`): `GSP_INIT` (0x10001, waits for its answer) and `GSP_SUSPEND` (0x10004, fire and forget). **NVKV** (`nvkv.rs`) is the key-value encoding the system information, registry and static information travel in (`sysinfo.rs`, `staticinfo.rs`).
- `rpc/loopback.rs` runs the same code over two rings in plain memory, which is how the selftest checks framing, wrap, a full ring, stale records and reply matching without a card.

## The RM object tree

Everything a booted GSP-RM does is reached through `RmAlloc`, `RmControl` and `RmFree` (`gsp_common/rmapi.rs`, wire formats `rpc_gsp_rm_alloc_v03_00` and `rpc_gsp_rm_control_v03_00`; the control header is 40 bytes in 615). The call's own status is an NV_STATUS separate from the transport's result: a call can be delivered and still fail. Status and command names come from generated tables (`rm_names.rs`, from `dev/nvidia/gen_rm_names.py`), so the log names every control.

Every object goes through `RmClient`, which records the tree: `alloc` refuses a live handle and a parent the driver never allocated, `free` frees children depth first and siblings newest first (as NVKMS does; freeing a core channel before its windows made GSP-RM assert), and `class_of` makes a handle's class checkable. Objects get the parent OpenRM gives them: RM tolerates a channel parented to the device, and the graphics context is then missing. Never issue a raw allocation from driver code.

```
NV01_ROOT (client, 0xc1d00000)
└── NV01_DEVICE_0 (0xde1d0000)
    ├── NV20_SUBDEVICE_0 (0x5d1d0000)       most NV2080 controls; events
    ├── FERMI_VASPACE_A (0x90f1)            externally owned: our page tables
    ├── KEPLER_CHANNEL_GROUP_A (0xa06c)     a timeslice group
    │   └── BLACKWELL_CHANNEL_GPFIFO_B (0xca6f)
    │       ├── BLACKWELL_DMA_COPY_B (0xcab5)
    │       └── BLACKWELL_COMPUTE_B (0xcec0)
    └── NVCA70_DISPLAY (0xca70)
        ├── NVCA73_DISP_CAPABILITIES, NVC372_DISPLAY_SW (IMP)
        ├── NVCA7D_CORE_CHANNEL_DMA
        ├── NVCA7E_WINDOW_CHANNEL_DMA, NVCA7B_WINDOW_IMM_CHANNEL_DMA
        └── NVCA7A_CURSOR_IMM_CHANNEL_PIO
```

`nvidia rm-tree` prints the tree as RM holds it for this driver, followed by the graphics context as RM describes it. A leak is a line there. `nvidia chan-cycle N` builds and tears down the copy channel N times and prints what the driver still holds each round. A new client chain means GSP-RM was restarted, so `bootstrap` clears the record.

## GSP-RM's health, logs and crash reports

`gsp_common/kgsp/` is what belongs to GSP-RM itself (OpenRM's `KernelGsp`):

| Module | What it does |
|---|---|
| `events.rs` | What each event means: OS error log (Xid), robust-channel reports (named with the channel's owner), NOCAT records, libos prints, lockdown notices, hot-plug and DP short pulses (queued for the event thread) |
| `service.rs` | The GSP falcon's interrupt: SWGEN0 (a message), HALT, the GB100 fatal bits |
| `health.rs` | Whether GSP-RM is alive, after every drain; a fatal interrupt, a halted RISC-V core or a fatal CrashCat report sets the fatal flag, which stops every RPC and every channel until GSP-RM is unloaded |
| `crashcat.rs` | CrashCat: GSP-RM's own crash reports, read from a 4 KiB system-memory queue registered through the WPR metadata (not with `gspbase`). A libos watchdog report is logged and kept but never fatal |
| `logs.rs`, `../logdecode.rs` | GSP-RM's libos log buffers (INIT, INTR, RM), read after every drain and after an unload's suspend wait; records go to the kernel log at their level (`nvidia log fw`), WARNING and up also to serial. Decoding needs the logging ELF |
| `worker.rs` | The bottom half: events nobody is waiting for are drained by the GPU event thread, woken by the interrupt and at least every 200 ms |
| `unload.rs` | `GSP_SUSPEND` and the wait for the processor to park |

**NOCAT records are not failures.** A NOCAT `ASSERT` or `ENGINE` record is a journal entry GSP-RM writes about something it noticed; the boot is not blocked by it. It is raised during GSP-RM's handling of one request and only read during the next, which is why the log names both calls. When OpenRM has no source for a behaviour, the GSP-RM image itself can be disassembled; `nvidia gb206-sym <hex>` resolves an image offset to a function name where the logging ELF allows it.

## Interrupts

The card raises one MSI (`nvidia/intr.rs`). Everything it says arrives in the VFN interrupt tree at BAR0 0xb80000: 32-bit leaves with a summary word above them; the number of leaves is the chip's (GB20x has more than GA102's eight). Which bit means what is assigned by GSP-RM at boot and read back (`gsp_common/intr.rs`); a vector is `leaf * 32 + bit`. The service order is OpenRM's and nouveau's: unarm, read the summary, read the leaves it points at, clear, rearm. A vector is registered before it is allowed, nothing without a handler is unmasked, and a bit that fires anyway is cleared and counted.

| Source | Handler | Effect |
|---|---|---|
| GSP falcon SWGEN0 | `kgsp::service` | RPC waits and the event worker sleep instead of polling |
| Display DISP and DISP_LOW | `blackwell/interrupts.rs` | The head's per-frame event paces the compositor's frame loop (instead of polling the line counter) |
| Copy and compute engines' non-stall vectors | `blackwell/interrupts.rs` | GPU fences sleep: compute fences raise a non-stall interrupt with a host method, copy fences with the copy engine's own `LAUNCH_DMA` interrupt bit |

The interrupt handlers touch only atomics; the per-frame head is an atomic written once at bring-up so the handler never takes a lock. `nointr` on the boot line never enables the MSI and every GPU wait polls as before; `nononstall` keeps only the fence vectors closed. `nvidia intr` prints the vector table GSP-RM assigned, serviced/spurious/unclaimed counts, vblanks counted, which path the frame loop is on, the CrashCat state, and a `fences:` line (waits woken by the interrupt, waits that outlived a tick, waits that passed with no interrupt; the last should stay near zero).

## Memory

| Piece | File | What it does |
|---|---|---|
| VRAM allocator | `gsp_common/vram.rs` | Every block of video memory has a name and an owner; seeded from the framebuffer regions in GSP-RM's static information. A sorted free list under an ordinary lock (a few dozen blocks; nothing allocates from an interrupt). Blocks come from the `Low` end (scanout surfaces, which must not be near GSP-RM's protected region at the top) or are placed `Below` a ceiling. `nvidia vram` prints the regions, every block and who has it, and what is left |
| PRAMIN | `pramin.rs` | A 1 MiB window into video memory through BAR0, positioned in 64 KiB units. On Hopper and later the base register is `NV_XAL_EP_BAR0_WINDOW` (0x10fd40); the legacy register exists, does not fault, and silently does nothing, so the driver probes which one the chip honours. It survives GSP-RM's BAR1 remap, which is why the boot's text can still reach the screen; `nvidia pramin-test` checks it |
| BAR1 in physical mode | `blackwell/bar1.rs` | GSP-RM puts BAR1 in virtual mode during its display init; the driver binds it back to physical mode (OpenRM's `kbusBar1InstBlkBind_GH100`: LOW, then HIGH, then wait for the pending bits), so a BAR1 offset is a video-memory offset and the CPU writes video memory with ordinary write-combining stores. On from boot unless `nobar1`; bound back before GSP-RM is unloaded. `nvidia bar1 probe` checks both directions against PRAMIN and restores; `on`/`off` by hand |
| GPU page tables | `gmmu.rs` | VER3 format (Hopper and Blackwell), six levels; level 1 is a dual PDE with separate big-page and small-page halves. The tables live in system memory. TLB invalidates target the right space kind (a hub-only invalidate, which OpenRM uses for a BAR space alone, once left the compute engine's TLBs stale and a second program wrote through a freed page) |
| Page sizes | `gmmu.rs` | 4 KiB, 64 KiB and 2 MiB pages chosen piece by piece as OpenRM chooses them; the VRAM allocator aligns and pads a block to its page. Full-screen surfaces map as 2 MiB pages, system memory as 4 KiB. `gpu4k` forces 4 KiB everywhere; `nogpuvm` never unlinks or frees a page table (implies `gpu4k`); `tlball` makes every invalidate target all page directories |
| Address spaces | `vaspace.rs` | OpenRM's `OBJGVASPACE` for a space RM does not manage (`IS_EXTERNALLY_OWNED`): keyed by page directory, a range from 1 MiB to 2^47, named reservations (the kernel's layout from `chanmem::layout`), and the mappings in each. Freeing a reservation unmaps what was left in it; a pool going away unmaps its pages before its frames are freed. `nvidia va [all]` lists spaces, reservations, owners and mappings |
| Program video memory | `user_vram.rs` | `gpu_alloc` (121) gives a process a block from the top of the BAR1 aperture down, 128 MiB for all programs together, at least one 64 KiB page each, zeroed first. The process maps it write-combining; the GPU reads it in place. Given back only after it is out of every GPU address space and both engines have been waited out; a block the engines will not release is kept, never reused. Refusals: `ENODEV` without a card or BAR1, `EDQUOT` over the limit |

Design: [../dev/gpu-memory-compute.md](../dev/gpu-memory-compute.md) (steps 3.1 to 3.10).

## Channels, the copy engine and fences

A channel is built the way any RM client builds one, over RPC (`gsp_common/chan.rs`): an address space whose page directory is ours (`NV0080_CTRL_CMD_DMA_SET_PAGE_DIRECTORY`), memory for the instance block, USERD and the method buffer, a timeslice group (in OpenRM a channel outside a group does not exist; `nochangrp` builds bare channels as before), the GPFIFO channel, binding to an engine and scheduling, and the engine class on it.

- **GPFIFO and the doorbell.** The driver writes methods into a pushbuffer cut into up to 16 slots (16 submissions in flight per channel), stores `GP_PUT` into USERD through BAR1 (read back through the window to flush, as OpenRM flushes video memory by reading through the BAR; PRAMIN under `nobar1` or while BAR1 is being rebound), and rings the doorbell. In 615, RM's work-submit token equals the doorbell value; the boot line says whether it does.
- **The copy engine** (`BLACKWELL_DMA_COPY_B`) moves the console's shadow, the compositor's window copies and uploads; one submission may mix system-memory and video-memory rectangles (a copy carries its own aperture). Up to `MAX_BATCH` copies go in one submission.
- **Fences** are semaphore releases the engine writes into a page the channel keeps; a waiter sleeps on the non-stall interrupt with a one-tick backstop, and the wait happens outside the channel lock. `gpu_wait` (97) is the ring-3 side.
- **Robust-channel recovery.** After a robust-channel report the kernel's compute channel is rebuilt into its kept group and the compositor takes the engine back; a process whose work was lost gets `EIO` from `gpu_wait` (OpenRM's error notifier, as an errno). A rebuilt compute channel needs one bit of its instance block that only GSP-RM's context promotion sets (`INST_ENGINE_CTX_INSTALLED`, word 0x050 bit 1); the driver keeps the working channel's instance block and restores any word a rebuild differs in.
- **Display channels** (core, window, window-immediate, cursor) are driven by pushbuffers of methods committed by an UPDATE (`gsp_common/pushbuf.rs`); the core channel's control region is at BAR0 0x680000.

## Compute

**Programs are CUDA C, compiled at build time.** `lib/buildgen/shadergen.rs` runs from the kernel's `build.rs`, compiles every `.cu` in `kernel/drivers/gpu/nvidia/shaders/` with `nvcc -cubin` for `sm_120`, honouring flags a file names on its first line (`// nvcc: --fmad=false`), and emits a table of program bytes, registers per thread (from `cuobjdump -res-usage`), shared memory and the size of constant bank 0 (parameters start at `c[0x0][0x380]`, checked). There is never a shader compiler in the kernel. Without the CUDA toolkit the build prints `no nvcc - building without GPU programs (blur, shadows)`, `shaders::AVAILABLE` is false, and everything that needs a program reports it is unavailable; the desktop then composes without blur or GPU effects.

**One cubin parser, two includers.** `lib/buildgen/cubin.rs` is included by `shadergen.rs` at build time and by `kernel/drivers/gpu/nvidia/cubin.rs` at run time, where it parses what a process hands to `gpu_program` and checks every bound because a process may lie.

**The built-in programs:**

| Program | File | Used by |
|---|---|---|
| `blur_down`, `blur_up` | `blur.cu` | The Kawase blur of the compositor's backdrop |
| `composite` | `composite.cu` | One launch per window: blurred backdrop, the window at its opacity, the squircle outline and border, the drop shadow |
| `over` | `over.cu` | Folding the wallpaper and the layers below the windows into one image |
| `fill` | `fill.cu` | A test program (also built into `/etc/gpu/fill.cubin` for `gpudemo`) |
| `paint` | `paint.cu` | aosh's painter: a block per 16x16 tile, a thread per pixel, the tile's operations in order, into the window's buffer in video memory ([wiki/aosh.md](wiki/aosh.md)) |

A built-in program may not use a stack, local memory or constant bank 2: the kernel's launches set up none of them, and nvcc puts runtime-indexed arrays on the stack and dense `switch` jump tables in bank 2, which build, launch and read nonsense. The build refuses a program whose resource usage shows `STACK`, `LOCAL` or `CONSTANT[2]`.

**Launches.** A dispatch is a QMD (`gsp_common/qmd.rs`, version 5.0, 96 words, fields as bit ranges from `clcec0qmd.h`) in memory, launched with `SEND_PCAS_A` and `SEND_SIGNALING_PCAS2_B`. **QMDs in one submission run concurrently**: the work distributor runs a small grid beside the previous one. `submit_compute_chain` therefore puts `INVALIDATE_SHADER_CACHES` (data and flush-data, an implicit wait-for-idle) between launches, as NVK does; a chain of launches is ordered and gets one fence. The compute context's buffers are sized by asking RM (`INTERNAL_STATIC_KGR_GET_CONTEXT_BUFFERS_INFO`), allocated, mapped and promoted (`GPU_PROMOTE_CTX`) the way OpenRM does, with two deviations that the card needs (see the comments in `compute.rs`).

**Program slots.** There are 16 program slots of 128 KiB each (`gpu_info` prints `program_slots`); the build uses six. Ring 3 has 16 launch slots of its own above the effects pass's 32.

**From ring 3** (full reference in [MikuOS_ABI.md](MikuOS_ABI.md)):

| Syscall | Number | Capability | What it does |
|---|---|---|---|
| `gpu_info` | 94 | none | Plain text: what the card is, the programs, slots, clocks and power (the perf lines) |
| `gpu_submit` | 95 | none | `GPU_OP_BLIT` (copies between bound pools), `GPU_OP_DISPATCH` (a built-in program), or `GPU_OP_CHAIN` (up to 16 dispatches in order with one fence) |
| `gpu_bind` | 96 | none | Map a pool into the GPU address space; returns its GPU address (a parameter, not mapped in the process) |
| `gpu_wait` | 97 | none | Wait for a fence with a timeout; `EIO` if the channel died |
| `gpu_program` | 98 | `CAP_GPU` | Load a process's own cubin into a slot |
| `gpu_alloc` | 121 | none | A pool of video memory (see [Memory](#memory)) |

Buffers handed to a program carry their size (`PARAM_BUFFER_SIZED`): the kernel puts the GPU address and the bytes from the offset to the end of the pool in the constant bank, and programs check every access against it. That is what makes the built-in programs safe without `CAP_GPU`. `gpu_program` needs `CAP_GPU` because a loaded program runs in the shared GPU address space and could reach the console's shadow, other clients' pools and the compositor's surfaces; a private GPU address space per process (`gpuown`) is refused by GSP-RM: the engine's global context buffers belong to the engine, and the attempt left the kernel's compute channel dead until reboot (measured 2026-09-14). `apps/src/gpudemo.rs` exercises all of it.

## The display

Enumeration and bring-up are `blackwell/display.rs` and `blackwell/modeset.rs`; the chip-neutral display code is `gsp_common/disp.rs`, `output.rs`, `dispcaps.rs` and `gsp_common/modeset/`. Programming follows NVKMS (its source is in `refsrc/openrm/src/nvidia-modeset/`), nvdisplay class 4 (`ca7d`). Design and history: [../dev/display-nvkms.md](../dev/display-nvkms.md), [../dev/gb206-display.md](../dev/gb206-display.md).

- **The output table** (`gsp_common/output.rs`): one entry per display id the board reports (OR info, DPCD, EDID, mode list, head, SOR, output LUT, scanout, cursor, blanked). The lock is never held across an RM call and the table is never read from interrupt context.
- **Capabilities and resources.** The capabilities object is read at boot and the built-in sizes become a floor; heads get tiles and physical windows from free masks, reusing what a lit head has; the SOR is assigned before link training.
- **DisplayPort.** GSP-RM trains the link; the host never touches a training pattern. Training falls back as NVKMS's DP library does (clock recovery drops the rate, equalisation drops lanes), the stream is sized for what trained, and the DPCD capabilities are read from 0x2200 when the extended field is present. A short pulse is handled by the event thread (sink awake, link lost: retrain). No LTTPR training, no MST, no DSC.
- **HDMI / TMDS** is written in `modeset/hdmi.rs` (deep colour runs the clock at bpc/8). It has never had a sink: the machine has one DP panel. HDMI InfoFrames are not written.
- **EDID** (`kernel/drivers/gpu/edid.rs`): base block, CTA extension video codes (OpenRM's 219-row table), DisplayID 1.3/2.0 timings, established and standard timings; HDR static metadata and colorimetry kept but not used. `edidbase` reads only the base block (a 240 Hz panel then falls back to 60 Hz). Modes are ranked by pixels, then refresh.
- **IMP** (`modeset/imp.rs`): RM's display-engine budget is asked for each mode beside the link's answer; it is a second opinion, nothing is refused on it yet. `nvidia disp imp <id>`.
- **Mode set**: propose, check, apply (`blackwell/modeset.rs`). The boot keeps the head the firmware lit (recording the mode its raster really runs) unless `coldhead`, which programs it from cold at the driver's mode when that fits the trained link.
- **Underflow** (`nvidia disp underflow <id>`): asks the raster generator whether it ran dry, which shows as horizontal smears rather than as a failed frame.
- **Variable refresh** (`display vrr on|off`, the `vrr` boot flag; off by default): per head, between the EDID range. The machine's panel is refused correctly (it does not set "ignore MSA timing"; Linux agrees). The DP Adaptive-Sync SDP that NVKMS 615 sends is not sent.
- **Power** (`display on|off`): DPMS. A DP sink is put in D3 over AUX before its link goes down and woken (with retries) after; other sinks get `SET_MONITOR_POWER`.
- **Shutting down.** Every lit head is darkened (window first, then head) before the display objects are freed, as NVKMS does; freeing the core channel under a lit head made GSP-RM assert and left the next GSP-RM a display engine mid-scan.

**The colour pipeline**, in hardware order ([../dev/colour-pipeline.md](../dev/colour-pipeline.md)): window input LUT, six window CSC stages, tone-mapping LUT, composition, head output LUT, OCSC0/OCSC1, PROCAMP, clamp, dither, pixel depth. **The window input LUT takes half-float (FP16) entries and the head's output LUT takes 16-bit fixed point**; building both as fixed point decoded every pixel as a denormal or NaN and kept the screen black until 2026-07-29. `display color [<output>] k=v ...` sets gamma, range, dither, depth (10 bits a component, gated on the EDID) and per-channel gains as one state per head; the head keeps three output LUTs and switches to the new one inside the same update, so a curve never shows mid-frame.

**Scanout and flips** (`scanout.rs`): the window fetches its surface every frame, so drawing is "write video memory, flush" (`NV_UFLUSH`); a flip is needed only to change which surface is scanned. The compositor double-buffers; the flip is held back by an acquire semaphore until the copies land (`noflipsema` makes the processor wait instead).

**The cursor plane** is the default since 2026-09-11 (`nohwcursor` goes back to the compositor's software cursor). The earlier hang came from writing three methods after checking one free slot; NVKMS writes two, each behind its own wait.

**Second head, hot-plug, MST.** Resources are handed out per head and hot-plug is handled as an RM event by the event thread (every 200 ms, or `nvidia disp poll`), but a second head, hot-plug and HDMI have never run: there is no second monitor.

**The KMS layer** (`kernel/drivers/gpu/kms/`, [../dev/kms-architecture.md](../dev/kms-architecture.md)) sits above the driver: DRM's model (objects, an atomic request with test-only) with NVKMS's commit order, and output names as nvidia-drm gives them (the panel is `DP-3`, as on Linux; a DP++ port is two display ids and one connector). `nvidia/kms.rs` is this driver's half: state from the output table, check (link at the depth, IMP's opinion, VRR against the mode, free heads), apply (new heads lit, every other change in one UPDATE, `blackwell/apply.rs`). K1 (the layer and `display`) and K2 (one request, one update, colour per head) ran on the card on 2026-10-01; K3 (Mayland's output manager and maywm's `monitor =` lines, [wiki/mayland.md](wiki/mayland.md#outputs)) is QEMU-verified only. Ring 3 reaches it only through Mayland; there is no device node.

## Window buffers in video memory

A `gpu_alloc` pool can be a Mayland window's buffer, which the compositor reads in place: this system's DMA-BUF, limited to that one case by the user's choice (no descriptor export between processes, no direct scanout). The processor never reads video memory; the copy engine brings down only the rectangles it must blend over, and the closing afterimage and `may_desktop.capture` download through the same path. Ring 3 writes such a buffer with write-combining stores through BAR1. Separately, the compositor keeps a copy of every ordinary window in video memory and uploads only the damage (`render/texture.rs`, `notex` turns it off), because at idle the link runs at PCIe Gen1 x8 (about 2 GB/s) and a 1920x1080 window read from system memory every frame takes 4 to 6 ms against a 4.17 ms frame at 240 Hz. Details: [wiki/mayland.md](wiki/mayland.md#buffers-in-video-memory), [../dev/gpu-window-buffers.md](../dev/gpu-window-buffers.md).

## Power and clocks

On a GB20x the firmware (GSP-RM and its PMU) chooses the P-state and programs every clock and voltage; OpenRM's kernel half never touches a PLL, and neither does this driver ([../dev/gpu-perf.md](../dev/gpu-perf.md), `gsp_common/perf/`). The driver has no clock policy of its own, by the user's decision.

| What | Source |
|---|---|
| P-state | `NV2080_CTRL_CMD_PERF_GET_CURRENT_PSTATE` |
| Clocks, load, power, temperatures, fans, why clocks are held down | The RM user shared data page (`NV00DE_SHARED_DATA`, class 0x00DE, 3184 bytes in system memory) that GSP-RM writes while it is polled (`perf/rusd.rs`). Polling is a lease: readers touch it with atomics and the GPU event thread asks GSP-RM to keep writing, so no syscall makes an RPC. Each section has its own PTIMER stamp and they lag each other, so each line is judged by its own section's age |
| History | The PMU's clock samples, about 5 s (`perf/clocks.rs`) |
| Requests | `boost` (`PERF_BOOST` as the client OS asks it), a clock lock or floor at `RATED_TDP` (`tdp`/`turbo`), the power source (`perf/limits.rs`) |
| The display's floor | The minimum P-state IMP asks for |

`nvidia perf` prints the P-state, clocks, load, power, temperatures, fans, throttle reasons and the PCIe link; `perf clocks` the recent history; `perf lock tdp|turbo|off`, `perf floor tdp|turbo|off` and `perf boost max|1|off [seconds|forever]` ask. The same lines are in `gpu_info` for ring 3. On the card (2026-10-20): idle P8 at 180/405 MHz (GPC/memory), 7.9 W, as Linux; under `lock turbo` 2595/14001 MHz. The link follows the P-state: Gen1 x8 at P8, up to Gen4 x8 in this machine's slot. The SM clock domain reads 0 on this card and is left out; GSP-RM's utilisation ring always came back empty and was removed.

## Unload, load, reset and recovery

| Command | What happens |
|---|---|
| `nvidia gsp unload` | Refused while anything still uses the card. Every engine down in reverse order, the RM client freed (newest first), GMCAPI `GSP_SUSPEND` sent and the processor waited for (events handled meanwhile, mailbox test `MAILBOX0 & SUSPENDED`), WPR2 checked, BAR1 bound back, every record of that GSP-RM forgotten, so a second boot is possible |
| `nvidia gsp load` | The whole boot again |
| `nvidia gsp reset` | A forced unload, a PCIe function-level reset, the BAR firewall waited out, a cold boot, and the compositor restarted. The display comes back from cold (the reset took the head the firmware lit) |
| `gsprecover` (boot flag) | When the event thread first sees GSP-RM fatal, a kernel worker runs the reset path, at most twice a boot |
| `nvidia gb206-reset [force]` | A function-level reset alone, to rerun on-die devinit; refused when GFW boot progress is already 0xff or when this GPU drives the console, unless `force` |
| reboot | `bringup::shutdown`: the full unload with a short budget (about a second), as OpenRM's `RmShutdownAdapter` does; sending the unload with everything alive once crashed GSP-RM |

**The GSP reload test.** The unload darkens the screen, so it is tested unattended. Its boot line is `noauth "run=pstore_mirror;nvidia_gsp_unload;nvidia_gsp_load;nvidia_disp;nvidia_intr;nvidia_gb206-diff;mayland_start;nvidia_gb206-diff;mayland_status;mayland_stop;nvidia_log_err;reboot"`, on the `multiboot2` line of a one-entry ISO ([wiki/testing.md](wiki/testing.md#the-developers-qemu-harness)) or added with `e` at the GRUB menu. `pstore mirror` empties the 512 KiB persistent RAM region and from then on every kernel log line at INFO and above, plus the driver's DEBUG lines (every RPC and RM call), is mirrored into it. The final reboot is warm, so RAM survives. On the next boot read it with `mikudo pstore err`, `mikudo pstore -g gsp`, `mikudo pstore -n 60`. `klogpstore` on the boot line mirrors from the start of a boot that itself dies. If the machine hangs, only the reset button keeps the region (the kernel does not handle Ctrl-Alt-Del). See [wiki/diagnostics.md](wiki/diagnostics.md).

## Commands

`nvidia` (alias `gpu`) is a kernel command (`kernel/shell/commands/nvidia_cmds.rs`). From msh, the read-only ones (`nvidia`, `info`, `list`, `temp`, `vram`, `va`, `disp` without `poll`, `perf` without `boost`, `pcie`, `firmware`, `rm-tree`, `gb206`, `next`, `help`) run directly; everything else needs `mikudo nvidia ...` ([wiki/commands.md](wiki/commands.md)). The Turing-only subcommands are listed in [Nvidia_tu116.md](Nvidia_tu116.md).

**State and inventory:**

| Command | Output |
|---|---|
| `nvidia` / `nvidia info` | On a GB20x, the same summary as `nvidia gb206`; on a Turing card, that card's summary |
| `nvidia list` | Every NVIDIA card found: the GB20x (full driver), the Turing card, and every card brought up host-side only (`generic.rs`) |
| `nvidia gb206` | Chip, host boot mode, GFW progress (information only: not a gate for GSP-RM), FSP state, firmware staging, compute and boot state |
| `nvidia fsp` | FSP queue pointers and the secure-boot scratch (read-only) |
| `nvidia gb206-scratch [full]` | One screen of boot state: GFW and PLM, FSP scratch and ucode version, the FMC error code, falcon and RISC-V state; `full` adds the raw 4 KiB `NV_PGC6` dump |
| `nvidia cot-dryrun` | Builds and checks the chain-of-trust payload without sending it |
| `nvidia pcie` | The GPU and its root port as `lspci -vv` shows them, and what GSP-RM was told about the chipset |
| `nvidia intr` | The interrupt table, counts, fences line, CrashCat state |
| `nvidia vram` | The video-memory map |
| `nvidia va [all]` | GPU address spaces, reservations, mappings |
| `nvidia rm-tree` | RM's objects for this driver, as a tree |
| `nvidia perf ...` | P-state, clocks, power; requests (above) |
| `nvidia disp` | Every display id: connector name, OR/SOR and protocol, plugged in or not, DPCD, HDMI caps, head, surface, cursor, adaptive-sync capability |
| `nvidia disp underflow <id>`, `nvidia disp imp <id>`, `nvidia disp poll` | Raster underflow; RM's IMP budget per mode; drain GSP-RM's events now and act on any hot-plug |
| `nvidia gb206-diff` | The lit head's R block: frame rate, SOR, armed state, exceptions, and the diff against the shipping driver's registers |
| `display ...` | Outputs, modes, power, VRR and colour through the KMS layer ([wiki/mayland.md](wiki/mayland.md#displays-and-modes)) |

**Boot and lifecycle:** `nvidia gb206-boot [noscreen] [log] [noprimary] [dispvid]` runs the GSP-RM boot by hand (`noscreen` parks the console before the chain of trust and continues on serial; `log` shows every GSP-RM log line; `noprimary` sends `bIsPrimary=0`; `dispvid` forces display instance memory into video memory, a probe). `nvidia gsp unload|load|reset` and `nvidia gb206-reset [force]` as above.

**Logs:**

| Command | Output |
|---|---|
| `nvidia log [section...] [err\|debug] [-n N] [-f] [-g text]` | The driver's lines from the kernel log, from memory. Sections: `boot` (bring-up: chip, firmware, FSP/FMC, GSP-RM boot, engines), `gsp` (transport, events, health, unload), `fw` (GSP-RM's own log and crash reports), `rpc` (every message), `rm` (objects and controls with status), `disp` (outputs, modeset, DP, EDID, IMP, scanout), `mem` (video memory, page tables, BAR1, PRAMIN), `chan`, `intr`, `perf`. `err` is errors and warnings; `debug` adds every RPC and RM call; `-f` follows; `log help` lists all |
| `nvidia gb206-log [all\|raw]` | GSP-RM's libos log decoded (file, line, text) when the logging ELF is installed, with counts per level and the last 16 warnings and errors; `all` uncaps, `raw` forces ASCII |
| `nvidia gb206-sym <hex>` | A GSP-RM image offset as a function name |

**Experiments and checks:** `nvidia bar1 probe|on|off`, `nvidia pramin-test`, `nvidia chan-cycle N`, `nvidia compute-test` (runs the first program on the GPU and says whether it did what it was asked), `nvidia blur` (blurs what is on screen once, in place, to prove the pass), `nvidia backdrop` (a thumbnail of what the effects pass reads), `nvidia gb206-draw test|fill RRGGBB|clear|text <words>` (draws into the live scanout surface), `nvidia screen on|off` (re-attach or park the framebuffer console), `nvidia selftest`.

## The selftest

`kernel/drivers/gpu/nvidia/selftest.rs` decides on the CPU, in QEMU, everything about the driver that is a pure function: where a virtual address lands in six levels of page table, page-size choice and PDE/PTE bits against OpenRM's `dev_mmu.h`, range overlap and range checks, address-space reservations, the 615 transport over loopback rings (MCTP framing, the pointer registers, `SEEN` and stale records, every receive refusal, ring wrap, reply matching by sequence), GMCAPI responses, NVKV round trips, the system and static information lists, `GSP_FMC_BOOT_PARAMS`, boot arguments built and read back, the heap and FRTS numbers, chipset tables, the interrupt table, CrashCat reports built by hand, the cubin parser, sized buffers and chains, KMS names and checks, VRR arithmetic, and EDID. Nothing in it reads or writes a page table on a card. Run it with `run=nvidia_selftest` or `nvidia selftest`. The last recorded run was 184 of 184 in QEMU on 2026-10-21. See [wiki/testing.md](wiki/testing.md).

## Boot flags

Every GPU-related word the kernel reads from the boot line. All but `vrr` and `gsprecover` are ways back from one change. The general flags (`trace`, `klogpstore`, `noauth`, `fbdirect`) are in [wiki/boot.md](wiki/boot.md#kernel-command-line-reference).

| Flag | Effect | When to use | Code |
|---|---|---|---|
| `nogpu` | The GSP-RM boot is skipped; the card stays as the firmware set it and the desktop composes on the processor over the firmware framebuffer | A boot that dies at `NVIDIA display` | `nvidia/mod.rs` |
| `nointr` | The MSI is never enabled; every GPU wait polls | Interrupt problems | `nvidia/intr.rs` |
| `nononstall` | Only the fence (non-stall) vectors stay closed; fence waits spin | Fence wake-up problems | `blackwell/interrupts.rs` |
| `gspbase` | GSP-RM is booted without the sysmem flush buffer, the wall-clock time, `bIsPrimary` from the machine, the larger heap and the CrashCat queue, and with the chipset half of the system information zeroed | GSP-RM stops answering `GSP_INIT` | `blackwell/bringup/mod.rs` |
| `gsprecover` | Automatic unload, reset and reload when GSP-RM dies (at most twice a boot) | Experimenting with recovery | `blackwell/bringup/recover.rs` |
| `pciebase` | The GPU's PCIe Device Control left as the firmware set it | Copies or the picture look wrong | `chipset/mod.rs` |
| `nobar1` | BAR1 left in virtual mode; programs' video memory has no CPU address, stores go through PRAMIN | BAR1 problems | `blackwell/bringup/mod.rs` |
| `gpu4k` | Every GPU mapping in 4 KiB pages | GPU page-table problems | `gmmu.rs` |
| `nogpuvm` | GPU page tables are never unlinked or freed (implies `gpu4k`) | The picture disappears after an unmap | `gmmu.rs` |
| `tlball` | Every GPU TLB invalidate also targets all page directories | Stale GPU translations | `gmmu.rs` |
| `nochangrp` | Channels built bare, without a timeslice group | Channel creation fails | `gsp_common/chan.rs` |
| `ctxuvm` | Compute context buffers described the UVM way; GSP-RM refuses it. Kept to rerun the reasoning | Never, normally | `gsp_common/compute.rs` |
| `gpuown` | A process's GPU programs get a private address space and context; refused by GSP-RM on the card | Testing that path | `blackwell/compute_chan.rs` |
| `coldhead` | The firmware's head programmed from cold at the driver's mode | Testing the cold modeset | `blackwell/modeset.rs` |
| `edidbase` | Only the EDID's base block | A fast mode misbehaves | `blackwell/modeset.rs` |
| `vrr` | Variable refresh asked for on the primary output once the picture is up | Trying VRR | `nvidia/mod.rs` |
| `nohwcursor` | The compositor draws the pointer | A head without a plane; a cursor-plane hang | `blackwell/window.rs` |
| `noflipsema` | The flip is not held by the acquire semaphore; the processor waits for the copies | A window that stops arming | `mayland/render/mod.rs` |
| `nobackdrop` | No backdrop buffer: the blur pass reads the screen it writes | Wrong picture under a window | `blur.rs` |
| `notex` | No video-memory copies of windows | A copy shows an older picture | `mayland/render/texture.rs` |

A flag goes on one boot with `e` at the GRUB menu ([wiki/boot.md](wiki/boot.md#the-boot-line)).

## Checking on the card

[../dev/nvidia-card-checklist.md](../dev/nvidia-card-checklist.md) is the ordered list of everything built and not yet run on the card, with the command to type and what the log should say for each step. Reboots of the real machine are expensive, so items are batched: the first boot covers what a normal boot can show, and a second boot (`coldhead`, then a reset) covers the rest. How to use it:

- Do the QEMU part first (section 0): `run=nvidia_selftest`, `run=dmesg_selftest`, `run=mayland_test`, and a normal QEMU boot with the processor compositor.
- On the card, from msh in the desktop: `mikudo nvidia log boot` (the `display up` stamp), `mikudo nvidia log err` (NOCAT records name their stack and the call in flight), `mikudo nvidia log disp`, `mikudo nvidia log rm debug -n 60` (every control named), `nvidia perf`, `mikudo nvidia intr` (fences line: woken-by-interrupt growing, passed-with-no-interrupt near zero), `mikudo nvidia disp`.
- Photograph the screen: there is usually no serial cable on the real machine. For anything that may take the screen, use the reload-test pattern: `pstore mirror`, run, warm reboot, read `mikudo pstore`.
- Escape hatches if a boot goes wrong, in the order the list gives them: `gspbase`, `nointr`, `nononstall`, `nogpuvm`, `nogpu`, `trace`, `fbdirect`, `nobar1`, `gpu4k`.

Open items at the time of writing include: the kernel-core, heap and bottleneck rounds (all QEMU-verified, card unrun), the folded backdrop's re-check after the launch-ordering fix, the window shape in the compute pass, layers stacked in the pass, the DMA-BUF window's close animation and capture, and K3's `monitor =` lines.

## Verification status

QEMU runs none of the hardware paths; "card" means the RTX 5060 Ti machine.

| Feature | Status |
|---|---|
| GSP-RM 615.78.08 boot, desktop and compute | Card 2026-10-15 (second boot: no errors, work-submit token equals the doorbell) |
| GSP-RM unload, load, reset on 615 | Reported working by the user 2026-10-20 (no pstore photo) |
| Unload and reload with the display (570 firmware) | Card 2026-10-01, through the reload test (the fourth run) |
| Boot to display in about 5 s | Card 2026-10-01 |
| Fences sleeping on non-stall interrupts | Card 2026-10-01 (877 interrupts, none missed) |
| MSI, GSP and display interrupts | Card since 2026-08-29 |
| BAR1 physical mode (3.1) | Card 2026-09-29; physical from boot (3.7) card 2026-10-02 |
| Program video memory, `gpu_alloc` (3.7) | Card 2026-10-02, after the TLB invalidate fix |
| 64 KiB and 2 MiB pages (3.8) | Card 2026-10-02 |
| `GP_PUT` through BAR1 (3.5b) | Ran on the card 2026-10-02; the read-back flush added the same day |
| Window copies in video memory (3.10) | Card 2026-10-03 |
| PCIe and chipset setup like OpenRM (1.17) | Card 2026-10-06, the bus matches Linux |
| Compute channel rebuild | Card 2026-09-14 |
| `gpu_program` and `gpudemo` | Card 2026-09-14 |
| Private GPU address space (`gpuown`) | Refused on the card 2026-09-14; does not work |
| Picture on the panel (FP16 input LUT) | Card 2026-07-29 |
| Cold head at 240 Hz | Card 2026-10-01 |
| DPMS off and on, `disp poll` | Card 2026-09-09 |
| 10 bits a component | Card 2026-09-15 |
| KMS K1 and K2 (names, one update, colour per head) | Card 2026-10-01 |
| KMS K3 (`monitor =`, output manager) | QEMU only |
| VRR | Refused correctly on this panel (card 2026-10-01); never driven |
| Cursor plane | Default since 2026-09-11 |
| Hot-plug, HDMI, second head | Never run (no second monitor or HDMI sink) |
| P-states, RUSD, PMUMON, boost/lock/floor | Card 2026-10-20 |
| Window buffers in video memory | Card 2026-10-20; afterimage and capture not yet seen |
| Folded backdrop and per-window composite | Card 2026-10-20 (smooth, striped widgets); the launch-ordering fix is unrun |
| aosh's `paint` program | Card 2026-10-21 (smooth); two fixes the same day unrun |
| CrashCat decoding | Decoded a real GSP-RM fault on the card 2026-10-01 |
| `gsprecover` | Not known to have fired on the card |
| `nvidia_selftest` | 184 of 184 in QEMU, 2026-10-21 |

## Code map

| Path | Contents |
|---|---|
| `mod.rs` | Layering, `init` dispatch (Turing, GB20x, generic), `autoboot`, the active-device records, `said`/`speaking` log helpers |
| `pci.rs`, `chip.rs`, `chipset/` | PCI scan, chip ids, chipset and root-port tables and Device Control |
| `mmio.rs`, `pri.rs`, `regs.rs`, `le.rs` | Register access |
| `intr.rs` | The VFN tree and the MSI |
| `pramin.rs`, `fb.rs`, `gmmu.rs`, `vaspace.rs`, `user_vram.rs` | PRAMIN, framebuffer flush, page tables, address spaces, program video memory |
| `scanout.rs`, `surface.rs` | The screen being driven, the console on it, double buffering, compositor blits; a software rasteriser |
| `blur.rs` | The compositor's effects pass on the compute engine |
| `kms.rs` | This driver's half of the KMS layer |
| `cubin.rs`, `shaders/` | The shared cubin parser; the `.cu` programs and their generated table |
| `selftest.rs` | The driver selftest |
| `falcon.rs`, `fbif.rs`, `nvfw_hs.rs`, `pmc.rs`, `vbios.rs`, `reset.rs`, `profile.rs` | Falcon access, DMA interface, signed-firmware containers, PMC, VBIOS, resets, timing |
| `generic.rs` | Host-side bring-up for any other NVIDIA card |
| `blackwell/mod.rs`, `chips.rs`, `init.rs`, `device.rs` | SKUs, the GB20x chip table, recognition, the device record |
| `blackwell/fsp.rs`, `fmc.rs`, `regs.rs` | FSP EMEM transport, the COT payload and FMC parser, registers |
| `blackwell/bringup/` | The boot in stages: `stage.rs`, `handshake.rs`, `rm.rs`, `narrate.rs`, `unload.rs`, `recover.rs` |
| `blackwell/hal_gb20x.rs`, `memsys.rs`, `engines.rs`, `gspboot.rs` | The chip's halves of the boot, the flush buffer, engine order, boot state |
| `blackwell/interrupts.rs` | GSP, display and fence vectors |
| `blackwell/bar1.rs` | BAR1 physical mode |
| `blackwell/display.rs`, `modeset.rs`, `window.rs`, `control.rs`, `apply.rs` | Enumeration and bring-up, head/SOR/DP, the window and cursor, post-boot display control, the KMS update |
| `blackwell/channel.rs`, `chanmem.rs`, `compute_chan.rs` | The copy and compute channels and their memory layout |
| `gsp_common/rpc/` | The message queue and the one receive path |
| `gsp_common/kgsp/` | Events, service, health, CrashCat, logs, worker, unload, engine order |
| `gsp_common/gmcapi.rs`, `nvkv.rs`, `sysinfo.rs`, `staticinfo.rs`, `bootargs.rs`, `gspfw.rs` | GMCAPI, NVKV, the system and static information, boot arguments, firmware containers (radix3, WPR metadata, ELF) |
| `gsp_common/rmapi.rs`, `rm_names.rs`, `refstate.rs` | The RM object layer, generated names, reference state |
| `gsp_common/vram.rs` | The VRAM allocator |
| `gsp_common/chan.rs`, `compute.rs`, `qmd.rs`, `pushbuf.rs` | Channels, the compute context, launch descriptors, display pushbuffers |
| `gsp_common/disp.rs`, `dispcaps.rs`, `output.rs`, `modeset/` | Display objects and controls, capabilities, the output table, the modeset (core, window, cursor, DP, HDMI, IMP, VRR, colour) |
| `gsp_common/perf/` | P-state, the shared-data page, clock history, requests |
| `gsp_common/intr.rs`, `logdecode.rs` | GSP-RM's interrupt assignment; the libos log decoder |
| `firmware/` | Firmware folders (git-ignored) and their README |
| `turing/` | The Turing driver ([Nvidia_tu116.md](Nvidia_tu116.md)) |
| `kernel/drivers/gpu/kms/` | The KMS layer above the driver |
| `kernel/drivers/gpu/edid.rs`, `edid/` | EDID decoding |
| `kernel/shell/commands/nvidia_cmds.rs`, `display_cmds.rs` | The `nvidia` and `display` commands |
| `kernel/syscall/adapters/gpu.rs` | Syscalls 94 to 98 and 121 |
| `lib/buildgen/shadergen.rs`, `cubin.rs` | Building the GPU programs |
| `dev/nvidia/fw_extract.py`, `gen_rm_names.py` | Building the firmware set; generating RM names |

## Known gaps

- One card tested; the other four GB20x chips are untested and need their firmware folder staged.
- No second head, MST, DSC, HDMI InfoFrames or HDMI FRL; hot-plug and HDMI never run.
- VRR does not send the DP Adaptive-Sync SDP; never driven on a capable panel.
- No HDR output, colour management or ICC; no scaling, rotation or mirroring.
- No 3D and no video engines.
- No private GPU address space per process: user programs share the kernel's, which is why `gpu_program` needs `CAP_GPU`.
- No clock policy of our own; the SM clock domain and GSP-RM's utilisation samples are not available.
- GSP-RM's own log is mostly undecoded without the logging ELF, which 615 does not ship.
- `gsprecover` and the reset path have little card mileage; the reload test on 615 has no recorded pstore.
- The firmware set comes from NVIDIA's driver package; publishing an image with it needs the licence checked.
