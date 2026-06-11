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
| [Building](#building) | Dependencies, toolchain, how to build and run |
| [Disk Images](#disk-images) | Disk setup and partitioning |
| [GPT Partitioning](#gpt-partitioning) | Creating and managing GPT partition tables |
| [Formatting](#formatting) | Formatting partitions with ext2/ext3/ext4 |
| [Swap](#swap) | Setting up and using swap |
| [Storage & Block Layer](#storage--block-layer) | Block devices, buffer cache, blkstat, AHCI/NVMe/virtio-blk |
| [Mounting Filesystems](#mounting-filesystems) | Mounting disks, multi-mount slots, switching |
| [Unified Ext Commands](#unified-ext-commands) | Auto-version ext commands (recommended) |
| [VFS Commands](#vfs-commands) | Core filesystem navigation and file operations |
| [Ext2 Commands](#ext2-commands) | Direct ext2 filesystem operations |
| [Ext3 Commands](#ext3-commands) | Journaling and recovery |
| [Ext4 Commands](#ext4-commands) | Extents, checksums, advanced features |
| [Network Commands](#network-commands) | Networking: ping, fetch, traceroute, NTP, DHCP |
| [/proc Filesystem](#proc-virtual-filesystem) | System info via virtual files |
| [/dev Filesystem](#dev-virtual-filesystem) | Device files |
| [Mount Commands](#mount-commands) | Mounting and unmounting filesystems |
| [System Commands](#system-commands) | Power, memory, system info |
| [Memory Management](#memory-management) | PMM, VMM, swap internals |
| [Scheduler](#scheduler) | Preemptive multitasking internals |
| [Syscall Interface](#syscall-interface) | Kernel syscall ABI |
| [libmiku.so](#libmikuso-standard-library) | Standard library: 63 modules, 956 functions |
| [Network Stack](#network-stack) | Drivers, protocols, TLS 1.2 / 1.3 |
| [ELF Loader](#elf-loader-and-dynamic-linking) | ELF parsing, dynamic linking, ld-miku |
| [mikuD Init](#mikud-init-daemon) | systemd-like service supervisor (PID 1) |
| [Userspace SDK](#userspace-sdk) | Rust/C SDK for building userspace programs |
| [NVIDIA GPU](#nvidia-gpu-commands) | GPU probe, Falcon engines, GSP-RM pipeline |
| [Color Macros](#color-macros) | Developer output macros |
| [Serial Output Tags](#serial-output-tags) | Boot log tag reference |

---

<div align="center">

*MikuOS v0.2.4-rc - Built with love in Rust*

</div>