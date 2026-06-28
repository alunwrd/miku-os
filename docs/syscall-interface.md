## Syscall Interface

Implemented via `SYSCALL/SYSRET` (MSR-based). Uses `swapgs` for kernel stack switching in a naked asm handler.

### Calling Convention

| Register | Role |
|---|---|
| `rax` | Syscall number |
| `rdi` | Argument 1 |
| `rsi` | Argument 2 |
| `rdx` | Argument 3 |
| `r10` | Argument 4 |
| `rax` (return) | Return value (negative value = error code) |

### Syscall Table

| Nr | Name | Arguments | Description |
|---|---|---|---|
| `0` | `sys_exit` | | Terminate current process and yield |
| `1` | `sys_write` | fd, buf_ptr, len | Write to stdout/stderr (fd 1/2), up to 65536 bytes |
| `2` | `sys_read` | fd, buf_ptr, len | Read from stdin (fd 0) or open file descriptor |
| `3` | `sys_mmap` | addr, len, prot, flags | Map anonymous memory pages |
| `4` | `sys_munmap` | addr, len | Unmap previously mapped pages |
| `5` | `sys_mprotect` | addr, len, prot | Change page protection flags |
| `6` | `sys_brk` | addr | Set program break (heap end) |
| `7` | `sys_getpid` | | Return PID of current process |
| `8` | `sys_getcwd` | buf, size | Write "/" into buf (stub) |
| `9` | `sys_set_tls` | addr | Set FS base register (TLS pointer) |
| `10` | `sys_get_tls` | | Read FS base register |
| `11` | `sys_open` | path_ptr, path_len | Open a file, returns fd |
| `12` | `sys_close` | fd | Close a file descriptor |
| `13` | `sys_seek` | fd, offset | Set read offset for a file descriptor |
| `14` | `sys_fsize` | fd | Return file size in bytes |
| `15` | `sys_map_lib` | name_ptr, name_len | Map a shared library into process address space |
| `16` | `sys_sleep` | ticks | Sleep for N ticks (~4ms/tick at 250 Hz) |
| `17` | `sys_uptime` | | Return ticks since boot |
| `18` | `sys_stat` | path, buf | Get file metadata by path |
| `19` | `sys_fstat` | fd, buf | Get file metadata by fd |
| `20` | `sys_mkdir` | path, mode | Create directory |
| `21` | `sys_rmdir` | path | Remove empty directory |
| `22` | `sys_unlink` | path | Remove file |
| `23` | `sys_readdir` | fd, buf, len | Read directory entries |
| `24` | `sys_rename` | old, new | Rename file / directory |
| `25` | `sys_link` | old, new | Create hard link |
| `26` | `sys_chmod` | path, mode | Change permissions |
| `27` | `sys_chown` | path, uid, gid | Change ownership |
| `28` | `sys_dup` | fd | Duplicate file descriptor |
| `29` | `sys_dup2` | oldfd, newfd | Duplicate to specific fd |
| `30` | `sys_truncate` | fd, len | Truncate file to length |
| `31` | `sys_write_file` | path, buf, len | Write whole file contents |
| `32` | `sys_symlink` | target, link | Create symbolic link |
| `33` | `sys_readlink` | path, buf, len | Read symlink target |
| `34` | `sys_pipe` | fds[2] | Create pipe (read/write fds) |
| `35` | `sys_chdir` | path | Change current directory |
| `36` | `sys_statfs` | path, buf | Filesystem statistics |
| `37` | `sys_fallocate` | fd, off, len | Preallocate file space |
| `38` | `sys_getxattr` | path, name, buf, len | Read extended attribute |
| `39` | `sys_setxattr` | path, name, val, len | Write extended attribute |
| `40` | `sys_utimensat` | fd, path, times | Set file timestamps |
| `41` | `sys_fsync` | fd | Flush file to disk |
| `42` | `sys_punch_hole` | fd, off, len | Punch hole in file |
| `43` | `sys_fork` | | Fork current process |
| `44` | `sys_wait4` | pid, status, opts | Wait for child process |
| `45` | `sys_kill` | pid, sig | Send signal to process |
| `46` | `sys_exec` | path, path_len, argv, argc | Execute ELF binary (no envp; see ABI doc) |
| `47` | `sys_umask` | mask | Set file mode creation mask |
| `48` | `sys_getuid` | | Get real user id |
| `49` | `sys_getgid` | | Get real group id |
| `50` | `sys_geteuid` | | Get effective user id |
| `51` | `sys_getegid` | | Get effective group id |
| `52` | `sys_setuid` | uid | Set user id |
| `53` | `sys_setgid` | gid | Set group id |
| `54` | `sys_seteuid` | uid | Set effective user id |
| `55` | `sys_setegid` | gid | Set effective group id |
| `56` | `sys_socket` | domain, type, proto | Create a socket (TCP/UDP) |
| `57` | `sys_connect` | fd, ip, port | Connect a socket to a remote host |
| `58` | `sys_send` | fd, buf, len, flags | Send data on a socket |
| `59` | `sys_recv` | fd, buf, len, flags | Receive data from a socket |

FD table is currently a single global table in `MikuVFS::fd_table`; per-process isolation is on the roadmap and will arrive together with per-process VFS contexts.

---