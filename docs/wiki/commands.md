# Command reference

Every command on MikuOS: the programs in `/bin`, the builtins of the shell
`msh`, and the commands of the in-kernel console (the emergency shell). For
each, where it runs, its synopsis with every flag its argument parsing
accepts, what it does, and whether it needs privilege. Areas that another page
covers in depth (storage, the network, the NVIDIA driver, services and
packages) are kept short here and link there.

## Contents

- [Where a command runs](#where-a-command-runs)
- [Privilege](#privilege)
- [Files](#files)
- [Text](#text)
- [Processes](#processes)
- [System](#system)
- [Users and privilege](#users-and-privilege)
- [Packages](#packages)
- [Network](#network)
- [Graphics and desktop](#graphics-and-desktop)
- [Development and test programs](#development-and-test-programs)
- [msh builtins](#msh-builtins)
- [Kernel console](#kernel-console)
  - [Console-only basics](#console-only-basics)
  - [Diagnostics](#diagnostics)
  - [Storage and filesystems](#storage-and-filesystems)
  - [Services (sv)](#services-sv)
  - [Network (kernel)](#network-kernel)
  - [GPU and display](#gpu-and-display)
  - [Selftests](#selftests)
  - [Session](#session)
- [Commands moved out of the kernel](#commands-moved-out-of-the-kernel)
- [Where the code is](#where-the-code-is)

---

## Where a command runs

| Kind | What it is | Marked |
|---|---|---|
| **program** | an ELF in `/bin`, a ring-3 process with the caller's credential | (none) |
| **msh builtin** | handled inside `msh` (`cd`, `exit`, `jobs`, `fg`, `bg`, `wait`) or in its child (`echo`, `pwd`, `tty`, `help`) | *builtin* |
| **kernel console** | a command of the in-kernel shell (`kernel/shell/commands/`), running in ring 0 on the console, or on the caller's thread through the `kcmd` syscall (104) | *kernel* |

How a name is resolved in `msh`: a builtin; else `/bin/<name>` (or the path
as written if it contains `/`); else, if the name has no `/` and no
`NAME=value` prefix, the line is sent to the kernel through `kcmd`; else
`msh: <name>: not found in /bin`. The kernel console resolves the other way
round: its own command table first, then `/bin/<name>`. So `nvidia list` works
at an `msh` prompt and `ls` works at the kernel console. Output of a kernel
command goes to the terminal, not through fd 1, so `nvidia list > file` writes
an empty file.

Every program in `/bin` comes from the boot module
([sdk.md](sdk.md#how-it-reaches-bin)); the package each ships in is given in
brackets (`miku-base`, `miku-system`, `miku-desktop`, `miku-net`,
`miku-demos`, `zed`).

## Privilege

There is no setuid bit. A command that needs more than the caller has is run
with **`mikudo`**, which asks the broker `mikudod` ([system.md](system.md)).
Kernel commands gate themselves: `reboot`, `poweroff` and their aliases need
`CAP_SYS_BOOT` (which a logged-in session holds); a list of commands that
change the machine need `CAP_SYS_ADMIN` (which a session does not hold):
everything starting with `ext`, `mkfs.`, `fs.`, `gpt`, `blk`, `swap`,
`mkswap`, and `dd`, `mount` (with arguments), `umount`, `fstrim`,
`partprobe`, `smart`, `nvmestress`, `fiemap`, `chattr`, `lsattr`,
`getxattr`, `setxattr`, `listxattr`, `install`, `nvidia` and `display`
(except their read-only verbs), `usb`, `memmap`, `pstore`, `dmesg`, and `sv`
(except its read-only verbs). Refused, they print:

```
  dmesg: this is a machine command and you are not root
  it changes the machine, so it wants CAP_SYS_ADMIN, which a
  desktop session deliberately does not carry. Ask for it:
     mikudo dmesg ...
```

The read-only verbs that need nothing: `nvidia` with no verb or `info`,
`status`, `list`, `ls`, `gpus`, `temp`, `thermal`, `vram`, `fbmap`, `va`,
`disp`/`outputs` (not `poll`), `util`, `pcie`, `bus`, `firmware`, `rm-tree`,
`gb206`, `next`, `help`, `perf` (not `boost`); `sv` with no verb or `list`,
`status`, `tree`, `rdeps`, `analyze`, `failed`, `cat`, `show`, `help`;
`mount` with no arguments; `blkstat`, `swapinfo`, `usb`/`lsusb`; `display`
with no verb or `list`, `help`; `mayland` with no verb or `status`, `info`,
`list`, `help`. In the tables below **P** marks a command that needs
privilege.

---

## Files

| Command | Synopsis | What it does |
|---|---|---|
| `ls` [miku-base] | `ls [-l] [-a] [-1] [path...]` (`-la`, `-al` combine) | list a directory; `-l` long form (mode, links, owner, size, time), `-a` include dot files, `-1` one per line. Colour and columns only when the output is a terminal |
| `cat` [miku-base] | `cat [-n] [file...]` | print files, or standard input with none; `-n` numbers lines; bytes pass through unchanged |
| `cp` [miku-base] | `cp [-r\|-R] [-v] source... destination` | copy files, or trees with `-r`; the mode is kept, timestamps are not; `-v` names each copy |
| `mv` [miku-base] | `mv [-v] source... destination` | one `rename` per source; refuses across filesystems instead of copying |
| `rm` [miku-base] | `rm [-r\|-R] [-f] [-v] path...` (`-rf`, `-fr`) | remove files; directories only with `-r`; `-f` ignores missing names |
| `mkdir` [miku-base] | `mkdir [-p] [-m mode] dir...` | make directories; `-p` parents too and no error if it exists; `-m` octal mode |
| `rmdir` [miku-base] | `rmdir dir...` | remove empty directories |
| `ln` [miku-base] | `ln [-s] target linkname` | hard link, or symbolic with `-s` |
| `readlink` [miku-base] | `readlink link...` | print where a symbolic link points |
| `touch` [miku-base] | `touch file...` | create files that do not exist, and set an existing file's access and modification times to now (`utimensat` with no times) |
| `stat` [miku-base] | `stat file...` | everything `lstat` says: type, size, blocks, mode, links, owner, inode, device, times |
| `chmod` [miku-base] | `chmod [-R] mode file...` | octal (`644`) or symbolic (`u+x`, `go-w,a+r`: `[ugoa]*[+-=][rwx]*`, comma-separated); no `s`, `t` or `X` |
| `chown` [miku-base] | `chown [-R] uid[:gid] file...` | numeric ids only; giving a file away needs `CAP_CHOWN` (**P** through `mikudo`) |
| `du` [miku-base] | `du [-s] [-b] [path...]` | space used under each path, KiB (`-b` bytes), from file sizes; `-s` totals only |
| `df` [miku-base] | `df [-k] [-b] [path...]` | free space of the filesystem a path is on (default `/`), KiB or bytes |
| `tree` [miku-base] | `tree [-d] [-a] [-L depth] [path]` | a directory tree; `-d` directories only, `-a` dot files, `-L` depth limit |

Storage administration (mounting, partitioning, formatting) is in the
[kernel console](#storage-and-filesystems) and [storage.md](storage.md).

## Text

| Command | Synopsis | What it does |
|---|---|---|
| `echo` [miku-base], *builtin* | `echo [-n] [word...]` | print the words; `-n` no newline (the msh builtin has no `-n`; see [msh builtins](#msh-builtins)) |
| `head` [miku-base] | `head [-n count] [file...]` | the first lines (default 10) |
| `tail` [miku-base] | `tail [-n count] [file...]` | the last lines; `-` is standard input; `-f` is refused |
| `wc` [miku-base] | `wc [-l] [-w] [-c] [file...]` | lines, words, bytes |
| `grep` [miku-base] | `grep [-i] [-n] [-v] [-c] pattern [file...]` (`-in`, `-ni`) | lines containing a **plain substring** (not a regular expression); `-i` ignore case, `-n` line numbers, `-v` invert, `-c` count |
| `sort` [miku-base] | `sort [-r] [-u] [-n] [file...]` | byte order, or numeric with `-n`; `-r` reverse, `-u` unique |
| `uniq` [miku-base] | `uniq [-c] [-d] [file]` | drop repeated adjacent lines; `-c` counts, `-d` only the repeated ones |
| `tee` [miku-base] | `tee [-a] file...` | copy standard input to files and standard output; `-a` append |

## Processes

| Command | Synopsis | What it does |
|---|---|---|
| `ps` [miku-base] | `ps [-f]` (`-l`, `-ef`, `-aux`, `aux` = `-f`; `-e`, `-a`, `-A` accepted) | processes (not threads) from `procinfo`: pid, terminal, CPU time, command; `-f` adds owner, parent, job, state and the full path (`proc_path`) |
| `kill` [miku-base] | `kill [-s sig] pid...`, `kill -sig pid...`, `kill -l` | send a signal (default `TERM`) by number or name; a negative pid is a process group; `-l` lists the names |
| `sleep` [miku-base] | `sleep seconds` | wait; Ctrl-C ends it |
| `jobs`, `fg`, `bg`, `wait` | *builtin* | [msh builtins](#msh-builtins) |
| `top` | *kernel* | every task with CPU%, refreshed every 4 s ([diagnostics.md](diagnostics.md#others)) |
| `nice` | *kernel*: `nice <pid> <1-20>` | change a task's priority |
| `affinity` | *kernel*: `affinity <pid> <hex_mask>` | pin a task to CPUs |

## System

| Command | Synopsis | What it does |
|---|---|---|
| `date` [miku-base] | `date [+seconds]` (`-u` accepted) | the wall clock in UTC; `+seconds` prints Unix seconds. Setting the clock is `ntpdate` |
| `uptime` [miku-base] | `uptime` | time of day, time since boot and process count, from `/proc/stat` |
| `uname` [miku-base] | `uname [-a]` | system name, or everything `/proc/version` says |
| `free` [miku-base] | `free` | memory from `/proc/meminfo`, KiB |
| `env` [miku-base] | `env` | print the environment (it does not run commands; use `NAME=value cmd` in msh) |
| `which` [miku-base] | `which name...` | `/bin/<name>` if it exists (msh's only search directory) |
| `clear` [miku-base] | `clear` | blank the terminal through `tty(TTY_CLEAR)` |
| `mikuctl` [miku-base] | `mikuctl start\|stop\|restart\|reload <service>`, `mikuctl enable\|disable <service>`, `mikuctl status [service]`, `mikuctl list`, `mikuctl notify --ready\|--watchdog` | mikuD's services, with the verbs checked in ring 3 and forwarded through `kcmd`; changing verbs need `CAP_SYS_ADMIN` (**P**: `mikudo mikuctl restart netd`). `notify` is run from inside a service and needs nothing: it reports that service ready or alive (`sv notify`). [system.md](system.md#service-commands) |
| `info`, `heap`, `memmap` (**P**), `dmesg` (**P**), `pstore` (**P**) | *kernel* | [diagnostics.md](diagnostics.md#console-diagnostic-commands) |
| `reboot`, `restart` / `poweroff`, `shutdown`, `halt` | *kernel*, need `CAP_SYS_BOOT` | stop services through mikuD, then reset or power off |

## Users and privilege

These are covered in [system.md](system.md) and
[MikuOS_Security.md](../MikuOS_Security.md).

| Command | Synopsis | What it does |
|---|---|---|
| `id` [miku-system] | `id [-u] [-g] [-G] [-n] [user]` | ids, groups and the capability sets; `-u`, `-g`, `-G` one field, `-n` names instead of numbers |
| `mikudo` [miku-system] | `mikudo [-u user] [-n] [-s] [-l] [-c] [-L] [--] command [args...]` | run a command as root or `-u user`, if `/etc/mikudo.conf` allows; `-n` never prompt, `-s` the target's shell, `-l` list what you may run, `-c` check without running, `-L` forget this terminal's remembered authentication. A command that is not in `/bin` is run as a kernel command (`kernel:<name>` in the policy) |
| `su` [miku-system] | `su [-] [user] [-c command]` (`-l`, `--login`, `-s`, `-u`) | a front for `mikudo -u user -s` / `-c` |
| `passwd` [miku-system] | `passwd [user]` | change a password through `mikudod`, which checks the old one; another user's needs policy permission |
| `useradd` [miku-system] | `useradd [-u uid] [-g gid] [-G group,group] [-s shell] [-d home] name`, `useradd -D name` (`--delete`) | add or delete an account in `/etc/passwd` and `/etc/group`; run it under `mikudo`; no password option (use `passwd`) |
| `login` [miku-system] | `login` | the console login prompt; started as `greeter` by the kernel console's `login` (run at boot when `/bin/login` exists), asks `mikudod` |
| `mikudod` [miku-system] | (service) | the privilege broker; not run by hand |
| `login`, `logout`, `who` | *kernel* | the console's own session ([Session](#session)) |
| `cred` | *kernel*: `cred [show\|test]` | the console's credential, or the credential selftest |

## Packages

| Command | Synopsis | What it does |
|---|---|---|
| `mikupkg` [miku-base] | `mikupkg list [-e\|-d]`, `info <pkg>`, `files <pkg>`, `owner <path>`, `why <pkg>`, `verify [pkg]`, `search <text>`, `add <file.mpk>...`, `remove <pkg>...`, `tx recover`, `clean`; global `--dry-run` (`-n`), `--allow-unsigned` | query and change the installed packages; the changing verbs need a writable root (`root=... rootrw` on the boot line) and run `mikupkg-apply` through `mikudo`. [system.md](system.md#mikupkg) |
| `mikupkg-apply` [miku-base] | `mikupkg-apply [--root DIR] [--allow-unsigned] ...` | the root half that verifies signatures and file hashes and writes; run by `mikupkg`, not by hand |

## Network

The stack and the programs are described in [network.md](network.md).

| Command | Synopsis | What it does |
|---|---|---|
| `ping` [miku-net] | `ping <host\|address> [count]` | ICMP echo; Ctrl-C stops |
| `traceroute` [miku-net] | `traceroute <host\|address> [max-hops]` | rising TTLs over ICMP |
| `host` [miku-net] | `host <name> [server]` | DNS lookup |
| `wget` [miku-net] | `wget <url> [-O <file>] [--insecure] [-v]` | fetch a URL (HTTP, HTTPS with certificate checks against `/etc/ssl/certs/ca-bundle.pem`) |
| `curl` [miku-net] | `curl <url> [-X GET\|POST] [-d <data>] [-o <file>] [-I] [--insecure] [-v]` | one HTTP request; `-I` headers only |
| `ntpdate` [miku-net] | `ntpdate [server] [-q]` | set the wall clock from NTP (`-q` query only); setting needs `CAP_SYS_TIME` (**P**) |
| `dhcp`, `net ...` | *kernel* | [Network (kernel)](#network-kernel) |

## Graphics and desktop

The display server is in the kernel ([mayland.md](mayland.md)); these are its
clients and tools.

| Command | Synopsis | What it does |
|---|---|---|
| `maywm` [miku-desktop] | (session program) | the window manager, configured by `/etc/mayland/` and the user's `may.conf` ([mayland.md](mayland.md)) |
| `mayterm` [miku-desktop] | (session program) | the terminal window: a pty with `msh` on it ([input-and-terminals.md](input-and-terminals.md)) |
| `maygreet` [miku-desktop] | (started by mikuD) | the graphical login screen, asking `mikudod` |
| `maysession` [miku-desktop] | (started by `mikudod` at login) | the session leader: starts `/etc/mayland/session.d/*.conf` and stays alive; the session ends when it exits |
| `aosh` [miku-desktop] | `aosh [file.qml]`, `aosh -c NAME`, `aosh --eval '<js>'`, `aosh check <file\|dir>...`, `aosh ipc [-c NAME] call <target> <function> [args...]`, `aosh ipc [-c NAME] show`, `aosh run [file.qml]`, `aosh eval <file.qml> '<expr>'`, `aosh shot <file.qml> [...]` | the QML desktop shell (bar, launcher, overview) and its tools ([aosh.md](aosh.md)) |
| `zed` [zed] | `zed` | the Zed editor (Ctrl-W on the desktop) |
| `mayland`, `display`, `nvidia` | *kernel* | [GPU and display](#gpu-and-display) |

## Development and test programs

All in `miku-demos`. They are proofs that a part of the system works, kept on
the image, and most are also run headless with `run=` ([testing.md](testing.md)).

| Program | Crate | What it checks |
|---|---|---|
| `hello` | apps | the minimal no_std program |
| `maydemo` | apps | a Mayland client window |
| `gpudemo` | apps | the GPU system calls, refusals included, and a window in video memory |
| `polltest` | apps | `poll` on pipes, sockets, socketpairs and the terminal |
| `abitest` | apps | system call ABI smoke tests |
| `crashtest` | apps | crash reports and core files, on purpose |
| `threadtest` | apps | threads, shared memory and descriptors, futex wait/wake/timeout, a contended mutex |
| `stdhello`, `stdthreads`, `stdfs`, `stdproc`, `stdpoll`, `stdasync`, `stdtokio`, `stdc`, `stdsignal`, `stdwatch`, `stddns`, `stdpty`, `stdremote`, `stdbench` | stdapps | the Rust `std` port, the async runtimes, C dependencies, signals, inotify, DNS, ptys, Zed's remote server, and `stdbench` the benchmark compared against Linux ([sdk.md](sdk.md#crates-verified-on-the-machine)) |

Kernel console development commands: `exec <path> [args...]` (run an ELF on
the console), `elf info|syms|addr|core|cache|test`, `ldconfig`, `ldd <path>`
(shared-library resolution, see [diagnostics.md](diagnostics.md)).

---

## msh builtins

The full shell language is in [msh.md](msh.md).

| Builtin | Synopsis | What it does | Runs in |
|---|---|---|---|
| `cd` | `cd [dir]` | change directory; no argument means `/` (not `$HOME`) | the shell |
| `exit`, `quit` | `exit [code]` | end the shell (and its terminal window) | the shell |
| `jobs` | `jobs` | list background and stopped jobs | the shell |
| `fg` | `fg [n]` | resume job `n` (default the current one) in the foreground | the shell |
| `bg` | `bg [n]` | resume job `n` in the background | the shell |
| `wait` | `wait` | wait for every background job | the shell |
| `echo` | `echo [word...]` | print the words (no options; `-n` is printed) | a child |
| `pwd` | `pwd` | print the working directory | a child |
| `tty` | `tty` | the terminal id, this shell's job and the foreground job | a child |
| `help` | `help` | a summary of the shell | a child |

---

## Kernel console

The in-kernel shell (`kernel/shell/`) is the **emergency console**: the
prompt a text boot shows, or the one a `noauth` boot drops into. On a
graphical boot it is not on screen; its commands are reached from `msh`
through `kcmd`, and privileged ones with `mikudo`. Commands are looked up in
its table first and then in `/bin`. Everything in this section is *kernel*.

### Console-only basics

| Command | Synopsis | What it does |
|---|---|---|
| `cd`, `pwd` | `cd <dir>`, `pwd` | the console's own working directory |
| `write` | `write <file> <text>` | write text to a file |
| `dd` **P** | `dd if=<src> of=<dst> [bs=N] [count=N] [skip=N] [seek=N] [conv=notrunc,fsync]` | copy between files and devices |
| `exec` | `exec <path> [args...]` | run an ELF as the console's identity and wait for it |
| `history` | `history` | the console's command history |
| `help` | `help` | the command list |
| `info` | `info` | version, vnodes, mounts, heap, slabs, RAM, swap, uptime |

### Diagnostics

`info`, `top`, `heap`, `memmap` **P**, `mm test`, `dmesg` **P**, `pstore`
**P**, `cred`, `tty [list|test]`, `ipc [status|test]`, `elf
info|syms|addr|core|cache|test`, `ldconfig`, `ldd <path>`, `swaptest`, `who`,
`history`. All of them are described in
[diagnostics.md](diagnostics.md#console-diagnostic-commands).

### Storage and filesystems

All **P** except `mount` with no arguments, `blkstat` and `swapinfo`.
Details and the disk-safety rules are in [storage.md](storage.md) and
[filesystems.md](filesystems.md).

| Command | Synopsis |
|---|---|
| `mount` | `mount` (list), `mount ext<2\|3\|4> <path>` (mount the selected volume at a path) |
| `umount` | `umount <path>` |
| `ext2mount`, `ext3mount`, `ext4mount` | `ext2mount [drive 0-7] [partition] [rw]` (read-only unless `rw`) |
| `fs.list`, `fs.select`, `fs.rw`, `fs.ro`, `fs.umount` | `fs.list`, `fs.select <0\|1>`, `fs.rw [slot]`, `fs.ro [slot]`, `fs.umount [0\|1]` |
| `extls`, `extcat`, `extstat`, `extinfo`, `extwrite`, `extappend`, `exttouch`, `extmkdir`, `extrm`, `extrmdir`, `extmv`, `extcp`, `extln`, `extlink`, `extchmod`, `extchown`, `extdu`, `exttree` | the active ext volume, whatever its version: `extcat <path>`, `extwrite <path> <text>`, `extrm [-rf] <path>`, `extmv <path> <newname>`, `extcp <src> <dst>`, `extln -s <target> <linkname>`, `extlink <existing> <linkname>`, `extchmod <mode> <path>`, `extchown <uid> <gid> <path>` |
| `ext2*`, `ext3*`, `ext4*` | the same operations for one version: `ext2ls`, `ext2cat`, `ext2stat`, `ext2info`, `ext2write`, `ext2append`, `ext2touch`, `ext2mkdir`, `ext2rm`, `ext2rmdir`, `ext2mv`, `ext2cp`, `ext2ln`, `ext2link`, `ext2chmod`, `ext2chown`, `ext2du`, `ext2tree`, `ext2fsck`, `ext2cache`, `ext2cacheflush`; `ext3ls`.. `ext3du`; `ext4ls`, `ext4cat`, `ext4stat`, `ext4info`, `ext4sync`, `ext4write`, `ext4append`, `ext4mkdir`, `ext4rm`, `ext4rmdir`, `ext4cp`, `ext4tree`, `ext4du`, `ext4fsck` |
| `extfsck` | `extfsck [-y]` (check, `-y` repair) |
| `extcache`, `extcacheflush`, `extsync`, `sync` | block cache statistics, flush, sync |
| ext3 journal | `ext3mkjournal`, `ext3info`, `ext3journal`, `ext3recover`, `ext3clean` |
| ext4 | `ext4extents`, `ext4checksums`, `ext4extinfo <path>` |
| attributes | `fiemap <path>`, `getxattr <path> <name>`, `setxattr <path> <name> <value>`, `listxattr <path>`, `chattr <+/-flags> <path>` (`i` immutable, `a` append, `d` nodump, `A` noatime), `lsattr <path>` |
| `mkfs.ext2`, `mkfs.ext3`, `mkfs.ext4` | `mkfs.ext4 <drive 0-3> [partition\|sectors]` |
| `mkfs.dry` | `mkfs.dry <drive 0-3> <ext2\|ext3\|ext4>` (plan only) |
| `gpt`, `gpt.init`, `gpt.add`, `gpt.del` | `gpt <drive>`, `gpt.init <drive>`, `gpt.add <drive> <fs\|swap> <size_mb> [name]`, `gpt.del <drive> <partition>` |
| `partprobe` | `partprobe [drive 0-7]` |
| swap | `mkswap <drive> <partition>`, `swapon <drive> <partition>`, `swapoff`, `swapinfo`, `mkswap.raw <drive> <start_lba> <size_mb>`, `swapon.raw <drive> <start_lba> <size_sectors>`, `swapon.auto` |
| block devices | `blkstat`, `blkdiscard <drive 0-7> [lba count]`, `blkzero <drive 0-7> <lba> <count>`, `blkonline <drive 0-7>`, `fstrim`, `smart <drive 0-7>`, `nvmestress` |
| `install` | `install plan <drive>` (what it would do, writing nothing), `install go <drive> <size>` (the size from `plan` must match), `install gui` ([system.md](system.md#the-installer)) |

### Services (sv)

`sv` is mikuD's control command ([system.md](system.md#service-commands)).
Read verbs need nothing; the rest are **P**.

```
sv [list|ls] | status [name] | tree <name> | rdeps <name> | analyze | cat <name>
sv start | stop | restart | reload <name>
sv enable | disable | mask | unmask | force-stop (kill) <name>
sv journal (log) [name]
sv target [name] | sv isolate <sysinit|multi-user|graphical|rescue>
sv load <path.service> | sv scan
sv timer <list|start|stop|remove> [name]
sv socket (sock) <list|stop|remove> [name]
```

The privilege gate also lists `failed` and `show` as read-only verbs, but
`sv` itself has no such verbs.

### Network (kernel)

[network.md](network.md) has the details.

| Command | Synopsis | What it does |
|---|---|---|
| `dhcp` | `dhcp` | get an address by DHCP |
| `net` | `net status`, `net pci`, `net poll`, `net arp`, `net dns [<ip>]`, `net ip <ip> <gw> <mask>`, `net send <ip> <port> <msg>`, `net test` | adapter state, PCI NICs, receive, ARP table, DNS server, static address, a UDP packet, the selftest |

### GPU and display

| Command | Synopsis | What it does |
|---|---|---|
| `nvidia`, `gpu` | `nvidia [verb ...]` | the NVIDIA driver: status, temperatures, video memory, P-states (`perf`, `perf boost` **P**), the display engine, GSP (`gsp unload\|load\|reset` **P**), the log (`log ...` **P**), firmware, and many bring-up verbs. Read-only verbs need nothing ([Privilege](#privilege)). Full reference: [Nvidia_gb206.md](../Nvidia_gb206.md) |
| `display` | `display [list]`, `display mode <output> <n> [test]`, `display on\|off <output> [test]`, `display vrr on\|off [<output>] [test]`, `display color ...`, `display help` | outputs and modes; setters **P** ([mayland.md](mayland.md)) |
| `mayland`, `may` | `mayland [status\|start\|stop\|demo [on\|off]]`, `mayland pointer <x> <y>` | the display server's state, start, stop; `status` needs nothing |
| `usb`, `lsusb` | `usb` | xHCI controllers and USB devices |

### Selftests

Each can also be run on the boot line with `run=<name>_test` (see
[testing.md](testing.md)): `mm test`, `ext test`, `btrfs test`, `vfs test`,
`cred test`, `tty test`, `ipc test`, `elf test`, `net test`, `dmesg
selftest`, and the GPU's `nvidia` self tests.

### Session

| Command | What it does |
|---|---|
| `login` | start a console session: run `/bin/login` as the `greeter` account, which asks `mikudod`; on success the console takes the user's identity |
| `logout` | end the console session; the console does not become root again on a graphical boot |
| `who` | the open sessions |

`login` and `logout` cannot be sent through `kcmd` (`EPERM`): they are the
console's own state.

---

## Commands moved out of the kernel

The rule is **move, don't duplicate**: a command ported to ring 3 is deleted
from the kernel. These were kernel console commands and are programs now,
still reachable by name from the console through its `/bin` fall-through:

- files and text: `ls`, `cat`, `cp`, `mv`, `rm`, `mkdir`, `rmdir`, `ln`,
  `readlink`, `touch`, `stat`, `chmod`, `chown`, `du`, `df`, `tree`, `head`,
  `tail`, `wc`, `grep`, `sort`, `uniq`, `tee`, `echo`;
- processes and system: `ps`, `kill`, `sleep`, `date`, `uptime`, `uname`,
  `free`, `clear`, `which`, `env`;
- network: `ping`, `traceroute`, `wget`, `curl`, `host`, `ntpdate`.

The console's `help` still lists `ps` and `kill` under "System" with kernel
descriptions; both are the `/bin` programs.

---

## Where the code is

| What | Where |
|---|---|
| Which binaries go into `/bin`, and their packages | `builder/src/main.rs` (`SYSTEM_BINS`, `UTIL_BINS`, `APP_BINS`, `STD_BINS`, `AOSH_BINS`, `EXTERNAL_BINS`) |
| Everyday commands | `utils/src/*.rs` |
| System programs | `system/src/` (`msh/`, `maywm/`, `mayterm/`, `mikupkg/`, `mikudo.rs`, `mikudod.rs`, `login.rs`, ...) |
| Network tools, demos, tests | `apps/src/` |
| std programs | `stdapps/src/` |
| The desktop shell | `aosh/` |
| Kernel console dispatcher and privilege gate | `kernel/shell/commands/mod.rs` (`execute`, `gated_by`, `read_only`, `admin_only`, `run_external`) |
| Kernel console commands | `kernel/shell/commands/*.rs` |
| The `kcmd` syscall | `kernel/syscall/adapters/admin.rs` |
