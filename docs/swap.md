## Swap

### Setup

```bash
mkswap 1 1
swapon 1 1
swapinfo
```

### Commands

| Command | Syntax | Description |
|---|---|---|
| `mkswap` | `mkswap <drive> <partition>` | Format a partition as swap |
| `mkswap.raw` | `mkswap.raw <drive> <lba> <mb>` | Format swap at a raw LBA offset |
| `swapon` | `swapon <drive> <partition>` | Activate swap on a partition |
| `swapon.raw` | `swapon.raw <drive> <lba> <sectors>` | Activate swap at a raw LBA offset |
| `swapon.auto` | `swapon.auto` | Scan all drives and activate swap automatically |
| `swapoff` | `swapoff` | Deactivate swap |
| `swapinfo` | `swapinfo` | Show swap usage: total / used / free pages |
| `swaptest` | `swaptest` | Self-test: swap out 256 pages with patterns, swap back in, byte-verify |

### Notes

- Swap data persists on disk. `mkswap` only needs to be run once.
- After reboot just run `swapon 1 1` again.
- `swapinfo` shows how many 4 KB pages are in use.

---