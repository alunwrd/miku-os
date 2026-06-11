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

---