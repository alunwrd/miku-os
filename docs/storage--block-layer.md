## Storage & Block Layer

Every disk access in MikuOS goes through a Linux-style block layer with a
shared buffer cache. Filesystems, GPT, swap and mkfs never touch a driver
directly.

```
ext2/3/4 -> VFS -> buffer cache -> block layer -> ATA | AHCI | NVMe | virtio-blk
```

### Device IDs

| ID | Backend |
|---|---|
| **0-3** | Legacy ATA slots (primary/secondary x master/slave) |
| **4-7** | PCI devices probed at boot: AHCI ports, NVMe namespaces, virtio-blk |

### blkstat

`blkstat` is the one-stop view of the storage stack (lsblk + iostat):

```bash
blkstat
# Block layer devices
# blk1 [ata]:  'QEMU HARDDISK' 8388608 sectors (4096 MB) lba48=true ro=false
#       io: read 284 KB, written 100 KB
# blk4 [ahci]: 'QEMU HARDDISK' 524288 sectors (256 MB) lba48=true ro=false
#       io: read 32 KB, written 1028 KB
# bio queue: submitted=305 completed=305 errors=0
# buffer cache: hits=282 misses=23 (92% hit rate), readaheads=8, dirty=0
```

### Buffer Cache

| Parameter | Value |
|---|---|
| **Size** | 2 MiB: 512 x 4 KiB chunks, 8-way set-associative, per-set LRU |
| **Reads** | Whole-chunk read-around; sequential misses pull a 32 KiB readahead window |
| **Writes** | Write-back: data lands in RAM and the call returns |
| **Barriers** | ext3 journal records, GPT tables and swap headers use an ordered write-through path |
| **Flush** | `sync` / unmount drain dirty chunks in ascending-LBA order (elevator), then flush the device cache |
| **bdflush** | Background mikuD service writes dirty chunks back every 2 seconds |

> Data safety: `sync` returns only after everything is durable on disk.
> The `dirty=` counter in `blkstat` shows how much data is still RAM-only.

### Drivers

| Driver | Hardware | Notes |
|---|---|---|
| **ATA** | Legacy IDE | PIO + bus-master DMA (64 KiB per command), LBA28/48, IDENTIFY, auto PIO fallback |
| **AHCI** | SATA controllers (PCI 01.06) | Per-spec port bring-up, H2D FIS commands, polled completion |
| **NVMe** | NVMe SSDs (PCI 01.08) | Admin + I/O queue pairs, PRP lists, phase-bit completion |
| **virtio-blk** | QEMU/KVM paravirtual | Legacy virtio-pci, ring sized from the device, FLUSH feature |

---