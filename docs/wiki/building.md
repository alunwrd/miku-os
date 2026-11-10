# Building and running MikuOS

MikuOS is built by one host program, `builder/`, which compiles the dynamic loader, the vDSO, the shared libraries, every ring-3 program and the kernel, packs the programs and `/etc` into packages, installs those packages into a staging tree, turns the tree into an ext2 image that GRUB loads as a boot module, and makes a BIOS-bootable ISO. This page covers what the build host needs, how the repository is laid out, what the builder does step by step, the cargo targets involved, building pieces on their own, and running the result in QEMU or on real hardware.

## Contents

- [Host requirements](#host-requirements)
- [Repository layout](#repository-layout)
- [The builder](#the-builder)
- [Targets and cargo configuration](#targets-and-cargo-configuration)
- [How each program set is built](#how-each-program-set-is-built)
- [Building pieces on their own](#building-pieces-on-their-own)
- [What ends up on the machine](#what-ends-up-on-the-machine)
- [Running in QEMU](#running-in-qemu)
- [Disk images and the QEMU drive layout](#disk-images-and-the-qemu-drive-layout)
- [Real hardware and USB sticks](#real-hardware-and-usb-sticks)
- [Common build problems](#common-build-problems)

## Host requirements

| Tool | Used for | Without it |
|---|---|---|
| Rust nightly with `rust-src` (through rustup) | Everything: `-Z build-std` for every MikuOS target, `-Z json-target-spec` for the custom targets | Nothing builds. The `std` overlay stops with `rust-src is not installed: rustup component add rust-src --toolchain nightly` |
| clang | C code in `std` programs (QuickJS in aosh, SQLite, zstd, tree-sitter in Zed) through `lib/sdk/c/miku-cc`; the CUDA host check | aosh does not build, so the build fails (aosh is required) |
| `llvm-ar` (LLVM) | Archiving that C code (`AR_x86_64_unknown_miku=llvm-ar`, set by `builder/src/stdport/compile.rs`) | as above |
| `nm` (binutils) | The check that every function a C header declares is defined by a MikuOS library (`builder/src/stdport/cheaders.rs`) | `std` programs do not build |
| Network access, sometimes | Crate sources the `std` overlay patches (`tokio`, `mio`, `inotify`, `libc`...) that cargo's automatic cache clean has deleted and its cache no longer has; the builder runs `cargo fetch` for them, offline first | aosh does not build, so the build fails; `stdapps` is left out |
| python3 | Only `dev/nvidia/fw_extract.py`, to extract the GPU firmware | No firmware from a new driver package |
| `grub-mkrescue`, `xorriso`, `mtools` | The ISO | The builder panics at once (`grub-mkrescue not found`) |
| GRUB's i386-pc files (`boot.img` in `/usr/lib/grub/i386-pc`, `/usr/share/grub/i386-pc` or `/lib/grub/i386-pc`) | The `mbr.bin`/`core.bin` the in-system `install` command writes to a disk | The image boots, but `install` has no bootloader to write |
| e2fsprogs (`mke2fs`, `debugfs`) | `boot/firmware.img` (the root store), `miku-os/disk.img`, fixing file ownership in both | The ISO carries no `/bin`, `/etc` or firmware, and the system boots with no programs |
| `dd` (coreutils) | Making empty image files | — |
| CUDA toolkit, optional (`nvcc`, `cuobjdump`, looked for on `PATH`, then `/opt/cuda/bin`, then `/usr/local/cuda/bin`) | Compiling the GPU programs in `kernel/drivers/gpu/nvidia/shaders/*.cu` for `sm_120`; needs a CUDA version that knows `sm_120` | The kernel builds with no GPU programs (the build prints `no nvcc - building without GPU programs (blur, shadows)`). On the card the compositor composes without the blur pass, and aosh draws on the processor |
| `qemu-system-x86_64` | Running | — |
| `strip` (binutils), `patch` | Only for Zed (`cargo run --release -- zed build -p zed` applies `zed-patches/` with `patch`; the image build strips the binary) | No Zed in the image |
| Qt 6 with qt6-5compat, `xvfb-run` | Only for aosh's Qt reference renders ([aosh.md](aosh.md#running-aosh-on-the-linux-host)) | The comparison tests pass over missing renders |

**Packages.** On Arch: `rustup clang llvm grub xorriso mtools e2fsprogs qemu-full`, plus `cuda` for the GPU programs. On Debian or Ubuntu: `clang llvm grub-pc-bin grub-common xorriso mtools e2fsprogs qemu-system-x86`, rustup from rustup.rs, and NVIDIA's CUDA packages (the distribution's `nvidia-cuda-toolkit` is usually too old for `sm_120`). The release workflow in `.github/workflows/release.yml` runs on a `v*` tag: it installs `clang llvm xorriso mtools grub-pc-bin grub-common e2fsprogs` on Ubuntu, runs the builder and publishes `miku-os/miku-os.iso`. That image has no NVIDIA firmware and no GPU programs.

**The Rust toolchain.** There is no `rust-toolchain` file. Every script and the builder call `cargo +nightly`, so the installed `nightly` is what builds. The `std` port is written against a reference clone of `library/` pinned to the toolchain's own commit; the tree is built with the nightly whose `rustc +nightly --version` reports `1.98.0-nightly (cb46fbb8c 2026-06-08)`, which is rustup's `nightly-2026-06-09`. The release workflow installs that one and renames its directory to `nightly`, the name the builder calls. A different nightly can compile `std` from different sources than the patch was written against ([../../lib/sdk/rust-std/README.md](../../lib/sdk/rust-std/README.md)). Install `rust-src` for it: `rustup component add rust-src --toolchain nightly`.

## Repository layout

| Path | Contents |
|---|---|
| `kernel/` | The kernel, one crate (`miku-os-release`, entry `kernel/main.rs`) |
| `kernel/arch/x86_64/` | Boot entry, early console (`earlycon.rs`), GDT, IDT, traps, FPU, APIC, ACPI, SMP, RTC, per-CPU data, the multiboot2 reader (`grub.rs`) |
| `kernel/mm/` | Frames, paging, the slab heap (`mm/heap/`), vmap, mmap, swap, TLB shootdown, kernel stacks, shrinkers ([memory.md](memory.md)) |
| `kernel/sched/` | Scheduler, run queues, wait queues, timers, futex, idle ([scheduler.md](scheduler.md)) |
| `kernel/process/` | Processes, threads, ELF loader, dynamic linking, signals, credentials, sessions, vDSO, crash reports ([processes.md](processes.md)) |
| `kernel/syscall/` | Dispatch, adapters, user copies, errno ([../MikuOS_ABI.md](../MikuOS_ABI.md)) |
| `kernel/fs/` | VFS, page cache, ext, btrfs, tmpfs, memfs, devfs, procfs, GPT ([vfs.md](vfs.md), [filesystems.md](filesystems.md)) |
| `kernel/io/` | Block layer, console, framebuffer, input, terminals |
| `kernel/net/` | Ethernet to TCP ([network.md](network.md)) |
| `kernel/drivers/` | `block/` (ATA, AHCI, NVMe, virtio-blk, RAM disk), `bus/` (PCI, MSI), `net/` (e1000, RTL8139, RTL8168, virtio-net), `input/` (PS/2, xHCI and USB HID), `gpu/` (NVIDIA, Bochs, KMS, EDID) |
| `kernel/mayland/` | The display server ([mayland.md](mayland.md)) |
| `kernel/mikud/` | The init daemon ([system.md](system.md)) |
| `kernel/kcore/` | Boot state, clock, locks, kernel log, pstore, backtraces, power, RNG, the firmware store (`kcore/firmware/`) |
| `kernel/shell/` | The in-kernel emergency shell and its commands; `run=` goes through it |
| `kernel/setup/` | The installer window |
| `lib/runtime/ld-miku/` | The dynamic loader, embedded in the kernel as `ld-miku.bin` |
| `lib/runtime/vdso/` | The vDSO page, embedded as `vdso.bin` |
| `lib/abi/` | The numbers and layouts everything agrees on: syscalls, errno, structures, flags, ELF. No dependencies |
| `lib/base/`, `lib/crypto/` | Pure algorithms, and the cryptography, shared by whichever layer needs them |
| `lib/buildgen/` | What build scripts and the builder share: the library list, the export scanner, stubs, shims, declarations |
| `lib/runtime/libmiku/` | The ten shared libraries (`core_miku` ... `libc_miku`), listed in `libs.list`, embedded in the kernel and preloaded at boot |
| `lib/sdk/miku/` | The crate every core-only program links; `x86_64-miku-app.json`, `linker.ld`, and the generated link stubs in `stubs/` (git-ignored) |
| `lib/sdk/rust-std/` | The `std` port: the `x86_64-unknown-miku` target, the `library/` patch, and Zed's patches and files. How they are assembled and built is `builder/src/stdport/` |
| `lib/sdk/rust-libc/`, `lib/rust-rustix/` | Overlays of the `libc` and `rustix` crates for the MikuOS target |
| `lib/sdk/c/` | `miku-cc` (clang for MikuOS), the C headers in `include/`, layout checks in `check/` |
| `lib/gfx/freetype/` | FreeType 2.14.3 for aosh |
| `lib/gfx/mayland/` | `libmay`, the display protocol library linked by the kernel and by clients |
| `lib/net/` | `mikunet`: DNS, TLS, HTTP for ring 3, and the protocol code the kernel stack shares |
| `lib/fs/extfs/`, `lib/fs/btrfs/`, `lib/fs/fshost/` | Filesystem formats and the trait they reach a disk through; host-testable |
| `lib/admin/mikuauth/` | Password hashing and the account file parsers |
| `lib/admin/mikupkg/` | The package manager as a library; the builder installs the image with it |
| `lib/buildgen/` | Code the kernel's `build.rs` includes: `imagegen.rs` (pictures compiled in), `qoigen.rs` (wallpapers, icons, the greeter portrait as files), `shadergen.rs` (GPU programs through `nvcc`), `cubin.rs` (the cubin parser the kernel also uses) |
| `system/` | `msh`, `mayterm`, `maywm`, `maygreet`, `maysession`, `mikudo`, `mikudod`, `login`, `passwd`, `id`, `su`, `useradd`, `mikupkg`, `mikupkg-apply` |
| `utils/` | The commands: `ls`, `cat`, `cp`, `grep`, `ps`, `mikuctl` and the rest ([commands.md](commands.md)) |
| `apps/` | Demos and ABI tests: `hello`, `maydemo`, `gpudemo`, `abitest`, `crashtest`, `threadtest`, `polltest`, and the network tools `curl`, `wget`, `host`, `ping`, `ntpdate`, `traceroute` |
| `stdapps/` | `std` test programs and `stdbench` ([testing.md](testing.md)) |
| `aosh/` | The QML desktop shell ([aosh.md](aosh.md)) |
| `etc/` | `/etc` in the image: accounts, `mayland/`, `aosh/`, `mikupkg/`, `ssl/certs/ca-bundle.pem`, `os-release` |
| `assets/` | Wallpapers, icons, the UI fonts |
| `builder/` | The build program and the package layout (`src/main.rs`, `src/pkg.rs`) |
| `docs/` | This documentation, the ABI and security references, the NVIDIA pages, translations of the README |
| `grub.cfg` | The boot menu ([boot.md](boot.md#the-grub-menu)) |
| `linker.ld`, `build.rs`, `Cargo.toml` | The kernel's linker script, build script and manifest |
| `dev/` | Design notes and decision records, one or more per subsystem. Git-ignored: present in the developer's tree only |
| `refsrc/` | Reference sources (OpenRM, nouveau, Linux, Hyprland, Quickshell, the Rust `library/`, Zed). Git-ignored |
| `personal/` | The machine owner's desktop files ([aosh.md](aosh.md#the-negi-desktop)). Git-ignored; never part of a public build |
| `kernel/drivers/gpu/nvidia/firmware/<card>/` | NVIDIA firmware blobs. Git-ignored; see [../Nvidia_gb206.md](../Nvidia_gb206.md) |
| `miku-os/` | Build output: the ISO and the disk images. Git-ignored |
| `target/` | Cargo output and the builder's intermediate trees. Git-ignored |

## The builder

```bash
cd builder
cargo run --release
```

The builder finds the repository from its own manifest directory (`CARGO_MANIFEST_DIR/..`), so it can be run from anywhere. It is a host program with its own cargo config (`builder/.cargo/config.toml`: target `x86_64-unknown-linux-gnu`, its own `target/`, and `build-std = ["std", "panic_unwind", "proc_macro", "test"]`, because cargo merges the root config's `build-std` list into it and only building every crate from source keeps one copy of `alloc`).

### What it asks

Every question has a timeout, after which the default is taken, so an unattended build runs through.

| When | Prompt | Default | Timeout |
|---|---|---|---|
| Start | `Low RAM mode? [y/N]` | no. Yes adds `--jobs 1` to every cargo build and `-C codegen-units=1` to the kernel | 10 s |
| Start | `Build the std programs (stdapps: stdbench, stdfs, stdtokio...)? Missing crate sources are downloaded [y/N]` | no: the image ships without `stdapps`' programs, and their crates are neither built nor fetched. aosh is built either way | 10 s |
| After the ISO, only when `miku-os/disk.img` does not exist | `disk.img size in MB (default 4096)` | 4096 | 30 s |
| After the ISO, only when `miku-os/data.img` does not exist | `Create data.img? [y/N]`, then `data.img size in MB (default 2048)` | no; 2048 | 15 s; 30 s |
| End | `Launch QEMU? [y/N]` | no | 10 s |

When both disk images exist, the disk questions are skipped and their current sizes are used.

### The steps, in order

| Step | Function | What it does | Output |
|---|---|---|---|
| 1 | `check_grub_mkrescue` | Checks `grub-mkrescue --version` | — |
| 2 | `build_ldmiku` | `lib/runtime/ld-miku`, target `x86_64-miku-ldso.json`, `-Z build-std=core` | `lib/runtime/ld-miku/ld-miku.bin` |
| 3 | `build_vdso` | `lib/runtime/vdso`, target `x86_64-miku-vdso.json`, `-shared`, `--hash-style=sysv` | `lib/runtime/vdso/vdso.bin` |
| 4 | `build_libmiku` | `lib/runtime/libmiku`, target `x86_64-miku-lib.json`, one `.so` per line of `libs.list` | `lib/runtime/libmiku/libs/*.so` |
| 5 | `build_userspace` | `system/`, `utils/`, `apps/` (core programs), then `stdapps/` and `aosh/` (`std` programs) | `<crate>/target/x86_64-miku-app/release/*`, `<crate>/target/x86_64-unknown-miku/release/*` |
| 6 | `build_kernel` | `cargo build --release -p miku-os-release --target x86_64-unknown-none -Z build-std=core,compiler_builtins,alloc`; the kernel's `build.rs` also renders wallpapers, icons and the greeter portrait and compiles the GPU programs | `target/x86_64-unknown-none/release/miku-os-release`, `target/{wallpapers,icons,art,gpu}/` |
| 7 | `create_iso` | Stages the root tree (below), makes `boot/firmware.img`, copies the kernel to `boot/kernel.elf`, copies `grub.cfg` and adds `module2 /boot/firmware.img firmware` after every `multiboot2` line, runs `grub-mkrescue`, removes `iso_root/` | `miku-os/miku-os.iso` |
| 8 | `provision_root_disk` | Creates `miku-os/disk.img` as ext4 from the same staging tree, or refreshes an existing one through `debugfs` | `miku-os/disk.img` |
| 9 | `ensure_disk` | Creates an empty `miku-os/data.img` if one was asked for | `miku-os/data.img` |
| 10 | — | Launches QEMU if asked ([below](#the-builders-own-qemu-line)) | — |

A failed build of `system/`, `utils/`, `apps/` or `aosh/` stops the builder: an image without them boots into a machine with no shell, no login and no desktop. `stdapps/` is optional: it is built only when the question above is answered yes, and if it fails, the image ships without its programs and the builder says so. A `stdapps` binary left over from an earlier build is not packaged when it was not built this time. Zed is never built by the image build: `cd builder && cargo run --release -- zed build -p zed` builds it into `target/zed-miku/`, and the image build strips that binary into `target/image-extern/zed` and packages it when it is there.

### Staging the root tree

`stage_root_tree` (called by steps 7 and 8) gathers what the build produced and hands it to `pkg::build_image` (`builder/src/pkg.rs`):

1. **Firmware.** Each folder in `kernel/drivers/gpu/nvidia/firmware/` is staged under `lib/firmware/<path>`: `rtx5060ti` goes to `nvidia/gb206`, `gtx1650` to `nvidia/tu116` (table `NVIDIA_FW_CARDS`), any other folder to `nvidia/<folder>`. The `MIKU_FW` environment variable picks which: unset or `all` stages everything; `none` stages nothing; `MIKU_FW=nvidia/gb206` (the form the kernel prints for each card it finds) or `MIKU_FW=rtx5060ti,gtx1650` stages those. Firmware left out is reported with its size. No firmware at all is normal for a fresh clone: the blobs are NVIDIA's and are not in git.
2. **Programs.** Every binary of every program crate, each assigned to a package in the `*_BINS` tables of `builder/src/main.rs`. A program crate with no build output, or a listed binary missing from it, stops the build.
3. **Packages.** `build_image` builds one `.mpk` container per package into `target/pkgs/<name>-<version>.mpk` and installs them all into `target/fw_root/` through `mikupkg`'s own applier, the same code a running machine uses, which also writes `/pkg/db`. The packages:

| Package | Contents |
|---|---|
| `miku-base` | The commands, `msh`, `mikupkg`, `mikupkg-apply`, `/etc/os-release`, `/etc/mikupkg/` |
| `miku-system` | `mikudo`, `mikudod`, `login`, `passwd`, `id`, `su`, `useradd`; `/etc/passwd`, `/etc/group`, `/etc/shadow` (mode 0640), `/etc/mikudo.conf` |
| `miku-desktop` | `maywm`, `mayterm`, `maygreet`, `maysession`, `aosh`; `/etc/mayland/` with wallpapers, icons and the greeter portrait from `target/`; `/etc/gpu/` (the compiled GPU programs from `target/gpu/`); `/etc/aosh/` with the fonts from `assets/` |
| `miku-net` | The network tools; `/etc/ssl/` |
| `miku-demos` | The demos, ABI tests and `std` test programs |
| `zed` | Zed, when it was built |
| `miku-firmware` | `/lib/firmware/...` |
| `miku-personal` | Only when `personal/` exists ([aosh.md](aosh.md#the-negi-desktop)) |

A `conf.d` directory under `etc/` ships empty even when the source tree has files in it. A file under `etc/` that no package claims stops the build. A package with nothing in it is skipped.

4. **Kernel and bootloader.** The kernel is copied to `boot/kernel.elf` in the staging tree (the `install` command copies it from there), and `build_boot_blobs` makes `boot/grub/i386-pc/mbr.bin` (440 bytes) and `core.bin` with `grub-mkimage -O i386-pc -p (,gpt2)/boot/grub`, patched for core.img at LBA 2048.

### The images

| Image | Made by | Format | Contents |
|---|---|---|---|
| `boot/firmware.img` inside the ISO | `build_firmware_module` | ext2, no journal, no `resize_inode`, no `dir_index`, 4 KiB blocks, one inode per 64 KiB and at least what the tree holds; the staged size plus 5% (at least 4 MiB) of slack, at least 40 MB | The whole staging tree. GRUB loads it as the multiboot2 module named `firmware`; the kernel mounts it read-only as a RAM disk and takes `/bin`, `/etc`, `/pkg` and `/lib/firmware` from it ([boot.md](boot.md)) |
| `miku-os/disk.img` | `provision_root_disk` | ext4 with the e2fsprogs defaults (journal, extents, `dir_index`, `metadata_csum`) except `^resize_inode` | The same tree. Used only when the boot line names it with `root=` |
| `miku-os/data.img` | `ensure_disk` | Zeroes | Nothing; an extra drive for experiments |

`mke2fs -d` copies each file's owner from the build host, where it is usually uid 1000, which is MikuOS's `miku`. The builder resets every file on both images to the owner the package gave it with `debugfs sif` (`chown_image_to_root`). An existing `disk.img` is refreshed in place with a `debugfs` script (every directory made, every file removed and written again, with mode and owner); the firmware is skipped when it is already there.

## Targets and cargo configuration

| Target | Spec | Used by | Built with |
|---|---|---|---|
| `x86_64-unknown-none` | built into rustc | The kernel | Root `.cargo/config.toml`: `build-std = core, compiler_builtins, alloc`, `compiler-builtins-mem`; rustflags `-Tlinker.ld --no-dynamic-linker -C relocation-model=static -C force-frame-pointers=yes` |
| `x86_64-miku-app` | `lib/sdk/miku/x86_64-miku-app.json` | Core-only programs (`system`, `utils`, `apps`) | `-Z build-std=core,alloc`; PIE (`-C relocation-model=pic -C link-arg=-pie`), `-z noexecstack`, no red zone, frame pointers, `--build-id=sha1` |
| `x86_64-unknown-miku` | `lib/sdk/rust-std/x86_64-unknown-miku.json` | `std` programs (`stdapps`, `aosh`, Zed) | `builder/src/stdport/`: `-Z build-std=std,panic_abort` from the patched `library/` |
| `x86_64-miku-ldso` | `lib/runtime/ld-miku/x86_64-miku-ldso.json` | The dynamic loader | `-Z build-std=core`, linked with `lib/runtime/ld-miku/ld_link.ld` |
| `x86_64-miku-vdso` | `lib/runtime/vdso/x86_64-miku-vdso.json` | The vDSO | `-Z build-std=core`, `-shared`, `lib/runtime/vdso/vdso.ld` |
| `x86_64-miku-lib` | `lib/runtime/libmiku/x86_64-miku-lib.json` | The shared libraries | `-Z build-std=core` |

**The workspace.** The root `Cargo.toml` is the kernel package (`miku-os-release`, version 1.0.2, release profile `opt-level = 2`, `lto = true`, `panic = "abort"`) and a workspace whose members are `lib/runtime/ld-miku`, `lib/gfx/mayland`, `lib/sdk/miku` and `lib/runtime/vdso`. Crates that are tested on the host (`lib/net`, `lib/fs/extfs`, `lib/fs/fshost`, `lib/fs/btrfs`, `lib/admin/mikuauth`, `lib/admin/mikupkg`, `lib/runtime/libmiku/hosttest`, `kernel/mm/heap/hosttest`) and `builder` are excluded and have their own configs. Program crates (`system`, `utils`, `apps`, `stdapps`, `aosh`) are separate workspaces with their own `target/`.

**The kernel's `build.rs`** generates the preload table of shared libraries from `lib/runtime/libmiku/libs.list` (so the kernel and the builder agree on the set), compiles the mascot picture into the kernel, writes wallpapers and icons as QOI files to `target/wallpapers` and `target/icons`, cuts the greeter portrait to a circle in `target/art`, and runs `shadergen::generate` over `kernel/drivers/gpu/nvidia/shaders/` for `sm_120`, printing one `cargo:warning` line per GPU program with its size, registers, shared memory and constant bank. A `.cu` file can ask for compiler flags on its first line (`// nvcc: --fmad=false`, which `paint.cu` uses). See [../Nvidia_gb206.md](../Nvidia_gb206.md) for the programs.

**What the kernel embeds.** `lib/runtime/ld-miku/ld-miku.bin`, `lib/runtime/vdso/vdso.bin` and every `lib/runtime/libmiku/libs/<name>.so` are compiled into the kernel with `include_bytes!`. They have to exist before the kernel builds, which is why the builder builds them first.

## How each program set is built

**Core programs** (`system/`, `utils/`, `apps/`), as `build_program_crate` does it:

```bash
cd system        # or utils, apps
RUSTFLAGS="-C relocation-model=pic -C link-arg=-pie -C link-arg=-z -C link-arg=noexecstack \
  -C no-redzone=y -C force-frame-pointers=yes -C link-arg=--build-id=sha1" \
cargo +nightly build --release --target ../lib/sdk/miku/x86_64-miku-app.json \
  -Z json-target-spec -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  --bin msh
```

The binaries land in `<crate>/target/x86_64-miku-app/release/`. A plain `cargo build` inside one of these crates does not do this: the root `.cargo/config.toml` targets the kernel and is inherited.

**`std` programs** (`stdapps/`, `aosh/`):

```bash
cd builder
cargo run --release -- std stdapps            # or aosh, or any crate directory below the root
cargo run --release -- std aosh --check       # cargo check instead of build
cargo run --release -- std stdapps --bin stdfs   # anything after the crate (and --check) goes to cargo
cargo run --release -- overlay                # only assemble target/rust-src-miku
```

This is the code the image build runs, `builder/src/stdport/` (it used to be `lib/sdk/rust-std/build-std.sh`, `overlay.sh` and five Python scripts). In order:

1. **Sources** (`sources.rs`). Cargo deletes an extracted crate source it has not used for a month, and a downloaded `.crate` after three. The overlay's crates are only ever built from their patched copies, so cargo never counts them as used. For every crate the overlay patches, at the version the consumer's `Cargo.lock` pins, a source that is gone is brought back with `cargo fetch` on a throwaway manifest: offline first, which unpacks what the cache still has, then over the network. Cargo records the use, so the next clean leaves them alone.
2. **Overlay** (`overlay.rs`, `arms.rs`). The toolchain's own `rust-src` `library/` is copied to `target/rust-src-miku/library` and `lib/sdk/rust-std/library/` is laid over it. `libc` (newest 0.2 on the machine, plus `lib/sdk/rust-libc/libc/`) and `rustix` (newest 1.x) get their arms. Every crate in the `arms` table gets MikuOS beside its neighbour, at every semver line on the machine. Copies keep modification times and skip unchanged files, so cargo does not rebuild what did not change.
3. **Link stubs.** `lib/sdk/miku` is built once so its build script refreshes `lib/sdk/miku/stubs/`.
4. **Stamp.** The patch tree and the target spec are hashed, and `<crate>/target/x86_64-unknown-miku` is deleted when either changed (cargo's `-Z build-std` fingerprints the sources by path and would relink the old `std`).
5. **C headers** (`cheaders.rs`). `target/miku-c/include/bits/<file>.h` is generated from `lib/abi/src/<file>.rs`, every function a header declares is checked against the stubs with `nm`, and `std`'s and `libc`'s own declarations are checked against the definitions (`lib/buildgen/src/check.rs`).
6. **cargo.** `CC`/`AR`/`CFLAGS` for the target are set to `lib/sdk/c/miku-cc` and `llvm-ar`. Then `cargo +nightly build --release --target lib/sdk/rust-std/x86_64-unknown-miku.json -Z build-std=std,panic_abort` runs, with the linker script and stubs passed by absolute path.

Output: `<crate>/target/x86_64-unknown-miku/release/`. See [sdk.md](sdk.md) for writing such programs.

**Zed:** `cargo run --release -- zed build -p zed` (or `zed check -p <crate>`, `zed tree ...`) does the following:
- copies `refsrc/miku_programs/zed` to `target/zed-miku/src`;
- applies `lib/sdk/rust-std/zed-patches/` with `patch` and copies in `lib/sdk/rust-std/zed-files/`;
- puts MikuOS into Zed's own target lists;
- assembles its own overlay in `target/zed-miku/rust-src` from the versions Zed's lock pins, and moves the lock onto them;
- builds into `target/zed-miku/target` with a profile a 16 GB machine can build (`ZED_JOBS`, `ZED_FULL_PROFILE=1` for upstream's).

## Building pieces on their own

**The kernel alone**, from the repository root, after the builder has run once (so the embedded binaries exist):

```bash
cargo build --release -p miku-os-release
```

The root config supplies the target, `build-std` and the rustflags. The builder instead sets `RUSTFLAGS` to `-C relocation-model=static -C link-arg=-Tlinker.ld -C link-arg=--no-dynamic-linker` (plus `-C codegen-units=1` in low-RAM mode); an environment `RUSTFLAGS` replaces the config's list, so the builder's kernel is built without `-C force-frame-pointers=yes`.

**A fast ISO without the builder** (the kernel changed, nothing else did): extract or keep an `iso_root` with `boot/firmware.img`, copy `target/x86_64-unknown-none/release/miku-os-release` to `iso_root/boot/kernel.elf`, put a `grub.cfg` with the `module2 /boot/firmware.img firmware` lines in `iso_root/boot/grub/`, and run `grub-mkrescue -o test.iso iso_root`. The same copy with a patched `multiboot2` line is how a one-off boot line is tested ([testing.md](testing.md)).

**One ring-3 program into an existing ISO.** `/bin` comes from `boot/firmware.img`, so replacing a program means replacing it in that image: `chmod u+w` the extracted image, then `debugfs -w` with `rm /bin/<name>` and `write <binary> <name>` in `/bin`, then `grub-mkrescue` again. Check the staged binary with `strings` for something just added before trusting a run.

**Host tests** have their own commands per crate; see [testing.md](testing.md#host-tests).

## What ends up on the machine

| Path at run time | Comes from |
|---|---|
| The kernel, ld-miku, the vDSO, the shared libraries | `boot/kernel.elf` on the ISO (the others are inside it) |
| `/bin`, `/etc`, `/pkg`, `/lib/firmware` | `boot/firmware.img`, mounted read-only; with `root=` (and `rootrw`), `/bin`, `/etc` and `/pkg` come from that disk instead |
| `/`, `/tmp`, `/home` and the rest | tmpfs, rebuilt every boot ([filesystems.md](filesystems.md)) |

So a live boot never writes the ISO's files, account changes do not survive a reboot unless a disk is named with `root=... rootrw`, and `~/.config` lives in RAM. [boot.md](boot.md) describes the mount sequence.

## Running in QEMU

**Minimal:**

```bash
qemu-system-x86_64 -enable-kvm -cpu host -m 4G -boot d -cdrom miku-os/miku-os.iso -serial stdio
```

**The README's line**, with four CPUs and a virtio network card:

```bash
qemu-system-x86_64 -enable-kvm -cpu host -smp 4 -m 4G \
  -boot d -cdrom miku-os/miku-os.iso \
  -serial stdio \
  -netdev user,id=n0 -device virtio-net-pci,netdev=n0
```

The GRUB menu shows for three seconds, then boots. The serial port is output only: the kernel never reads it, so typing goes through the QEMU window or the monitor's `sendkey`.

| Topic | How |
|---|---|
| KVM vs TCG | `-enable-kvm` needs `/dev/kvm` readable and writable. Under KVM the system is at the prompt in a couple of seconds; under TCG it takes on the order of a minute and a half, and in-guest frame timings spread widely, so render cost is measured on the host ([aosh.md](aosh.md#performance)). The builder probes `/dev/kvm` itself, because `qemu -enable-kvm -version` succeeds without KVM |
| CPU model | `-cpu host` passes the host's SMEP and SMAP through, and the kernel turns them on ([boot.md](boot.md)). `qemu64` has neither; the developer's stress harness uses it deliberately |
| SMP | `-smp N`. Two- and four-CPU desktops run (seven of seven two-CPU desktop boots passed on 2026-10-14) |
| Memory | 4 GiB is the usual size. tmpfs may take a quarter of RAM |
| Network | `-netdev user,id=n0 -device virtio-net-pci,netdev=n0` gives virtio-net with MSI-X. The shorthand `-nic user,model=virtio-net-pci` gives a card with no MSI-X, which the driver polls every tick. With no network option QEMU adds an e1000: it works, but every TCP connect through QEMU's user network costs about 500 ms on that model, and that delay is QEMU's ([network.md](network.md)) |
| USB | `-device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0 -device usb-tablet,bus=xhci.0`. The tablet is an absolute pointer, so the host cursor and the guest cursor are one. To prove the USB keyboard path rather than PS/2, remove the emulated 8042 with the QEMU machine option `-machine q35,i8042=off` (this is QEMU's option, not a kernel flag). QEMU's xHCI does not exercise port power or the BIOS hand-off ([input-and-terminals.md](input-and-terminals.md)) |
| Disks | See [below](#disk-images-and-the-qemu-drive-layout) |
| Headless | `-display none -serial file:serial.log -monitor stdio` (or `-monitor unix:mon.sock,server,nowait`). At the monitor, `sendkey` types and `screendump out.ppm` saves the screen |
| BIOS vs UEFI | Boot with SeaBIOS (QEMU's default). Under UEFI (OVMF/edk2) GRUB 2.14's own `multiboot2` command page-faults inside the GRUB image, because recent edk2 maps loaded image sections read-only. A minimal multiboot2 stub fails the same way, so it is not the kernel; established on 2026-07-26 with grub 2.14-1, edk2-ovmf 202605-1, QEMU 11.0.2. UEFI would need another loader or a fixed GRUB |
| Display | The kernel drives QEMU's standard VGA through Bochs DISPI and takes the mode from the EDID ([mayland.md](mayland.md)). QEMU has no NVIDIA card, so nothing of [../Nvidia_gb206.md](../Nvidia_gb206.md) runs there, and Mayland's compute pass (blur, borders, VRAM copies) is absent |

**Always pass `-boot d`** when a disk is attached. With `-cdrom` and a bootable disk, QEMU prefers the disk; a disk that MikuOS was once installed on then boots its own, older kernel, and the run looks like a build that did not take.

### The builder's own QEMU line

Answering yes to `Launch QEMU?` runs:

```
qemu-system-x86_64 -boot d -cdrom miku-os/miku-os.iso
  -drive file=miku-os/disk.img,format=raw,if=none,id=disk0,cache=unsafe,aio=threads
  -device ide-hd,drive=disk0,bus=ide.0,unit=1,rotation_rate=1
  -serial stdio -display gtk -m <80% of free host RAM, at least 512M>
  -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0 -device usb-tablet,bus=xhci.0
  [-drive file=miku-os/data.img,...,id=disk1 -device ide-hd,drive=disk1,bus=ide.1,unit=1,rotation_rate=1]
  [-enable-kvm]
```

It adds no network option (so QEMU's default e1000 is used) and no `-smp` (one CPU).

## Disk images and the QEMU drive layout

The builder's layout puts the CD on QEMU's default IDE position, `disk.img` on the primary channel as unit 1, and `data.img` on the secondary channel as unit 1. Other drives QEMU can give the kernel drivers for:

```bash
# virtio-blk
-drive file=disk.img,format=raw,if=virtio
# NVMe
-drive file=disk.img,format=raw,if=none,id=nv0 -device nvme,serial=miku0,drive=nv0
# AHCI (SATA)
-device ahci,id=ahci -drive file=disk.img,format=raw,if=none,id=sd0 -device ide-hd,drive=sd0,bus=ahci.0
```

**No disk is ever mounted unless the boot line names it.** Attaching `disk.img` does not make it the root: without `root=`, the kernel says `no root= on the boot line; no disk mounted automatically` and runs from the boot image. To use a disk, add `root=<drive>[:<lba>]` or `root=LABEL=<name>` to the GRUB line (press `e` at the menu), and `rootrw` to mount it writable ([boot.md](boot.md#kernel-command-line-reference)). Drive numbers are the kernel's own, in probe order; the storage commands list them ([storage.md](storage.md)). btrfs tests expect a `miku-os/btrfs.img` attached as a drive ([testing.md](testing.md)).

## Real hardware and USB sticks

The ISO from `grub-mkrescue` is a hybrid image, so it can be written to a USB stick as it is:

```bash
sudo dd if=miku-os/miku-os.iso of=/dev/sdX bs=4M status=progress conv=fdatasync
```

Check the device name with `lsblk` first: `dd` overwrites whatever `/dev/sdX` is.

- **Boot mode.** Boot the stick in legacy BIOS (CSM) mode; UEFI fails in GRUB as described above.
- **The menu.** It shows for three seconds; press `e` there to add a flag for one boot ([boot.md](boot.md#the-boot-line)). At a timeout of zero GRUB boots at once and never looks at a key.
- **Firmware.** GRUB reads the whole `firmware.img` into RAM before the kernel starts, so on a USB stick its size is boot time. Each NVIDIA card the kernel finds prints the `MIKU_FW=` value for its firmware; build with that to leave other cards' firmware out. The developer's measurement: the ISO went from 143 MB to 114 MB with `MIKU_FW=nvidia/gb206`.
- **Serial.** Real machines rarely have a serial port wired; use the screen, the kernel log (`dmesg`) and the persistent store across a warm reboot ([diagnostics.md](diagnostics.md)).
- **The installer.** The image can install itself to a disk from inside the running system with the GPT layout and the GRUB blobs staged above; see [system.md](system.md).
- **Real hardware the tree is developed on:** the RTX 5060 Ti machine ([../Nvidia_gb206.md](../Nvidia_gb206.md)).

## Common build problems

| Symptom | Cause and fix |
|---|---|
| `rust-src is not installed` | `rustup component add rust-src --toolchain nightly` |
| `grub-mkrescue not found` (panic at start) | Install GRUB, `xorriso` and `mtools` |
| `mke2fs missing - ISO will NOT carry /lib/firmware, /bin or a bootloader` | Install e2fsprogs. An ISO built without it boots with an empty `/bin` |
| `no nvcc - building without GPU programs (blur, shadows)` | The CUDA toolkit is not found on `PATH`, `/opt/cuda/bin` or `/usr/local/cuda/bin`. The build continues; the card runs without GPU programs |
| `no NVIDIA firmware in .../firmware - image will boot without GPU firmware` | Normal on a fresh clone. To add it, see [../Nvidia_gb206.md](../Nvidia_gb206.md#firmware) |
| `<crate>/ failed to build - the image would ship without those programs` | A program crate failed; the error above it is cargo's |
| A `cargo build` inside `system/` or `apps/` produces a small static file in `target/x86_64-unknown-none/` | The root config's kernel target was inherited. Build with the command above, or run the builder |
| A change to `lib/sdk/rust-std/library/` is not in the binary | The `std` step should notice through its hash stamp (`std patch changed - rebuilding it`); if a build was interrupted, delete `<crate>/target/x86_64-unknown-miku` |
| A new `libmiku` function links but is missing at run time | The stubs in `lib/sdk/miku/stubs/` were stale. The `std` step rebuilds `lib/sdk/miku` first for this reason; for core programs, build `lib/sdk/miku` before them |
| `failed to read .../target/rust-src-miku/<crate>-miku/Cargo.toml` | The overlay found no source of that crate. The builder says `still no source for: ...` above it: cargo's cache no longer has the version the lock pins and the download failed. Run again with network access |
| `<crate> <file>: no ... found - the crate has changed shape` | A crate the overlay patches has a new release whose file no longer has the anchor a row of `builder/src/stdport/arms.rs` looks for. Update that row |
| The ISO appears to run old code | QEMU booted an attached disk: add `-boot d`. Or `/bin` was replaced on a disk that `root=` does not name, while the boot image still serves `/bin` |
| `cat /etc/shadow` works as `miku`, or `mikudo` refuses `/etc/mikudo.conf` | Files on an image owned by uid 1000 from the build host. The builder's `debugfs sif` pass fixes this; a staging step added elsewhere needs the same |
| `disk.img not writable by debugfs (MikuOS-formatted ext?)` | A `disk.img` formatted by MikuOS itself may use features `debugfs` will not write. The builder leaves it as it is; refresh it from inside MikuOS or recreate it (which loses what is on it) |
| The build runs out of memory | Answer `y` to `Low RAM mode?` |
| `kernel stack overflow` panic after a change | Not a build problem: boot with `bigstacks` ([boot.md](boot.md)) |
