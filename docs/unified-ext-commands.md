## Unified Ext Commands

> These commands automatically detect the version of the mounted filesystem (ext2, ext3, or ext4) and call the correct implementation. Use these instead of version-specific commands.

> All paths are resolved relative to the current working directory. Absolute paths are not required.

### Mounting

| Command | Syntax | Description |
|---|---|---|
| `ext2mount` | `ext2mount [drive] [partition]` | Mount ext2 |
| `ext3mount` | `ext3mount [drive] [partition]` | Mount ext3 |
| `ext4mount` | `ext4mount [drive] [partition]` | Mount ext4 |

### File and Directory Operations

| Command | Syntax | Description |
|---|---|---|
| `extls` | `extls [path]` | List directory contents |
| `extcat` | `extcat <path>` | Show file contents |
| `extstat` | `extstat <path>` | Show inode details |
| `extinfo` | `extinfo` | Show superblock and filesystem info |
| `extwrite` | `extwrite <path> <text>` | Write to file (overwrites) |
| `extappend` | `extappend <path> <text>` | Append text to file |
| `exttouch` | `exttouch <path>` | Create empty file |
| `extmkdir` | `extmkdir <path>` | Create directory |
| `extrm` | `extrm [-rf] <path>` | Delete file (or recursively with `-rf`) |
| `extrmdir` | `extrmdir <path>` | Delete empty directory |
| `extmv` | `extmv <path> <newname>` | Rename file |
| `extcp` | `extcp <src> <dst>` | Copy file |
| `extln -s` | `extln -s <target> <linkname>` | Create symbolic link |
| `extlink` | `extlink <existing> <linkname>` | Create hard link |
| `extchmod` | `extchmod <mode> <path>` | Change file permissions (octal) |
| `extchown` | `extchown <uid> <gid> <path>` | Change file owner |

### Inspection and Maintenance

| Command | Syntax | Description |
|---|---|---|
| `extdu` | `extdu [path]` | Show disk usage |
| `exttree` | `exttree [path]` | Show directory tree |
| `extfsck` | `extfsck` | Check filesystem integrity |
| `extcache` | `extcache` | Show block cache statistics |
| `extcacheflush` | `extcacheflush` | Flush block cache to disk |
| `extsync` | `extsync` | Flush all pending writes to disk |
| `sync` | `sync` | Alias for `extsync` |

### Usage Examples

```bash
ext4mount 1 2

extls
extmkdir mydir
exttouch mydir/hello.txt
extwrite mydir/hello.txt "hello world"
extcat mydir/hello.txt
extappend mydir/hello.txt " from mikuos"
extcat mydir/hello.txt

extcp mydir/hello.txt mydir/backup.txt
extmv mydir/backup.txt mydir/renamed.txt
extls mydir

extchmod 0755 mydir/hello.txt
extchown 1 1 mydir/hello.txt
extstat mydir/hello.txt

extrm mydir/renamed.txt
extrmdir mydir

extdu
exttree
extfsck
sync
```

> Legacy version-specific commands (`ext2ls`, `ext3cat`, `ext4write`, etc.) are kept as aliases for backward compatibility but are no longer the recommended way to work with the filesystem.

---