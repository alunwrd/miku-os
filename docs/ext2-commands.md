## Ext2 Commands

> Version-specific commands. Use [Unified Ext Commands](#unified-ext-commands) for day-to-day work.
> Always run `ext2mount` before any `ext2*` command.

| Command | Syntax | Description |
|---|---|---|
| `ext2mount` | `ext2mount [drive] [partition]` | Mount ext2 filesystem |
| `ext2info` | `ext2info` | Show superblock and filesystem info |
| `ext2ls` | `ext2ls [path]` | List directory |
| `ext2cat` | `ext2cat <path>` | Show file contents |
| `ext2stat` | `ext2stat <path>` | Show inode details |
| `ext2write` | `ext2write <path> <text>` | Write to file (overwrites) |
| `ext2append` | `ext2append <path> <text>` | Append text to file |
| `ext2touch` | `ext2touch <path>` | Create empty file |
| `ext2mkdir` | `ext2mkdir <path>` | Create directory |
| `ext2rm` | `ext2rm <path>` | Delete file |
| `ext2rm -rf` | `ext2rm -rf <path>` | Recursively delete |
| `ext2rmdir` | `ext2rmdir <path>` | Delete empty directory |
| `ext2mv` | `ext2mv <path> <newname>` | Rename file |
| `ext2cp` | `ext2cp <src> <dst>` | Copy file |
| `ext2ln -s` | `ext2ln -s <target> <linkname>` | Create symbolic link |
| `ext2link` | `ext2link <existing> <linkname>` | Create hard link |
| `ext2chmod` | `ext2chmod <mode> <path>` | Change file permissions |
| `ext2chown` | `ext2chown <uid> <gid> <path>` | Change file owner |
| `ext2du` | `ext2du [path]` | Show disk usage |
| `ext2tree` | `ext2tree [path]` | Show directory tree |
| `ext2fsck` | `ext2fsck` | Check filesystem integrity |
| `ext2cache` | `ext2cache` | Show block cache statistics |
| `ext2cacheflush` | `ext2cacheflush` | Flush block cache to disk |

---