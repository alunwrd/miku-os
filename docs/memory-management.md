## Memory Management

### Physical Memory Manager (PMM)

MikuOS uses a bitmap-based frame allocator supporting up to 4M frames (16 GB RAM).

| Parameter | Value |
|---|---|
| Frame size | 4096 bytes (4 KB) |
| Max frames | 4,194,304 (16 GB) |
| Bitmap size | 512 KB in BSS |

#### Emergency Frame Pool

| Parameter | Value |
|---|---|
| Pool size | 64 frames (256 KB) |
| Refill source | Normal PMM |
| Refill trigger | Timer ISR at 250 Hz (every 4 ms) |

```
alloc_frame()           - normal alloc from PMM bitmap
alloc_frames(n)         - allocate n contiguous frames
alloc_frame_emergency() - take from emergency pool only (fault context)
alloc_or_evict()        - alloc or evict a page if RAM is full
alloc_for_swapin()      - emergency pool only (page fault handler)
free_frame(phys)        - return one frame to PMM
free_frames(phys, n)    - return n frames to PMM
```

### Virtual Memory Manager (VMM)

| Feature | Description |
|---|---|
| Page table depth | 4 levels: PML4 -> PDP -> PD -> PT |
| HHDM | Higher Half Direct Map (`0xFFFF800000000000`) |
| Swap PTE | PRESENT=0 pages with SWAP_MARKER encode the swap slot |

### mmap Subsystem

| Parameter | Value |
|---|---|
| **Max VMAs** | 256 entries |
| **MAP_FIXED** | Unmaps existing pages before mapping, removes overlapping VMAs |
| **VMA validation** | Rollback on insert failure |

### Swap

#### How Swap Works

```
Page fault (PRESENT=0) -> check PTE for SWAP_MARKER bit

  YES (swapped out):
    1. alloc_for_swapin()  - get frame from emergency pool
    2. ATA read            - load page from swap slot
    3. update PTE          - set PRESENT, clear SWAP_MARKER
    4. swap_map::untrack() - remove from reverse map

  NO:
    normal fault or unmapped address
```

#### Eviction (Clock Sweep)

```
evict_one():
  1. pick_victim()           - two-pass clock sweep (age >= 3 first)
  2. swap_out_internal()     - write page to ATA disk
  3. vmm::mark_swapped()     - encode swap slot in PTE
  4. swap_map::untrack()     - remove from reverse map
  5. pmm::free_frame()       - return frame to PMM
```

#### Swap PTE Encoding

```
bit 0      = 0   (PRESENT=0, page not in RAM)
bit 1      = 1   (SWAP_MARKER)
bits 12..  = swap slot number

is_swap_pte additionally verifies slot number != 0 to prevent false positives
from stale PTE entries with WRITABLE bit set.
```

---