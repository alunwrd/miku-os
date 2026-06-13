## Ext3 Commands

> Ext3 adds journaling on top of ext2. Use `ext3mkjournal` to upgrade an existing ext2 filesystem.

| Command | Syntax | Description |
|---|---|---|
| `ext3mount` | `ext3mount [drive] [partition]` | Mount ext3 filesystem |
| `ext3ls` | `ext3ls [path]` | List directory |
| `ext3cat` | `ext3cat <path>` | Show file contents |
| `ext3stat` | `ext3stat <path>` | Show inode details |
| `ext3write` | `ext3write <path> <text>` | Write to file |
| `ext3append` | `ext3append <path> <text>` | Append text to file |
| `ext3mkdir` | `ext3mkdir <path>` | Create directory |
| `ext3rm` | `ext3rm <path>` | Delete file |
| `ext3rmdir` | `ext3rmdir <path>` | Delete empty directory |
| `ext3tree` | `ext3tree [path]` | Show directory tree |
| `ext3du` | `ext3du [path]` | Show disk usage |
| `ext3mkjournal` | `ext3mkjournal` | Create journal (convert ext2 to ext3) |
| `ext3info` | `ext3info` | Show journal metadata and status |
| `ext3journal` | `ext3journal` | Show journal transactions |
| `ext3recover` | `ext3recover` | Replay the journal (crash recovery) |
| `ext3clean` | `ext3clean` | Mark the journal as clean |

---