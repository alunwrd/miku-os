# Building

## System Dependencies

<details>
<summary><b>Debian / Ubuntu</b></summary>

```bash
sudo apt update && sudo apt install -y \
    git curl build-essential \
    qemu-system-x86 xorriso \
    mtools dosfstools nasm lld clang
```

</details>

<details>
<summary><b>Arch Linux</b></summary>

```bash
sudo pacman -S --needed \
    git base-devel \
    qemu-system-x86 xorriso \
    mtools dosfstools nasm lld clang
```

</details>

<details>
<summary><b>macOS (Homebrew)</b></summary>

```bash
brew install qemu xorriso mtools nasm llvm
```

</details>

## Rust Toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

rustup toolchain install nightly
rustup override set nightly
rustup component add rust-src llvm-tools-preview
rustup target add x86_64-unknown-none
```

## GRUB Bootloader

MikuOS uses [GRUB](https://www.gnu.org/software/grub/) with a **Multiboot2** header.

## Build

```bash
cd builder
cargo run
```

The builder will:

1. Compile `ld-miku.so` (dynamic linker)
2. Compile `libmiku.so` (standard library, 956 functions across 63 modules)
3. Compile the kernel (`cargo build`) targeting `x86_64-unknown-none`
4. Assemble a bootable ISO with GRUB
5. Ask for disk image sizes (first run only)
6. Output `miku-os.iso` ready for QEMU
7. Optionally launch QEMU

## Run in QEMU

```bash
# Basic run
qemu-system-x86_64 -cdrom miku-os.iso -m 128M -serial stdio -no-reboot

# With disk attached
qemu-system-x86_64 \
    -cdrom miku-os.iso \
    -drive file=disk.img,format=raw,if=ide,index=1 \
    -m 128M -serial stdio -no-reboot

# With network (E1000 auto-detected)
qemu-system-x86_64 \
    -cdrom miku-os.iso \
    -drive file=disk.img,format=raw,if=ide,index=1 \
    -m 128M -serial stdio -no-reboot \
    -net nic,model=e1000 -net user

# With AHCI (SATA), NVMe and virtio-blk disks - probed at boot,
# they appear as block devices 4-7 (see blkstat)
qemu-system-x86_64 \
    -cdrom miku-os.iso \
    -drive file=disk.img,format=raw,if=ide,index=1 \
    -device ahci,id=ahci0 \
    -drive file=sata.img,format=raw,if=none,id=sata0 \
    -device ide-hd,drive=sata0,bus=ahci0.0 \
    -drive file=nvme.img,format=raw,if=none,id=nvme0 \
    -device nvme,drive=nvme0,serial=miku001 \
    -drive file=vblk.img,format=raw,if=none,id=vblk0 \
    -device virtio-blk-pci,drive=vblk0,disable-modern=on \
    -m 2G -serial stdio -no-reboot
```

> `-serial stdio` shows kernel boot logs: `[kern]`, `[int]`, `[ext3]`, `[net]`, `[heap]`, `[swap]`, etc.

## Cargo Dependencies

| Crate | Purpose |
|---|---|
| `x86_64` | x86-64 hardware abstractions |
| `pic8259` | Chained 8259 PIC interrupt controller |
| `pc-keyboard` | PS/2 keyboard scancode decoder |
| `spin` | `no_std` spinlock Mutex |
| `lazy_static` | Lazy statics for `no_std` |
| `linked_list_allocator` | Heap allocator |
| `volatile` | Volatile memory access |
| `noto-sans-mono-bitmap` | Fallback bitmap font |
| `log` | Logging facade |
| `fontdue` *(build)* | JetBrains Mono rasterization at build time |

## Common Issues

**`error: no such target: x86_64-unknown-none`**
Run `rustup target add x86_64-unknown-none`

**`can't find crate for 'core'`**
Run `rustup component add rust-src`

**`ext4mount` - no device found**
Make sure you passed `-drive file=disk.img,format=raw,if=ide`

**Keyboard not working in QEMU**
Click inside the QEMU window, or use `-display gtk`

**No network in QEMU**
Make sure QEMU has network access. E1000 is detected automatically.

**Swap not working**
Make sure a second ATA drive is attached and `swapon` was called.

---

## Color Macros

| Macro | Color | RGB |
|---|---|---|
| `print_error!(...)` | Red | `(255, 50, 50)` |
| `print_success!(...)` | Green | `(100, 220, 150)` |
| `print_info!(...)` | Cyan | `(128, 222, 217)` |
| `print_warn!(...)` | Yellow | `(220, 220, 100)` |

---

## Serial Output Tags

| Tag | Module | Example |
|---|---|---|
| `[kern]` | Boot / main | `[kern] poweroff` |
| `[boot]` | Boot steps | `[boot] ok  gdt` |
| `[heap]` | Heap allocator | `[heap] 32768 KB initialized` |
| `[gdt]` | GDT/TSS | `[gdt] IST page_fault configured` |
| `[int]` | Interrupts | `[int] loading idt` |
| `[sched]` | Scheduler | `[sched] spawn pid=2 name=shell` |
| `[swap]` | Swap I/O | `[swap] swap_out: phys=0x... -> slot=3` |
| `[swap_map]` | Swap eviction | `[swap_map] evicted virt=0x... slot=3 phys=0x...` |
| `[mkfs]` | Formatter | `[mkfs] done: 786432 blks 32 grps` |
| `[miku_extfs]` | Filesystem | `[miku_extfs] slot 0 drive 1 lba=524322 - found!` |
| `[ext3]` | Journal | `[ext3] recovery done: replayed=4 blocks` |
| `[net]` | Network | `[net] init: e1000 ok` |
| `[e1000]` | E1000 driver | `[e1000] init: done` |
| `[syscall]` | Syscall init | `[syscall] MikuOS native table ready` |
| `[vfs]` | VFS | `[vfs] init done` |
| `[gpt]` | GPT | `[gpt] init: 8388607 sectors` |
| `[timing]` | Perf | `[timing] ext4write disk=12ms` |
| `[io]` | ATA I/O | `[io] ata_commands=4` |
| `[pdflush]` | Periodic sync | `[pdflush] synced 8 dirty blocks` |
| `[mmap]` | Memory mapping | `[mmap] 0x10004a000+0x10000 prot=3` |
| `[dynlink]` | Dynamic linker | `[dynlink] ...` |
| `[solib]` | Shared libraries | `[solib] mapped 'libmiku.so' at 0x100000000` |
| `[ld-miku]` | Dynamic linker | `[ld-miku] shared: @ 0x4001c0` |
| `[elf]` | ELF loader | `[elf] EXEC entry=0x4001c0 bias=0x0` |
| `[exec]` | Process exec | `[exec] spawned pid=7 from 'hello'` |
| `[vfs_read]` | File reader | `[vfs_read] read 14216 bytes from 'test_dynamic'` |
| `[random]` | ASLR/RNG | `[random] RDRAND available` |