## Ext4 Commands

> Ext4 features: extent trees, metadata checksums (crc32c), 64-bit support, flex block groups.

| Command | Syntax | Description |
|---|---|---|
| `ext4mount` | `ext4mount [drive] [partition]` | Mount ext4 filesystem |
| `ext4ls` | `ext4ls [path]` | List directory |
| `ext4cat` | `ext4cat <path>` | Show file contents |
| `ext4stat` | `ext4stat <path>` | Show inode details |
| `ext4write` | `ext4write <path> <text>` | Write to file |
| `ext4append` | `ext4append <path> <text>` | Append text to file |
| `ext4mkdir` | `ext4mkdir <path>` | Create directory |
| `ext4rm` | `ext4rm <path>` | Delete file |
| `ext4rmdir` | `ext4rmdir <path>` | Delete empty directory |
| `ext4cp` | `ext4cp <src> <dst>` | Copy file |
| `ext4tree` | `ext4tree [path]` | Show directory tree |
| `ext4du` | `ext4du [path]` | Show disk usage |
| `ext4fsck` | `ext4fsck` | Check filesystem integrity |
| `ext4sync` | `ext4sync` | Flush pending writes to disk |
| `ext4info` | `ext4info` | Show ext4 feature flags and checksum status |
| `ext4extents` | `ext4extents` | Enable extent tree feature |
| `ext4checksums` | `ext4checksums` | Verify superblock, group desc, and inode checksums |
| `ext4extinfo` | `ext4extinfo <path>` | Show extent tree info for a file |

---