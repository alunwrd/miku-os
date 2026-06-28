## GPT Partitioning

MikuOS has a built-in GPT partition manager.

### Commands

| Command | Syntax | Description |
|---|---|---|
| `gpt` | `gpt <drive>` | Show partition table for a drive |
| `gpt.init` | `gpt.init <drive>` | Initialize a new GPT on a drive (destroys all data) |
| `gpt.add` | `gpt.add <drive> <type> <size_mb>` | Add a new partition |
| `gpt.del` | `gpt.del <drive> <partition>` | Delete a partition |

### Partition Types

| Type | Description |
|---|---|
| `swap` | Linux Swap partition |
| `fs` | Linux filesystem (ext2/3/4) |

### Example

```bash
gpt.init 1
gpt.add 1 swap 256
gpt.add 1 fs 3072
gpt 1
```

Expected output:

```
GPT partition table -- disk 1
Disk size: 8388608 sectors (4096 MB)

[ 1]  Linux Swap    34-524321      256 MB
[ 2]  Linux FS      524322-6815777 3072 MB
```

> Partition numbers start at **1**. GPT uses full 64-bit LBA - disks larger than 2 TB are supported.

---