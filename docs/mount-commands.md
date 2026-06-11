## Mount Commands

| Command | Syntax | Description |
|---|---|---|
| `mount` | `mount` | List all currently mounted filesystems |
| `mount ext2` | `mount ext2 <path>` | Mount ext2 filesystem at a path |
| `umount` | `umount <path>` | Unmount a filesystem |

### Filesystem Overview

| FS | Mount point | Description |
|---|---|---|
| **tmpfs** | `/` | Root RAM filesystem |
| **devfs** | `/dev` | Device files |
| **procfs** | `/proc` | Virtual system info |
| **ext2/3/4** | slot 0 / slot 1 | Real disk filesystems |

---