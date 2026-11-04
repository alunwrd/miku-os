# Memory management

MikuOS manages memory in `kernel/mm/`. A bitmap frame allocator hands out 4 KiB physical frames and keeps a copy-on-write count for each one. Four-level page tables give every process its own lower half and share one kernel half. The kernel heap is built like Linux's: size-class slabs, per-CPU magazines, and a mapped area for big objects. Kernel stacks live in that mapped area with guard pages. User memory is lazy: anonymous pages fault in zeroed, and file mappings fault in from the file. `fork` shares pages copy-on-write, swap pushes cold user pages out to a swap partition, and a set of shrinkers gives cached memory back when frames run short. This page describes how each part works, how to watch it, and where its limits are.

## Contents

- [Code map](#code-map)
- [Physical memory: the frame allocator](#physical-memory-the-frame-allocator)
- [Virtual memory and the address-space layout](#virtual-memory-and-the-address-space-layout)
- [The kernel heap](#the-kernel-heap)
- [The mapped area (vmap)](#the-mapped-area-vmap)
- [Kernel stacks](#kernel-stacks)
- [Device memory (DmaBuffer, DmaBox)](#device-memory-dmabuffer-dmabox)
- [User memory](#user-memory)
- [Page-fault handling](#page-fault-handling)
- [TLB shootdown](#tlb-shootdown)
- [Swap](#swap)
- [Reclaim: kswapd and shrinkers](#reclaim-kswapd-and-shrinkers)
- [tmpfs memory](#tmpfs-memory)
- [CPU protections](#cpu-protections)
- [Watching memory: info and /proc](#watching-memory-info-and-proc)
- [Boot flags](#boot-flags)
- [Tests](#tests)
- [Measured numbers](#measured-numbers)
- [Limits and what is not done](#limits-and-what-is-not-done)
- [Troubleshooting](#troubleshooting)

---

## Code map

| File | What it holds |
|:--|:--|
| [`kernel/mm/pmm.rs`](../../kernel/mm/pmm.rs) | Frame allocator: bitmap, copy-on-write counts, per-CPU frame caches, emergency pool |
| [`kernel/mm/vmm.rs`](../../kernel/mm/vmm.rs) | `AddressSpace`, page-table walks, mapping and unmapping, `clone_cow`, `break_cow_page`, PTE software bits, the user/kernel boundary constants |
| [`kernel/mm/mmap.rs`](../../kernel/mm/mmap.rs) | VMAs (`VmaMap`), `mmap`/`munmap`/`mprotect`/`brk`/`msync`, demand faults, fault-around, lent mappings |
| [`kernel/mm/heap/`](../../kernel/mm/heap/) | The kernel's `GlobalAlloc`: `class.rs` (size classes), `slab.rs`, `magazine.rs`, `pages.rs` (where memory comes from), `kcache.rs` (typed caches), `mod.rs` (locks, per-CPU array, shrinker, `slabdebug`) |
| [`kernel/mm/heap/hosttest/`](../../kernel/mm/heap/hosttest/) | Host test crate for the slab and magazine code |
| [`kernel/mm/vmap.rs`](../../kernel/mm/vmap.rs) | The mapped area for objects above 32 KiB and for kernel stacks |
| [`kernel/mm/kstack.rs`](../../kernel/mm/kstack.rs) | Kernel stacks: sizes, painting, depth accounting, the spare list |
| [`kernel/mm/dma.rs`](../../kernel/mm/dma.rs) | `DmaBuffer` and `DmaBox<T>`: contiguous, zeroed memory for devices |
| [`kernel/mm/tlb.rs`](../../kernel/mm/tlb.rs) | Cross-CPU TLB shootdown and `cr3_loaded_elsewhere` |
| [`kernel/mm/swap.rs`](../../kernel/mm/swap.rs) | Swap device: header, slot bitmap and per-slot reference counts, page in/out, `mkswap`/`swapon`/`swapoff` |
| [`kernel/mm/swap_map.rs`](../../kernel/mm/swap_map.rs) | Reverse map for eviction, swap PTE encoding, swap-in, the `kswapd` thread |
| [`kernel/mm/shrink.rs`](../../kernel/mm/shrink.rs) | The shrinker list and the two reclaim entry points |
| [`kernel/mm/selftest.rs`](../../kernel/mm/selftest.rs) | `mm test` / `run=mm_test` |
| [`kernel/arch/x86_64/cpu.rs`](../../kernel/arch/x86_64/cpu.rs) | CR0.WP, SMEP, SMAP, CET detection |
| [`kernel/arch/x86_64/interrupts.rs`](../../kernel/arch/x86_64/interrupts.rs) | Ring-0 `page_fault_handler` and the shared `resolve_page_fault` |
| [`kernel/arch/x86_64/boot_entry.rs`](../../kernel/arch/x86_64/boot_entry.rs) | The boot page tables |
| [`kernel/process/elf_loader.rs`](../../kernel/process/elf_loader.rs) | Where a program's image, stack, TLS and heap go |
| [`kernel/fs/vfs/pages.rs`](../../kernel/fs/vfs/pages.rs) | tmpfs pages (frames) and the tmpfs ceiling |

Design history is in [`dev/kernel-core.md`](../../dev/kernel-core.md), sections "The kernel heap", "Frames", "Kernel stacks" and "Anonymous page faults".

---

## Physical memory: the frame allocator

### How it starts

The boot names memory in two calls, and neither writes a bitmap yet:

- `pmm::add_region(base, size)` records a usable range of the Multiboot2 memory map (up to 128 regions; more are dropped with a warning).
- `pmm::reserve_region(base, size)` records a range that must not be handed out: the kernel image, modules, the framebuffer, the pstore region (up to 64 before layout; one more panics).

`pmm::layout()` then runs once, after the last reservation:

1. It takes the top of usable memory (the highest `base + length`, not the sum of lengths) and sizes two arrays by it: a bitmap of one bit per frame and a count array of one `AtomicU16` per frame.
2. It places both in the first usable run above 1 MiB and below 4 GiB that no reservation touches. No room panics with `pmm: no room for the frame bitmap`.
3. It frees every region, applies every reservation, reserves the arrays themselves, and prints `[pmm] <frames> frames up to <top>: bitmap and counts (<KiB>) at <addr>, <MiB> MiB usable`.

After `layout`, `add_region` and `reserve_region` act on the bitmap directly. Before it, the kernel heap takes its pages from an 8 MiB arena in `.bss` (`heap/pages.rs`).

There is no memory size cap other than the 512 GiB direct map (see below), and there are no Linux-style zones. A caller that needs memory below an address asks for it explicitly with `alloc_frames_below` or `alloc_frames_aligned(.., limit)`.

### The bitmap

`FrameAllocator` (behind one `IrqMutex`, `PMM`) keeps:

| Field | Meaning |
|:--|:--|
| `bitmap` | One bit per frame, set when the frame is not free |
| `frames` | Frames the bitmap covers (every frame below the top of usable memory, rounded up to 64) |
| `total`, `used` | Usable frames and frames in use |
| `free_hint` | Where the next single-frame search starts |
| `contiguous_hint` | Where the next multi-frame search starts |

Searches go word by word: a single frame is the first zero bit of the first word that is not all ones (`trailing_ones`); a run skips all-ones words and takes all-zero words 64 frames at a time. An aligned run starts only on a multiple of its alignment. When a search from the hint fails it wraps around once from frame 0.

The lock is an `IrqMutex` because the frame allocator is reachable from interrupt context: a CPU holding it when its own tick lands must not deadlock.

### Copy-on-write counts

Each frame has one `AtomicU16` in the count array holding **the number of holders minus one**. The array is zeroed at `layout`, so every frame starts with one holder and needs no write to be correct. Two values are special:

| Value | Meaning |
|:--|:--|
| `0` | One holder |
| `n` | `n + 1` holders |
| `MOST` (`u16::MAX - 1`) | Saturation: increments stop here |
| `CACHED` (`u16::MAX`) | The frame is free, sitting in a CPU's frame cache |

Every operation is a single atomic `fetch_update`: no lock and no allocation.

| Function | Effect |
|:--|:--|
| `ref_inc(phys)` | One more holder (saturates at `MOST`) |
| `ref_dec(phys)` | One fewer holder, if there was more than one; returns what was there |
| `try_ref(phys)` | One more holder, or `false` when the frame keeps no count. Used by readers that copy from a frame they do not own after letting go of the lock that found it (`FileCache::pin`) |
| `ref_get(phys)` | Current number of holders |
| `put_cow_ref(phys)` | Drop one reference and say what is left: `Shared`, `Unique` (one holder left, who now owns it), or `Last` (the caller frees it). Used by the copy-on-write break, so a parent and a child that let go at the same moment cannot both skip the free |
| `free_frame_cow(phys)` | Drop a reference; free the frame if it was the last |
| `free_frame_cow_swap(pmm, swap, phys)` | The same inside a batched teardown, keeping the swap reverse map right (untrack on `Last`, unpin on `Unique`) |

A frame just allocated has its count reset to 0 (`fresh`), whatever a careless free left behind.

### Per-CPU frame caches

A page fault and a copy-on-write break each want one frame. Each CPU keeps up to 32 free frames (`FrameCache`, `CACHE = 32`) touched only with interrupts off on that CPU. An empty cache refills 16 frames (`BATCH`) from the bitmap under the shared lock; a full one drains 16 back. A frame in a cache has the count value `CACHED`, so a second free of it is recognised and ignored with `[pmm] frame <addr> freed twice; the second free is ignored`.

### Emergency pool

`EMERGENCY_POOL` holds up to 64 frames for swap-in, which must find a frame even when the allocator is empty. `swapon` fills it, and kswapd refills it by evicting pages when it drops below 32 (see [Swap](#swap)).

### API

| Function | Use |
|:--|:--|
| `alloc_frame()` / `free_frame(phys)` | One frame, through the CPU's cache |
| `alloc_frames(n)` / `free_frames(phys, n)` | `n` contiguous frames |
| `alloc_frames_below(n, limit)` | Contiguous frames below a physical address (32-bit devices) |
| `alloc_frames_aligned(n, align, limit)` | Contiguous, starting on a multiple of `align` frames (slabs aligned to their size) |
| `lock_for_batch()` | A `PmmGuard` that holds the lock across many frees (address-space teardown, `fork`) |
| `stats()` | `(used, total)` frames |
| `running_low()` | Free frames under a sixteenth of the machine: the threshold where kswapd starts giving cached memory back |
| `total_ram_kb()` | RAM the memory map reported |

---

## Virtual memory and the address-space layout

### The boot page tables

`_start32` in [`boot_entry.rs`](../../kernel/arch/x86_64/boot_entry.rs) builds the first tables at physical 0x1000-0x4FFF before long mode:

| PML4 slot | Virtual range | Maps | Page size |
|:--|:--|:--|:--|
| 0 | `0x0` - 4 GiB | Identity, physical 0 - 4 GiB (boot only; user address spaces never get it) | 1 GiB |
| 256 | `0xFFFF_8000_0000_0000` + 512 GiB | The direct map (HHDM) of physical 0 - 512 GiB | 1 GiB |
| 511 (PDPT entry 510) | `0xFFFF_FFFF_8000_0000` | The kernel image (physical 0 - 1 GiB; the kernel is linked at `0xFFFF_FFFF_8020_0000`, loaded at 2 MiB) | 1 GiB |

The direct map is how the kernel reaches any frame: `grub::phys_to_virt(phys)` is `phys + 0xFFFF_8000_0000_0000`. It uses write-back caching; device registers that need uncached access (the LAPIC, the framebuffer) are remapped separately.

### The kernel half

| Range | What |
|:--|:--|
| `0xFFFF_8000_0000_0000` - `+512 GiB` | Direct map (PML4 slot 256) |
| `0xFFFF_C000_0000_0000` - `+512 GiB` | The mapped area, `mm::vmap` (PML4 slot 384): heap objects above 32 KiB and kernel stacks |
| `0xFFFF_FFFF_8000_0000` - | Kernel image (`linker.ld`: `.text`, `.rodata`, `.data`, `.bss`) |

`vmm::KERNEL_HALF` (`0xFFFF_8000_0000_0000`) is the boundary; `vmm::USER_END` (`0x0000_8000_0000_0000`) is the exclusive top of the user half, and `vmm::USER_MIN` (`0x1000`) its bottom: page zero is never mapped for a process, so a NULL dereference always faults. Both `usercopy` and `mmap` re-export these, so the boundary is defined in one place.

### Address spaces

`vmm::AddressSpace` is a CR3 value with methods. `AddressSpace::new_user()` allocates a PML4, zeroes slots 0-255, and **copies slots 256-511 by value** from the kernel's PML4. Every process therefore sees every kernel mapping. A kernel-half PML4 entry made later would be missing from every existing space, so the mapped area's slot is created at boot, before the first process (`vmm::share_kernel_slot`).

Rules the mapping code enforces:

- **A user address space only maps inside `[USER_MIN, USER_END)`** (`may_map`). A mapping at a kernel-half address through a user CR3 would edit the shared kernel tables; it is refused with `[vmm] refusing user mapping at ...`.
- **New intermediate tables are installed by compare-and-swap** (`get_or_create`). Two threads faulting in the same fresh 2 MiB each built a table and the second store used to orphan the first.
- **A demand fill installs its page by compare-and-swap** (`map_page_if_absent`, `map_shared_if_absent`, `map_lent_if_absent`). The loser of the race frees its frame, and the fault is resolved either way.
- **Tables are zeroed where they lie** (`vmm::zero_table`), not built on the stack and copied.
- The kernel's own CR3 is latched once at boot (`latch_kernel_cr3`); `kernel_cr3()` returns it.

### PTE bits

The hardware bits are the usual ones (present, writable, user, PWT, PCD, accessed, dirty, PAT, NX). MikuOS uses the three available software bits:

| Bit | Name | Meaning |
|:--|:--|:--|
| 9 | `PTE_COW` | Copy-on-write: read-only now, copied on the first write |
| 10 | `PTE_SHARED` | A frame the address space maps but does not own: a tmpfs file's page mapped `MAP_SHARED`. `fork` shares it instead of making it copy-on-write, and the swapper never sees it |
| 11 | `PTE_LENT` | A frame lent to this address space alone by its owner (a Mayland shared-memory pool, a GPU memory window). `fork` leaves it out of the child, the swapper never sees it, and only the owner frees it |

A swapped-out page has a non-present entry with bit 1 set as the swap marker, bits 2-11 holding the page's original flags, and the swap slot from bit 12 up (see [Swap](#swap)).

The PAT is reprogrammed so that index 4 is write-combining (`vmm::init_pat`); device apertures mapped into a program use it (`mmap::map_mmio_into_user_wc`).

### A process's layout

From [`elf_loader.rs`](../../kernel/process/elf_loader.rs) and [`mmap.rs`](../../kernel/mm/mmap.rs). Bases are randomized per `exec`.

| Region | Where |
|:--|:--|
| Page zero | Never mapped (`USER_MIN = 0x1000`) |
| `ET_EXEC` image | Wherever it is linked (typically `0x40_0000`) |
| Heap (`brk`) | After the image plus a random gap of up to 32 MiB (13 bits of 4 KiB). Without an image the default base is `0x0000_0060_0000_0000` |
| `mmap` region | `0x0000_0001_0000_0000` - `0x0000_7F00_0000_0000`, first fit from the bottom |
| PIE image | `0x0000_5555_0000_0000` + up to 1 TiB (28 bits) |
| `ld-miku` (the dynamic loader) | `0x0000_7F00_0000_0000` + up to 2 GiB (19 bits) |
| vDSO | Two pages (data, then code) wherever `mmap` finds room |
| TLS block | 16 MiB below the stack's low end |
| Stack | An 8 MiB VMA ending at `0x0000_7FFF_FFFF_0000` minus up to 16 MiB (12 bits). Only the top 64 pages (256 KiB) are mapped at `exec`; the rest faults in |

Images and the stack are registered as VMAs, so `mmap` never hands out a range on top of them. An `ET_EXEC` image may not reach above the space reserved for TLS and the stack (`IMAGE_CEILING`). The vDSO itself is described in [processes.md](processes.md).

---

## The kernel heap

The kernel's `#[global_allocator]` is `KernelHeap` in [`kernel/mm/heap/mod.rs`](../../kernel/mm/heap/mod.rs). Every `Box`, `Vec` and `BTreeMap` in the kernel goes through it. It has three paths, chosen by the `Layout` on both `alloc` and `dealloc` (Rust passes the same layout to both, so a free never has to look its cache up):

| Size | Path |
|:--|:--|
| Up to 6144 bytes, alignment up to 2048 | A size-class cache (slab), behind per-CPU magazines |
| 6145 bytes - 32 KiB (and 4096 or 8192 exactly) | One run of contiguous frames from `pmm::alloc_frames_aligned`, reached through the direct map |
| Above 32 KiB | Single frames mapped side by side in the mapped area (`mm::vmap`) |

32 KiB is order 3, Linux's `PAGE_ALLOC_COSTLY_ORDER`: longer runs are what fragmentation takes first. Nothing that allocates needs physically contiguous memory any more, so the caller does not choose between `kmalloc` and `vmalloc` as in Linux; each allocation takes what fits. `realloc` stays in place within the same class or the same number of pages.

### Size classes

`heap/class.rs` defines 17 classes, named like Linux's `kmalloc-*` caches:

| Classes (bytes) |
|:--|
| 8, 16, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048, 3072, 6144 |

4096 and 8192 are not classes; they are one and two frames on the page path with nothing wasted. An object sits at a multiple of its size from a slab aligned to its own size, so it is aligned to the lowest set bit of its size. A layout that wants more alignment takes the next class that gives it.

### Slabs

A slab (`heap/slab.rs`) is 1 to 8 pages, aligned to its own size, with a 64-byte header in its **last** bytes. The slab of any object is its address with the low bits cleared. Each class's slab order is the smallest that leaves the header room for at most one object in sixteen.

- **Free list.** Threaded through the free objects. Each link is stored XORed with the cache's secret and the link's own address, as Linux's `SLAB_FREELIST_HARDENED`: a stray write into a free object makes an address that faults rather than one that hands out someone else's memory.
- **Lists.** A cache keeps partial, full and empty slabs. Empty slabs beyond `MIN_EMPTY` (2) go back to the frame allocator at once. A slab that receives a free goes to the head of the partial list, so the most recently freed object is handed out next, while it is still in cache.
- **Wrong-cache frees.** The slab header carries its cache's object size, so a free with a layout that does not match the allocation is caught.

### Per-CPU magazines

Each CPU has, per class, a `loaded` and a `previous` magazine: plain arrays of 32 object pointers (`magazine.rs`, `MAG = 32`).

- `alloc` pops from `loaded`; `free` pushes to it.
- When `loaded` is empty (or full), it swaps with `previous`.
- When both are, a whole magazine is exchanged with the class's **depot** (up to 16 full magazines, `DEPOT = 16`) under the class lock. That lock is the only shared one, taken once per 32 objects.
- The depot remembers the fewest full magazines it held during the last interval, and kswapd returns that many to the slabs every five seconds (`heap::reap`, `REAP_TICKS = 5 * 250`): Bonwick and Adams' working-set rule.

Magazines are filled from fixed arrays, so the heap never allocates in order to free.

Magazines are turned on in `heap::init` once every CPU has its per-CPU base, and stay off under `slabdebug`.

### Typed caches

`KCache<T>` (`heap/kcache.rs`) is a named cache for one type, like Linux's `kmem_cache_create`: slabs sized for `T`, a lock of its own, no magazines. The VFS's vnode table uses it. Each appears in `/proc/slabinfo` by name, and the heap's shrinker returns their empty slabs too.

### Locks and rules

- The per-CPU magazine arrays and every class lock are `IrqMutex`, so **allocating in an interrupt handler or with interrupts off is safe**.
- **Lock order:** a CPU's magazines, then a class's cache, then frames. In the mapped area: vmap, then a cache, then frames. Nothing under the frame allocator's lock allocates.
- **An allocation that finds no frames** runs every shrinker in direct mode (`shrink::direct`) and tries once more. See [Reclaim](#reclaim-kswapd-and-shrinkers).
- **Memory a device reads is never the heap.** Use `DmaBox`/`DmaBuffer`. `grub::any_virt_to_phys` panics on an address in the mapped area, so a driver that still hands a heap pointer to a device fails at the line that does it.
- **Tests that allocate and free must `black_box` their pointers.** LLVM may remove an unused allocation together with its free, and may assume a new allocation differs from one just freed.

### slabdebug

`slabdebug` on the boot line turns on:

- magazines off (every object goes straight to its slab);
- a freed object filled with poison (`0x6b`) past its free-list link, checked when it is handed out again (write after free);
- the bytes between the requested size and the class size filled as a red zone (`0xbb`) and checked at free (write past the end). An object that fills its class exactly has none;
- a double free found on the slab's free list, reported and ignored.

Each finding is a `kerror!` naming the cache, printed after the cache lock is released (a report allocates). The `heap/slabdebug` selftest makes three such reports on purpose.

The old ring-3-derived binned heap (`lib/libmiku/heap_core.rs`) is no longer part of the kernel; it remains ring 3's allocator (see [sdk.md](sdk.md)).

---

## The mapped area (vmap)

[`kernel/mm/vmap.rs`](../../kernel/mm/vmap.rs) is Linux's `vmalloc`: 512 GiB at PML4 slot 384 (`vmap::BASE = 0xFFFF_C000_0000_0000`, `SIZE = 1 << 39`).

- **Objects** are single frames mapped side by side at 4 KiB, so a 28 MiB firmware image needs 7168 free frames, not 28 MiB in one run.
- **Guard pages.** The page after every object stays unmapped, so running off the end faults at the faulting instruction. The page below every object is never mapped either (it is the guard of the span below, or free space); that is what a kernel stack, which overflows downwards, needs.
- **Free ranges** are extents in two ordered maps: by address (to merge with neighbours) and by length (best fit). Ranges waiting for a shootdown are kept the same way. Every node comes from the small-object caches, which never come from vmap, so the area never allocates from itself.
- **Freeing** unmaps at once and flushes only the local CPU; the frames go back immediately. The virtual range is reused only after one shootdown of every CPU covers many frees (Linux's lazy `vunmap`). kswapd does that shootdown (`vmap::purge_if_wanted`) with no lock held once 8192 freed pages (`LAZY_PAGES`) are waiting; an allocation that finds the area full purges at once. A shootdown under a lock another CPU spins on with interrupts off could never be answered.
- `vmap::contains(addr)` and `vmap::mapped_pages()` are the queries used by `any_virt_to_phys`, `/proc/meminfo` (`VmallocUsed`) and `info`.

---

## Kernel stacks

[`kernel/mm/kstack.rs`](../../kernel/mm/kstack.rs). Every task has its own kernel stack, allocated from the mapped area like Linux's `CONFIG_VMAP_STACK`.

| Task kind | Stack | Constant |
|:--|:--|:--|
| A program's task (`new_elf`, `new_thread`, `new_fork`) | 128 KiB (512 KiB with `bigstacks`) | `program_stack_size()` |
| A kernel thread (`alloc_raw`, the idle tasks) | 512 KiB | `KERNEL_THREAD_STACK` |

- **Guard.** The unmapped page below every vmap object is the guard. An overflow faults at the instruction that did it; the page-fault handler finds the owner (`kstack::guard_fault_owner`), prints `[stack] kernel stack overflow: pid=<n> '<name>' hit its guard page at <addr>`, and panics naming the task. Nothing is unmapped to make a guard, so making a stack needs no TLB shootdown.
- **Painting.** A new stack is filled with a pattern (`PAINT`), not zeroed. When a task is freed (`kstack::account`, from `free_process_resources`) the stack is read back:
  - a task that reached its last 4 KiB (`RED_ZONE`) is always logged with `kwarn!`;
  - with `stackdepth` on the boot line every stack is read whole, each new deepest is logged with its task, and `/proc/meminfo` shows `KernelStackMax` (live tasks included) next to `KernelStack` (all stacks held).
- **Boot stacks.** Stacks made before the mapped area exists are contiguous direct-map frames with a direct-map guard frame. These keep a spare list (`SPARE`, at most 32) so a returned stack can be reused without re-unmapping its guard; the spare list is a shrinker ("spare kernel stacks").

Why 128 KiB: 64 KiB was tried, and every suite, the fork storm and the desktop fit in 34 KiB, but an ext4 `fsync` of 64 MiB overflowed in the block layer, which then kept a 64 KiB buffer on the stack. The large frames were found by reading function prologues off the binary (`llvm-objdump -d`, `sub $n,%rsp`) rather than by running tests. A new local array of 4 KiB or more on a syscall-reachable path belongs on the heap. See [`dev/kernel-core.md`](../../dev/kernel-core.md), "Kernel stacks".

---

## Device memory (DmaBuffer, DmaBox)

[`kernel/mm/dma.rs`](../../kernel/mm/dma.rs).

- `DmaBuffer::alloc(pages)`, `alloc_below(pages, limit)`, `alloc_aligned(pages, align, limit)`: zeroed, page-aligned, physically contiguous frames, freed on drop. `phys()`, `virt_base()`, `as_slice()`, `zero()`, `write_barrier()`, `flush_to_memory()`; `leak()` keeps the memory for ever.
- `DmaBox<T>` is a typed view on a `DmaBuffer`. `DmaBox::new_below` keeps it below 4 GiB. `phys_of(ptr)` answers only for a pointer inside the box.
- Below 4 GiB: AHCI, ATA bus mastering, rtl8139, rtl8168. Anywhere: NVMe, virtio-blk, the console's copy-engine shadow.
- Mayland's shared-memory pools are `DmaBuffer`s, lent into a client's address space with `PTE_LENT` (below).

---

## User memory

### VMAs

Each address space has a `VmaMap` in the global `VMA_MAP` (a `spin::Mutex<BTreeMap<cr3, VmaMap>>`):

- `vmas`: `BTreeMap<start, Vma>`, address order. A `Vma` is `{ start, end, prot, backing }`; `backing` is `None` for anonymous memory or a `FileBacking { vnode_id, file_off, file_size, shared }`.
- `brk` and `brk_floor`: the current program break and the lowest it may go (pinned by the ELF loader to the image's end).

Anonymous VMAs with the same protection merge with their neighbours (`insert_merged`); file VMAs never merge, since each carries its own file offset. `munmap` and `mprotect` split partly covered VMAs, and a right-hand piece of a file VMA has its offset advanced by the bytes before it. `overlapping(s, e)` touches only the VMA starting below `s` that reaches into the range plus those starting inside it; it used to read every VMA below the range.

A second table, `MAPPINGS` (`vnode -> count`), counts every address space's file VMAs per vnode. The VFS asks it (`mmap::is_mapped`) so that a mapped vnode is never evicted and an unlinked file is not freed while still mapped. The last mapping going tells the VFS, which frees the unlinked file then.

### System calls

All addresses must be page-aligned and the whole range must lie in the user half; lengths are rounded up to pages. Errors are negative errnos.

| No. | Call | Arguments | Notes |
|:--|:--|:--|:--|
| 3 | `mmap` | `addr, len, prot, flags` | Anonymous only. `MAP_FIXED` (0x10) replaces whatever is in `[addr, addr+len)`; otherwise first fit in the mmap region. `MAP_SHARED` is accepted but anonymous memory is always private (copy-on-write across `fork`) |
| 60 | `mmap_file` | pointer to `{ addr, length, prot, flags, fd, offset }` (six `u64`) | File mapping. `offset` must be page-aligned. `flags & 1` = `MAP_SHARED`, otherwise private. The mapping holds the vnode, not the descriptor, so it survives `close` |
| 61 | `msync` | `addr, len` | Write back dirty pages of `MAP_SHARED` file mappings in the range |
| 4 | `munmap` | `addr, len` | Writes back dirty shared file pages first, then unmaps |
| 5 | `mprotect` | `addr, len, prot` | Changes present pages in place and the VMA table; holes stay holes without an error |
| 6 | `brk` | `addr` | `0` returns the current break. Below the floor or at/above `USER_END` returns the old break unchanged |

Common errors: `EINVAL` (-22) for a zero length, an unaligned address, a range outside the user half, an unknown `prot` bit, or **`PROT_WRITE | PROT_EXEC` together**; `ENOMEM` (-12) when no free range fits. `mmap_file` also returns `EINVAL` for a descriptor that has no backing vnode, and `EFAULT` when the argument block is not readable. The full ABI is in [MikuOS_ABI.md](../MikuOS_ABI.md).

**W^X.** The kernel never creates a page that is both writable and executable. `mmap`, `mmap_file` and `mprotect` refuse the combination outright; `prot_to_flags` would drop `EXEC` anyway. `PROT_NONE` on a present page keeps the data as a supervisor-only page ring 3 cannot reach.

### Anonymous memory

Nothing is allocated at `mmap` time. A page is zeroed and mapped on its first touch (`handle_anon_fault`), from ring 3 or from a syscall that validates a pointer into it (`fault_in_user_range`). That makes a thread's 2 MiB stack cost only the pages it touches, and `mmap(PROT_NONE)` reservations (how wasmtime reserves gigabytes for guards) cost nothing.

- **`PROT_NONE` is a guard.** A fault in a `PROT_NONE` VMA is not filled; it is the fault the guard was placed to catch.
- **Fault-around.** When the page before the faulting one is already mapped, the fault also fills up to 15 pages after it (`FAULT_AROUND = 15`) inside the same VMA, zeroed: 64 KiB per fault for memory written front to back (a heap slab, a `Vec`, a read buffer). It stops at the first page already present or swapped out, and does nothing when `pmm::running_low()`. Those pages go from not-present to present, which no TLB caches, so they need no flush. Random access never has its previous page mapped and pays one fault per page.
- **`brk` grows eagerly**: `sys_brk` allocates, zeroes and maps the new pages at once, then records an anonymous VMA. Shrinking unmaps.

### File mappings

A file VMA is filled lazily by `handle_file_fault`, and only for faults from ring 3: the fill takes the VFS lock to read, and kernel code that may already hold it reaches file pages through `fault_in_user_range` before taking any VFS lock.

| Mapping | Of a tmpfs file | Of a disk file (ext, btrfs) |
|:--|:--|:--|
| `MAP_SHARED` | Maps **the file's own frame** (`vfs::shared_frame`, `PTE_SHARED`, one reference taken). Every mapping, `read` and `write` see the same page. `fork` shares it | A private frame filled from the file. Dirty pages (hardware dirty bit) are written back with `write_at_vnode` on `munmap` and `msync`, never past the file size at map time. Two mappings of the same file are **not coherent** with each other |
| `MAP_PRIVATE` | A private frame filled from the file; writes are never written back | The same |

The part of the last page past the end of file stays zero. A tmpfs page truncated under a mapping survives until the mapping lets go of its reference. How the VFS page cache relates to mappings is described in [vfs.md](vfs.md).

### fork and copy-on-write

`AddressSpace::clone_cow` builds the child's tables under `COW_LOCK` (a machine-wide spin with interrupts off that answers shootdowns while it waits):

- Kernel slots 256-511 are copied by value.
- Each user page-table level is allocated fresh and zeroed in place.
- For each present leaf entry:
  - `PTE_LENT` entries are left out of the child (and `vma_clone` removes those ranges from the child's VMA table, so the child's first touch faults instead of filling zeroes);
  - `PTE_SHARED` entries are copied as they are, with one more reference;
  - everything else becomes read-only with `PTE_COW` in **both** parent and child, the frame gets one more reference, and its swap reverse-map entry is pinned (the reverse map has one entry per frame and cannot record two owners).
- If another CPU has the parent's CR3 loaded (`tlb::cr3_loaded_elsewhere`, asked after the change), every CPU is shot down **before** `COW_LOCK` is released, so no sibling thread keeps writing through a stale writable translation.

The SWAP_MAP and PMM locks are held once for the whole walk, not per page.

Non-present entries are not copied, with one exception: a **swapped-out page**. Its entry (present bit clear, swap marker set) is copied into the child as it is, and the swap slot's reference count goes up (`swap::dup_swap_slot`), so both processes name the same slot. Whichever touches the page first reads it back into a frame of its own and drops its reference; the other reads the same slot later, and the slot is freed when the last entry naming it is read back or unmapped. A slot whose count cannot grow fails the `fork` with `ENOMEM`. (Before 2026-11-03 such entries were skipped and the child found a zero page there.)

**The copy-on-write break** (`break_cow_page`, from the fault handler and from syscalls writing out-parameters) runs under `COW_LOCK` in this order:

1. If the frame has more than one holder, copy it into a new frame.
2. Point the entry at the copy, **still read-only and still `PTE_COW`**, and flush locally.
3. Shoot down every CPU that has this CR3 loaded.
4. Make the entry writable and clear `PTE_COW`.
5. Drop the old frame's reference with `put_cow_ref`; free it if that was the last.

If the frame had only one holder, the entry is simply made writable. The order matters: a copy installed writable before the shootdown let one CPU write the new frame while another still read the old one, which lost heap free-list updates under a fork storm (about once in 24 runs of `stdproc --churn 3000 fork` before 2026-09-26).

Kernel writes to user memory obey the same rules because CR0.WP is on: a syscall writing into a copy-on-write page breaks it first.

### Lent pages

`mmap::map_phys_into_user(cr3, phys, size, prot)` maps physically contiguous memory the kernel owns into a process: a Mayland shared-memory pool, which the compositor and the copy engine read at its physical address while the client draws through this mapping. `map_mmio_into_user_wc` does the same for a device aperture (a program's video memory through BAR1), write-combining.

- The entries carry `PTE_LENT`: never copy-on-write, never left in a `fork` child, never tracked by the swapper.
- Each page gets one reference, so the teardown of the address space, which frees every present page it walks, only drops the count. The owner frees the frames when the pool goes.
- `unmap_phys_from_user` drops the window and the references and leaves the frames to their owner.

`fork` used to make pool windows copy-on-write, and a client that spawned anything drew into private copies the compositor never read (fixed 2026-09-26). The pool protocol is in [mayland.md](mayland.md).

### Unmapping

`munmap` and teardown take pages out with `AddressSpace::take_range`, which takes `COW_LOCK` once per page table, skips empty entries and missing tables, and collects the frames. If another CPU has the CR3 loaded, it shoots down **before** the frames are freed. Freed frames go through `free_frame_cow`, so a shared or copy-on-write frame survives while another holder remains. Swap slots named by swapped-out entries are freed too.

---

## Page-fault handling

A fault from ring 3 goes through `trap::user_trap`; a fault in ring 0 goes to `page_fault_handler` on its own IST stack. Both call `resolve_page_fault(cr3, addr, error_code, from_user)`:

1. Only canonical user addresses above page zero are considered.
2. **Swap PTE:** swap the page in (`swap_map::try_swapin`) and retry.
3. **Write to a present `PTE_COW` page:** break copy-on-write.
4. **Stale translation (ring 3 only):** if the entry already allows the access (present, user, writable for a write, executable for a fetch, no reserved-bit error), return and let the instruction retry. This happens when a sibling thread broke copy-on-write or filled the page and the faulting CPU still had the old translation cached.
5. **Not present inside a VMA:** `handle_vma_fault`. Anonymous VMAs fill from any CPL (the kernel writing a signal frame onto an untouched stack page lands here); file VMAs fill only for ring 3.

Anything not resolved:

- **From ring 3:** the task gets `SIGSEGV` with a Linux `siginfo_t`/`ucontext_t` (see [processes.md](processes.md)).
- **From ring 0:** the handler prints `[page fault] addr=... code=... user=false`, the PTE (`pte=... P= W= U= NX= COW=`), and a crash report, then halts.

Extra diagnostics in the ring-0 handler:

- A fault in the kernel half that lands on a stack guard page names the task (see [Kernel stacks](#kernel-stacks)).
- A ring-0 protection fault on a present user page with SMAP on and RFLAGS.AC clear prints `[smap] ring-0 access to user address ... - a usercopy::UserAccess is missing there`.
- A page fault inside the page-fault handler is detected with a per-CPU depth counter and panics with `page fault at ... while handling a page fault` instead of looping silently on the IST stack.

---

## TLB shootdown

[`kernel/mm/tlb.rs`](../../kernel/mm/tlb.rs). MikuOS does not use PCID, so a CR3 switch drops all user translations, and only CPUs that currently have an address space loaded can hold its translations.

- **`cr3_loaded_elsewhere(cr3)`** fences, then reads every other CPU's `Cpu::loaded_cr3`, which each CPU stores (SeqCst) just before every `mov cr3`. It is asked **after** the page-table change: a CPU it misses stores its `loaded_cr3` later, then loads CR3, and walks the new tables.
- **`shootdown()`** flushes every other CPU and waits:
  - one requester at a time (`REQUESTER`); a CPU waiting to become requester answers other requests from its spin loop, because it may have interrupts masked;
  - a generation number (`GENERATION`) and each CPU's last flushed generation (`SEEN`); the requester sends `VEC_IPI_TLB` to all but itself and waits for every online CPU's `SEEN` to reach its number;
  - interrupts are off from taking the requester flag to releasing it;
  - a CPU that does not answer within 200,000,000 spins is skipped with `[tlb] cpu <n> did not answer shootdown <gen>`.
- **Callers:** `munmap`/teardown (before freeing frames), `mprotect` (when it changed a present page), `fork` (before releasing `COW_LOCK`), the copy-on-write break (before making the copy writable), and vmap's lazy purge.
- **Rule:** never call `shootdown` while holding a lock another CPU may spin on with interrupts off.
- **Not-present to present** changes need no flush (Intel SDM vol. 3, 4.10.4.3); fault-around and putting a guard frame back rely on this.
- **`tlbalways`** on the boot line makes `cr3_loaded_elsewhere` answer yes every time: a shootdown for every change, the safe superset.

History: until 2026-09-24 unmaps flushed only the local CPU, and Zed's threads corrupted each other's heap through stale translations; since 2026-10-23 only CPUs with the CR3 loaded are asked.

---

## Swap

Swap moves cold user pages to a swap partition and brings them back on fault. Setting up a swap device (`mkswap`, `swapon`, `swapoff`) is described in [storage.md](storage.md).

### The device

[`kernel/mm/swap.rs`](../../kernel/mm/swap.rs).

- **Format.** Linux's swap header, version 1: page 0 holds the version, the last page number, a pseudo-UUID derived from the partition's LBA, a label of up to 15 bytes, and the magic `SWAPSPACE2` at offset 4086. `mkswap` refuses a partition smaller than 10 pages.
- **One device at a time.** `swapon` reads and checks the header, records the device and LBA, clears the slot bitmap, and fills the emergency frame pool. `swapoff` refuses while any slot is in use (`SwapInUse`).
- **Slots.** A bitmap of at most 65,536 pages (`SWAP_MAX_PAGES`), so at most 256 MiB of swap is used whatever the partition size. Slot numbers start at 1; slot `n` is at `partition_lba + n * 8` sectors.
- **I/O** goes through the block layer (`io::block::read`/`write`), 8 sectors per page.

### Choosing what goes out

[`kernel/mm/swap_map.rs`](../../kernel/mm/swap_map.rs) keeps a reverse map indexed by frame number: for each tracked frame, the owning CR3, the virtual address, the PTE flags, an age, and a pinned flag.

- Every user page mapped with `map_page`, `map_range` or `map_page_if_absent` is tracked. Kernel-half mappings and frames below 4 MiB are tracked pinned. `PTE_SHARED` and `PTE_LENT` pages are never tracked.
- Copy-on-write shared frames are pinned while shared, and unpinned when one holder is left.
- kswapd ages every unpinned entry about once a second (`age_all`).
- `pick_victim` is a single clock sweep: the first unpinned entry with age 3 or more, or else the first unpinned entry seen.
- **The map has 65,536 entries indexed by frame number** (`MAX_TRACKED`), so only frames in the first 256 MiB of physical memory are ever tracked and can be swapped out.

### Going out and coming back

- **Out** (`evict_one`): pin the victim, write it to a fresh slot, replace its PTE with a swap PTE that keeps the original flags (`make_swap_pte_with_flags`), untrack it, free the frame.
- **Swap PTE format:** bit 0 clear (not present), bit 1 set (swap marker), bits 2-11 the saved flags, bits 12-31 the slot (20 bits).
- **In** (`try_swapin`, from the fault handler or `fault_in_user_range`): take a frame from the emergency pool, or evict another page and allocate; read the slot; free the slot; map the frame with the saved flags; track it again.
- `handle_vma_fault` refuses to demand-fill a swap PTE: filling it with zeroes would lose data and leak the slot.

### When it runs

Swapping is driven by the emergency pool, not by general memory pressure:

- The timer interrupt only raises a flag (`request_reclaim_from_isr`) when the emergency pool is below 32 frames. It does no I/O and takes no lock.
- kswapd sees the flag and evicts pages until the pool holds 64 frames (`refill_emergency_pool`).
- `swap_map::alloc_or_evict` (an allocation that evicts one page when the allocator is empty, then falls back to the emergency pool) exists as `swap::try_reclaim_page`.

Every swap-out and swap-in prints a `[swap]` and a `[swap_map]` line to serial.

---

## Reclaim: kswapd and shrinkers

### kswapd

`swap_map::kswapd_thread` is a kernel thread started at boot. Each pass it:

1. ages the swap reverse map and runs `sched::stats::report_overdue_sleepers` about once a second (see [scheduler.md](scheduler.md));
2. refills the emergency pool if the timer asked for it;
3. runs the shrinkers if `pmm::running_low()` (`shrink::run_if_low`);
4. purges vmap's lazily freed ranges if enough are waiting;
5. every five seconds, has the heap's depots give back unused magazines (`heap::reap`);
6. sleeps 25 ticks (100 ms).

### Shrinkers

[`kernel/mm/shrink.rs`](../../kernel/mm/shrink.rs) is Linux's `register_shrinker` as one static list, asked in this order (cheapest to rebuild first):

| Name | What it gives back | Where |
|:--|:--|:--|
| `file pages` | Clean cached pages of disk files | `fs::vfs::core::FILE_CACHE_SHRINKER` |
| `vnode cache` | Unreferenced vnodes (their freed slabs then go back through the heap) | `fs::vfs::core::VNODE_SHRINKER`, see [vfs.md](vfs.md) |
| `kernel heap` | Every magazine back to the slabs, every empty slab back to the frame allocator | `mm::heap::SHRINKER` |
| `spare kernel stacks` | Boot-time stacks waiting for reuse | `mm::kstack::SHRINKER` |

Each shrinker has `count()` (frames it could give now) and `scan(frames, direct)`.

- **From kswapd** (`run_if_low`): when free frames are under a sixteenth of the machine, each shrinker is asked for a sixteenth of what it counts (plus one), every pass.
- **Direct** (`shrink::direct`): the kernel heap (and `KCache`) call it when an allocation finds no frames, then retry once. The caller may hold any lock, so each shrinker takes only what it can `try_lock`; the spare stacks give nothing in direct mode. Only one direct reclaim runs at a time on the machine.

A user page fault that finds no frame does not run direct reclaim; it fails (and the program gets `SIGSEGV`). There is no OOM killer.

---

## tmpfs memory

tmpfs file data are physical frames from `pmm`, one 4 KiB frame per page, taken when a file first writes a page and given back when the file drops it ([`kernel/fs/vfs/pages.rs`](../../kernel/fs/vfs/pages.rs)).

- **Ceiling:** `tmpfs_limit()` is a quarter of usable frames (at least 256 pages), computed when a page is allocated. Past it, writes fail with `NoSpace`.
- **File size:** up to 4 GiB (a two-level index of 1024 x 1024 pages, `address_space.rs`).
- **No eviction:** a tmpfs page has nowhere to be written back to, so it is never evicted and never swapped.
- **Shared mappings** map the file's own frame (above). Frames are reference-counted through the copy-on-write counts: the cache holds one reference and each mapping another.

The tmpfs filesystem itself is described in [filesystems.md](filesystems.md); the disk page cache, which can evict, in [vfs.md](vfs.md).

---

## CPU protections

`cpu::init_protections()` runs on the boot CPU and on every AP after the GDT and IDT are loaded:

| Feature | Effect | Turned off by |
|:--|:--|:--|
| CR0.WP | Ring 0 obeys read-only page bits, so kernel writes through a validated user pointer break copy-on-write instead of writing through it | always on |
| CR4.SMEP | Ring 0 cannot execute user pages | `nosmep` |
| CR4.SMAP | Ring 0 cannot read or write user pages unless RFLAGS.AC is set; only `usercopy::UserAccess` (`stac`/`clac`) sets it | `nosmap` |
| EFER.NXE / NX | No-execute pages: user data, stacks, heap, vmap and kernel stacks | always on (the AP trampoline sets NXE) |
| CET (IBT, shadow stacks) | Detected and reported only; never enabled (programs are not built with `endbr64`, and the VMM cannot express shadow-stack pages) | `nocet` forces the report to "off" |

SMEP and SMAP are enabled only when the CPU reports them. The boot CPU prints:

```
[cpu] CR0.WP=on SMEP=on SMAP=on
[cpu] CET: IBT=no SHSTK=no - unsupported
```

`off (cmdline)` and `unsupported` distinguish a flag from a missing feature. Kernel code that touches user memory must hold a `UserAccess` from `user_ptr_mapped`/`user_ptr_writable` (see [processes.md](processes.md) and [MikuOS_Security.md](../MikuOS_Security.md)). Ring 3 can leave RFLAGS.AC set; every fault handler clears it first (`clac`).

If a boot dies after a change in this area, `nosmap` is the first flag to try.

---

## Watching memory: info and /proc

### Kernel shell commands

| Command | Shows |
|:--|:--|
| `info` | Version, vnode and mount counts, heap used/total, the three largest slab caches and the mapped area, RAM used/usable, free, swap used/total/free or `inactive`, uptime |
| `heap` | Heap total, used, free and usage; warns when usage is high |
| `memmap` | The Multiboot2 memory map: base, length, type |
| `mm test` | The memory selftest (below) |

These are kernel-shell commands; the full command reference is [commands.md](commands.md).

### /proc

| File | Memory content |
|:--|:--|
| `/proc/meminfo` | `MemTotal`, `MemFree`, `MemAvailable` (equal to `MemFree`), `SwapTotal`/`SwapFree` (always 0, even with swap active), `Slab`, `SReclaimable`, `SUnreclaim`, `VmallocUsed`, `KernelStack`, `KernelStackMax`, then `Vnodes`, `TmpfsPages`, `KernelHeap: used/total (free)` |
| `/proc/slabinfo` | Every slab cache and `KCache` in Linux's version 2.1 columns |
| `/proc/heap` | Heap `total`, `used`, `free` |
| `/proc/stat` | Includes the file cache line (pages cached, dirty, dirtied, cleaned) |

The file-by-file reference is in [diagnostics.md](diagnostics.md).

---

## Boot flags

The full table of kernel command-line flags is in [boot.md](boot.md). Those that change memory behaviour:

| Flag | Effect |
|:--|:--|
| `slabdebug` | Heap poisoning, red zones, double-free detection; magazines off |
| `bigstacks` | 512 KiB kernel stacks for program tasks instead of 128 KiB |
| `stackdepth` | Read every kernel stack's depth when its task ends; log each new deepest; `KernelStackMax` in `/proc/meminfo` |
| `tlbalways` | A TLB shootdown for every page-table change, regardless of which CPUs have the CR3 loaded |
| `nosmap` / `nosmep` | Leave SMAP / SMEP off |
| `nocet` | Report CET as off even where the CPU has it |
| `nopagecache`, `writethrough` | Page-cache behaviour for disk files; see [vfs.md](vfs.md) |

---

## Tests

### On the machine: `mm test` / `run=mm_test`

Run `mm test` in the kernel shell, or put `run=mm_test` on the GRUB line (see [testing.md](testing.md)). Case groups in [`kernel/mm/selftest.rs`](../../kernel/mm/selftest.rs):

| Case | Checks |
|:--|:--|
| `dma/device-memory` | Every page the CPU sees in ATA's and NVMe's `DmaBox` memory is the frame the device is given (one physical run) |
| `heap/aligned-blocks` | Allocations of every class honour their alignment |
| `heap/churn` | 20,000 mixed allocations and frees; memory held against memory used |
| `heap/cost` | Cost of an allocation and a free, on one CPU and on all at once |
| `heap/gives-back` | A large burst of small objects goes back to the frame allocator when freed |
| `heap/shrinker` | The heap's shrinker returns magazines and empty slabs |
| `heap/slabdebug` | Write-after-free, write past the end, double free (three reports on purpose; only meaningful under `slabdebug`) |
| `vmap/big-object` | Objects above 32 KiB come from vmap and are readable/writable end to end |
| `vmap/purge` | Lazily freed ranges are reused after a purge |
| `fault/parts` | Measures a page fault's parts: frame, zeroing, mapping |
| `frames/cow-counts` | Copy-on-write count arithmetic |
| `frames/double-free` | A frame freed twice is caught and not handed out twice |

The baseline on 2026-10-27 was "mm 10+2" (ten cases and two measurements) on both CPU models.

### On the build host

```
cd kernel/mm/heap/hosttest && cargo test
```

Eleven cases run `class.rs`, `slab.rs` and `magazine.rs` against a model; those files know no lock and no kernel for exactly this reason.

---

## Measured numbers

All on QEMU/KVM with 4 vCPUs unless noted; Linux numbers are the dev box itself on bare metal, so not like for like. Source: [`dev/kernel-core.md`](../../dev/kernel-core.md).

**Kernel heap (2026-10-27, `mm_test`):**

| | before (binned heap) | slab heap |
|:--|:--|:--|
| One allocation and free, one CPU | 16-22 ns | 15-17 ns |
| The same, 4 CPUs at once | 647-687 ns | 18-24 ns |
| After 20,000 mixed (churn) | 68 MiB held for 29 MiB used | 34 MiB for 29-32 MiB |
| 96 MiB of 256-byte objects, all freed | nothing returned | all returned |
| A 64 MiB object | one run of 16,384 frames | 16,384 single frames, mapped |

**Page faults (stdbench, 64 MiB touched once, 2026-10-27):** 717 ns a page before, 532-539 ns after fault-around; Linux 258 ns (with THP). Of the remaining cost, zeroing a page is about 300-330 ns in QEMU, the same with `write_bytes`, `rep stosq` or `rep stosb`: it is guest memory bandwidth.

**Kernel stacks (2026-10-27):** 2000 threads held 1034 MB of kernel stack before vmap stacks, 263-265 MB at 128 KiB; the deepest program stack seen in every suite, the fork storm and the desktop was 34 KiB (`sys_fork`), the deepest kernel thread 82 KiB.

**munmap of a 2 MiB thread stack:** 37-45 us before `take_range`, 2-3 us after (2026-10-23).

**TLB (2026-10-27):** a random line per page costs 2 ns at 1 MiB, 2-3 ns at 16 MiB, 4-8 ns at 256 MiB; Linux on the host 2, 2 and 5. This is why THP stays deferred.

---

## Limits and what is not done

- **Transparent huge pages: deferred.** User memory is 4 KiB pages only. Measured page-walk costs are a few nanoseconds (above), so THP would gain little on random access and nothing that was measured waits on it. Fault-around (64 KiB per fault) is the small-scale substitute for sequentially written anonymous memory.
- **No pre-zeroed frames.** Zeroing in the background would take the cost out of the fault but not out of the machine; Linux had it and removed it.
- **No MWAIT idle, no PCID.**
- **Swap:**
  - one device, at most 65,536 pages (256 MiB) used;
  - only frames in the first 256 MiB of physical memory can be evicted (the reverse map is indexed by frame number up to 65,536);
  - eviction is driven only by the emergency pool running low, not by general memory pressure;
  - `/proc/meminfo` reports `SwapTotal` and `SwapFree` as 0 even with swap on (`info` shows the real numbers);
  - no swap files.
- **Anonymous `MAP_SHARED`** is not implemented: anonymous memory is always private across `fork`. Shared memory between processes is a tmpfs file mapped `MAP_SHARED`, or a Mayland pool.
- **Disk-file `MAP_SHARED` mappings are not coherent** with each other or with `read`/`write` until written back (`munmap`/`msync`).
- **No OOM killer.** A user page fault that finds no frame is a `SIGSEGV`; a kernel allocation that finds none after direct reclaim fails or panics at its caller.
- **512 GiB of physical memory** is the most the direct map reaches.
- **No zones.** 32-bit DMA is handled by explicit `below` allocations.

---

## Troubleshooting

| Symptom | Where to look |
|:--|:--|
| A boot dies or a syscall faults after a change touching user memory | Boot with `nosmap`; the `[smap] ring-0 access to user address ... rip=...` line names the site missing a `UserAccess` |
| `kernel stack overflow in pid N` panic | Boot with `bigstacks` to confirm; then find the large frame (`llvm-objdump -d`, function prologues) and move the buffer to the heap. `stackdepth` logs how deep each stack went |
| `[stack] ... hit its guard page` | Same as above |
| Heap corruption suspected | Boot with `slabdebug` and run the suites; reports name the cache |
| Memory not coming back | `/proc/slabinfo` and `info` show which caches hold it; kswapd returns depot magazines every five seconds and shrinks only when free frames are under 1/16 |
| `[pmm] frame ... freed twice` | A double free of a frame; the second free was ignored |
| `[tlb] cpu N did not answer shootdown` | A CPU with interrupts off for far too long, or halted by a panic |
| Data corruption in threaded programs after `munmap`/`fork` | Try `tlbalways` to rule out a missed shootdown |
| `page fault at ... while handling a page fault` | A bug in the fault path itself |
| `[swap_map] OOM: no frame for swap-in` | The emergency pool and the allocator were both empty |

Related pages: [scheduler.md](scheduler.md) (locks, kswapd's scheduling), [vfs.md](vfs.md) (page cache, vnode reclaim), [storage.md](storage.md) (swap devices), [processes.md](processes.md) (ELF loading, signals, vDSO), [diagnostics.md](diagnostics.md) (`/proc`, klog).
