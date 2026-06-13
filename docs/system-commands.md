## System Commands

| Command | Aliases | Description |
|---|---|---|
| `info` | | Show OS version, uptime, VNode, heap and RAM stats |
| `heap` | | Show detailed heap allocator statistics |
| `memmap` | | Show physical memory map from GRUB (base, length, type) |
| `ps` | | Show running processes with CPU%, stack usage, context switches |
| `top` | | Live process monitor sorted by CPU usage (Ctrl+C to exit) |
| `kill` | | `kill <pid>` - terminate a process |
| `nice` | | `nice <pid> <priority>` - change process priority (1-20) |
| `affinity` | | `affinity <pid> <mask>` - set CPU affinity bitmask |
| `swaptest` | | Allocate pages, swap them out and verify correctness |
| `ldconfig` | | Rebuild the shared library cache |
| `ldd` | | `ldd <path>` - show shared library dependencies of an ELF binary |
| `exec` | | `exec <path> [args...]` - load and run an ELF binary |
| `history` | | Show last 16 executed commands |
| `reboot` | `restart` | Restart the system (flushes filesystem before reboot) |
| `poweroff` | `shutdown`, `halt` | Shut down the system (flushes filesystem before poweroff) |

---