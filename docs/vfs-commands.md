## VFS Commands

> In-memory virtual filesystem. Files persist in RAM while the system is running.
> `ls` integrates ext disk content with VFS nodes in a single view. VFS-only nodes (proc, dev, mnt, ram files) are shown with tags `[vfs]` or `[ram]`.

| Command | Syntax | Description |
|---|---|---|
| `ls` | `ls [path]` | List directory (ext disk + VFS combined) |
| `cd` | `cd <path>` | Change current directory |
| `pwd` | `pwd` | Print working directory |
| `mkdir` | `mkdir <path>` | Create a directory in RAM |
| `touch` | `touch <path>` | Create an empty file in RAM |
| `cat` | `cat <file>` | Display file contents |
| `write` | `write <file> <text>` | Write text to a RAM file (overwrites) |
| `stat` | `stat <path>` | Show inode and file metadata |
| `rm` | `rm <path>` | Remove a file |
| `rm -rf` | `rm -rf <path>` | Recursively remove a directory |
| `rmdir` | `rmdir <dir>` | Remove directory (ext-aware: removes from disk too) |
| `mv` | `mv <old> <new>` | Move or rename |
| `ln -s` | `ln -s <target> <linkname>` | Create a symbolic link |
| `ln` | `ln <existing> <newname>` | Create a hard link |
| `readlink` | `readlink <path>` | Read the target of a symlink |
| `chmod` | `chmod <mode> <path>` | Change file permissions (octal) |
| `df` | `df` | Show filesystem disk usage |
| `echo` | `echo <text>` | Print text to the console |
| `history` | `history` | Show last 16 commands |
| `clear` | `clear` | Clear the terminal screen |
| `help` | `help` | Show built-in help |
| `info` | `info` | Show OS version, uptime, memory |

---