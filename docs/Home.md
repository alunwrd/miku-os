<div align="center">

<img src="https://raw.githubusercontent.com/alunwrd/miku-os/main/docs/miku.png" width="220" alt="Miku Logo">

# MikuOS Wiki

**An experimental OS written in Rust.**

![Version](https://img.shields.io/badge/version-0.2.4--rc-57c5bb?style=for-the-badge)
![Language](https://img.shields.io/badge/language-Rust-orange?style=for-the-badge&logo=rust)
![Arch](https://img.shields.io/badge/arch-x86--64-blueviolet?style=for-the-badge)
![License](https://img.shields.io/badge/license-MIT-green?style=for-the-badge)

</div>

---

## Table of Contents

| Section | Description |
|---|---|
| [Building](building.md) | Dependencies, toolchain, how to build and run |
| [Disk Images](disk-images.md) | Disk setup and partitioning |
| [GPT Partitioning](gpt-partitioning.md) | Creating and managing GPT partition tables |
| [Formatting](formatting.md) | Formatting partitions with ext2/ext3/ext4 |
| [Swap](swap.md) | Setting up and using swap |
| [Storage & Block Layer](storage--block-layer.md) | Block devices, buffer cache, blkstat, AHCI/NVMe/virtio-blk |
| [Mounting Filesystems](mounting-filesystems.md) | Mounting disks, multi-mount slots, switching |
| [Unified Ext Commands](unified-ext-commands.md) | Auto-version ext commands (recommended) |
| [VFS Commands](vfs-commands.md) | Core filesystem navigation and file operations |
| [Ext2 Commands](ext2-commands.md) | Direct ext2 filesystem operations |
| [Ext3 Commands](ext3-commands.md) | Journaling and recovery |
| [Ext4 Commands](ext4-commands.md) | Extents, checksums, advanced features |
| [Network Commands](network-commands.md) | Networking: ping, fetch, traceroute, NTP, DHCP |
| [/proc Filesystem](proc-virtual-filesystem.md) | System info via virtual files |
| [/dev Filesystem](dev-virtual-filesystem.md) | Device files |
| [Mount Commands](mount-commands.md) | Mounting and unmounting filesystems |
| [System Commands](system-commands.md) | Power, memory, system info |
| [Memory Management](memory-management.md) | PMM, VMM, swap internals |
| [Scheduler](scheduler.md) | Preemptive multitasking internals |
| [Syscall Interface](syscall-interface.md) | Kernel syscall ABI |
| [libmiku.so](libmikuso-standard-library.md) | Standard library: 63 modules, 956 functions |
| [Network Stack](network-stack.md) | Drivers, protocols, TLS 1.2 / 1.3 |
| [ELF Loader](elf-loader-and-dynamic-linking.md) | ELF parsing, dynamic linking, ld-miku |
| [mikuD Init](mikud-init-daemon.md) | systemd-like service supervisor (PID 1) |
| [Userspace SDK](userspace-sdk.md) | Rust/C SDK for building userspace programs |
| [NVIDIA GPU](nvidia-gpu-commands.md) | GPU probe, Falcon engines, GSP-RM pipeline |
| [Color Macros](color-macros.md) | Developer output macros |
| [Serial Output Tags](serial-output-tags.md) | Boot log tag reference |

---

<div align="center">

*MikuOS v0.2.4-rc - Built with love in Rust*

</div>
