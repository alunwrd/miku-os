# The MikuOS permission model

*One coherent system for the person using it; an open platform for the person
building on it.*

That sentence decides everything in this document. For the ordinary user,
permissions are meant to be invisible: they log in, their files are theirs, and
when something needs the machine rather than their account, `mikudo` asks once
and gets on with it. For the developer, none of it is hidden: the policy is a
text file with a grammar that fits on a page, every decision is journalled with
the line that produced it, and the kernel primitives underneath are ordinary
documented syscalls with no magic anywhere in them.

How the pieces are run day to day (services, accounts, logging in, packages)
is in [wiki/system.md](wiki/system.md); the system calls are in
[MikuOS_ABI.md](MikuOS_ABI.md). This document is the model and the argument
for it.

## Contents

1. [The invariant](#1-the-invariant)
2. [How it fits together](#2-how-it-fits-together)
3. [Sessions](#3-sessions)
4. [Credentials](#4-credentials)
5. [`/etc/mikudo.conf`](#5-etcmikudoconf)
6. [Passwords](#6-passwords)
7. [Accounts](#7-accounts)
8. [What else changed](#8-what-else-changed)
9. [Availability](#9-availability)
10. [Verifying it](#10-verifying-it)
11. [Using it](#11-using-it)

---

## 1. The invariant

> **Privilege is granted by a privileged process to a child. It is never
> acquired from a file.**

There is no setuid bit on MikuOS, no setgid bit, and no file capabilities.
`exec` reads a file's mode bits, `ls` renders them, and the loader ignores
them. On every `exec` the process's permitted capability set is intersected
with its bounding set and the effective set is replaced by the result — the
sets can stay the same or shrink, and there is no argument by which they grow.

This is not a simplification. It is the whole security argument, and everything
below is a consequence of it.

Every privilege-escalation advisory sudo, doas and pkexec have had needed the
same ingredient: **a process that became root while holding input the attacker
chose.** Once that has happened, every mistake in the program is worth root.

| Advisory | What it was | Why it cannot happen here |
|---|---|---|
| CVE-2021-4034 (PwnKit) | `pkexec` with `argc == 0` indexed `argv[0]`, walked into the environment behind it, and reintroduced a variable the loader reads | The privileged process never receives an argv. `mikudod` builds one, and `argc == 0` is refused before anything else is looked at |
| CVE-2021-3156 (Baron Samedit) | A heap overflow in `sudoedit`'s argv unescaping, in a process that had already become root | The code that parses a request has never been anything but root — it did not become root by being asked, so there is no untrusted argv to overflow on |
| CVE-2019-14287 | `sudo -u#-1`: the id parsed to `(uid_t)-1`, `setuid` returned success without changing anything | Every id crossing the syscall boundary goes through `checked_id`, which refuses anything above 65534 rather than truncating. `login` and `mikudod` also *verify* the transition took, rather than assuming it |
| CVE-2019-18634 | A buffer overflow in `pwfeedback` reading a password from something that was not a terminal | The password reaches `mikudod` as a framed message on the socket, into a fixed buffer sized at compile time, and one that would not fit is refused rather than truncated - a shortened password is a *different* password and the person typing it would have no idea |
| CVE-2023-22809 | `sudoedit` honoured a `--` inside `$EDITOR`, letting a user open files the rule did not name | The child's environment is *built*, not filtered. Nothing from the caller reaches it unless a rule names the variable, and a named variable is validated |
| CVE-2025-32463 | `sudo -R`: a chroot the user controlled, from which `/etc/nsswitch.conf` was then read | There is no option that changes where the privileged process reads its configuration from: `mikudod` reads `/etc/mikudo.conf` and nothing else |
| The whole `LD_PRELOAD` / `TZ` / `IFS` family | A setuid binary inheriting an environment that its loader or libc acts on before `main` | Nothing is inherited. `mikudod` is started by mikuD with an environment mikuD built |
| Ticket-file races (symlinks, TOCTOU, clock rollback) | sudo's timestamp files, in `/var/run`, raced or tampered with | The persist token lives in `mikudod`'s memory, keyed to the caller and its session, expiring on the monotonic clock. There is no file, so there is nothing to race, and moving the wall clock does not extend it |

---

## 2. How it fits together

```
user ──exec──> /bin/mikudo          uid=1000, no capability whatsoever
                   │
                   │  connect  /run/mikudo.sock       (a 0755 directory: only
                   │  request  op, target, argv, cwd   root may bind a name in it)
                   ▼
              /bin/mikudod          uid 0, every capability, started by mikuD
                   │
                   │  peercred(fd) ── the kernel names the caller. Snapshotted
                   │                   at connect, so pid reuse cannot fool it.
                   │                   The request has NO uid field to lie in.
                   │
                   │  /etc/mikudo.conf   root-owned, not group/other writable,
                   │                     last match wins, no match denies,
                   │                     a parse error denies everything
                   │
                   │  ── NeedPassword ──▶  mikudo prompts (echo off)
                   │  ◀── the password ──  and sends it back
                   │
                   │  verify against /etc/shadow (Argon2id, constant-time),
                   │  remember in RAM for 5 minutes
                   ▼
              fork ─> setgroups → setresgid → setresuid → verify → chdir → exec
                      environment built here; argv[0] written here
```

`mikudo` itself holds no authority at all. If every line of it were wrong, the
worst outcome is that `mikudo` misbehaves.

A command that is not a program in `/bin` — `nvidia`, `mount`, `mkfs.ext4`,
`dmesg`, `sv`, the machine's own commands, which live in the kernel because
each needs ring 0 — is run the same way: the child drops to the target
identity and *then* asks the kernel with `kcmd` (syscall 104), so the kernel's
own per-command gate judges the identity the rule produced, not `mikudod`'s.
In the policy such a command is written `kernel:<name>`, which cannot be
confused with a path.

**Why the password goes over the socket.** The first design had the client hand
its terminal to the daemon with `sendfd`, so root did the reading and the
password never entered an unprivileged address space. That is better, and it
does not fit this machine: a program's fds 0, 1 and 2 are, unless something has
been installed on them, a *routing rule* in the syscall layer that sends them
to the caller's controlling terminal — not descriptors that can be passed — and
only the terminal's foreground job receives keystrokes at all. So the password
takes the same route it takes under sudo and doas. What that costs is a copy of
it in the caller's own address space; what it does not open is a way in for
anybody else, because `/bin/mikudo` is root-owned and 0755 — replacing it
already requires the privilege it would be used to obtain.

---

## 3. Sessions

MikuOS used to have one command interpreter, in ring 0, and that left the
permission model with nothing to protect: a console that is always root is a
console where `mikudo` is a no-op and a file mode decides nothing. Today the
shell people use is `msh`, a ring-3 program with the user's identity on its own
pseudo-terminal, and the in-kernel shell is the **emergency console** — the
prompt of a text boot, reached from a desktop only through `kcmd`. Both are
held to the same identity by **sessions**.

A session is a kernel object rather than a credential on a thread
(`kernel/process/session.rs`). Eight of them at most, each with an identity, a
seat, and the process that leads it. There are two kinds:

* a **console** session has no leader - the in-kernel shell wears the
  identity, and because every VFS operation takes its credential from the
  calling thread's process, every console command becomes honest at once
* a **graphical** session's leader is the account's session program,
  `/bin/maysession`, started as the user; the session ends when it exits

### Nothing that asks for a password holds anything

`login` used to be the privileged half: mikuD started it as root, it read
`/etc/shadow` and computed Argon2id over what somebody typed at a prompt while
holding `CAP_SETUID`. That is the exact shape this system exists to remove - a
process holding authority *while* holding attacker-chosen input - and it was
unextendable besides, because a graphical greeter cannot be a second program
with the same authority and a font stack as well.

So the authority stayed where it already was. `login` and `maygreet` are both
unprivileged clients of **mikudod**, over the same `/run/mikudo.sock`, speaking
`Op::OpenSession`; both run as the `greeter` account, which is the only member
of the `session` group and cannot be logged into. mikudod reads the shadow
file, spends the hash, starts the session program and calls **syscall 92
`session(open, ...)`**. Neither prompt ever sees a hash.

There is deliberately **no operation that checks a password and does nothing
else**. An operation like that is an oracle: something that can be asked "is
this the password" without a login appearing in the journal. Every op that
verifies a password also does something, and says so.

```
login: miku
password for miku:

Welcome, miku.
  The console is yours. `id` says who you are, `mikudo` does what
  needs the machine, and `logout` hands it back.

miku@os:/ $ id
uid=1000(miku) gid=1000(miku) groups=1000(miku),10(wheel),100(users)
session=1
caps=may_priv
miku@os:/ $ who
miku         console    seat0  session 1
miku@os:/ $ cat /etc/shadow
cat: /etc/shadow: permission denied
miku@os:/ $ mikudo id
[mikudo] password for miku:
uid=0(root) gid=0(root) groups=0(root)
caps=all
miku@os:/ $ logout
  logged out - the console is root again. 'login' to start a session
```

A session gets the user's uid, gid and groups, and **exactly one capability:
`CAP_MAY_PRIV`.** That is the "whoever owns the seat owns the display" rule —
without it, starting the desktop fails at the handshake, because `maywm`,
`mayterm` and `aosh` each bind a privileged global. The window manager may
command the compositor and may not read `/etc/shadow`. `libmay::spawn` drops
even that before `exec`ing a program off a keybind or the launcher, so a
terminal you launch cannot take the screen or read the keyboard from under
you.

The leader of a graphical session gets its identity twice, and the two race:
it changes its own (`setgroups`, `setresgid`, `setresuid`) right after the
fork, while `mikudod`'s `session(open)` has the kernel stamp the same identity
onto it. Whichever wins, the other's step may report `EPERM` for a change that
has already happened, so the leader **judges the result** — it reads its ids
and groups back and compares them with what it was meant to become — rather
than failing on each step's return value. Checking the outcome is not weaker
than checking each call; it is the check that unchecked `setuid` returns in
sudo's history say to make.

### The capability follows the seat

A seat is the screen and the keyboard in front of a person. While nobody is
logged in, the **greeter** holds `CAP_MAY_PRIV` for seat 0 - it has to, or it
could not draw over everything and take the keyboard. The instant a session
opens on that seat the kernel takes it away again: permitted, effective *and*
bounding, so nothing the greeter starts can put it back. mikuD restarts the
greeter only while the seat is free.

That is what makes the login screen a lock rather than a picture of one. The
compositor pins the focus to the locking surface and makes Ctrl+Alt+Esc, F12,
Ctrl+Alt+Tab and every window-manager chord inert while it is up
(`kernel/mayland/server/iface/layer.rs`, `kernel/mayland/input/mod.rs`).
Without that, the lock screen would be a screen with a documented key
combination behind it that opens a ring-0 root console.

### Which boot this is

mikuD picks the target at boot: **graphical** when there is a framebuffer, a
`/bin/maygreet` to put on it and a `greeter` account to run it as; **multi-user**
otherwise, where the console asks who you are instead - the same login, through
the same broker, drawn in text. A graphical target whose greeter cannot be kept
running falls back to multi-user rather than leaving a black screen with a
working console behind it that nobody is told about.

`noauth` on the boot line keeps the developer's root console and starts no
login at all. It is deliberately a boot-line flag and not a setting: this is an
open platform, somebody working on the login path has to be able to get in, and
a person who can type on the GRUB line can boot this machine into anything
anyway.

### Logging out

On a text console, `logout` closes the console session and the console is root
again — the console is physical access, and the next person types `login`. In a
terminal window of a graphical session there is no console session to close,
and `logout` there **only drops** that shell's authority to an ordinary
credential with no capabilities. It used to return a ring-0 root shell with
every capability whenever no console session was open — which on a graphical
boot is always — so the word `logout`, typed by uid 1000 in a window, went past
`mikudo`, the password and the journal at once.

### What the console still lets root-era code do

Two honest limits, both enforced rather than hidden:

* **Machine commands are refused to a session.** The kernel console's command
  table decides per command, before any argument is parsed
  (`kernel/shell/commands/mod.rs`, `gated_by`). Powering off or rebooting needs
  `CAP_SYS_BOOT`, which a session holds. Everything that walks a filesystem's
  raw structures (`ext*`, `fs.*`, `fiemap`, the xattr and attribute commands —
  `ext2cat` would print `/etc/shadow` to anybody), partitions or formats
  (`gpt*`, `mkfs.*`, `swap*`, `mkswap*`, `blk*`, `dd`, `install`), mounts
  (`mount` with arguments, `umount`), controls services (`sv` and its verbs),
  touches the hardware (`nvidia`, `display`, `usb`, `memmap`) or reads the
  kernel log (`dmesg`, `pstore`) needs `CAP_SYS_ADMIN`, which a session does
  not carry, and says to use `mikudo`. The read-only verbs — `nvidia status`,
  `sv status`, `mount` with no arguments, `blkstat`, `mayland status` and the
  rest — need nothing, the way `nvidia-smi` and `systemctl status` need
  nothing. Teaching each machine command the credential model is the right
  long-term answer; refusing them fails loudly instead of leaking.
* **The everyday commands are programs.** `ls`, `cat`, `cp`, `rm`, `chmod` and
  the rest used to be builtins of the kernel shell that resolved paths and read
  bytes themselves; they are now ring-3 programs in `/bin`, deleted from the
  kernel, so every one of their accesses goes through the system call layer
  and the VFS checks like any other program's.

---

## 4. Credentials

`kernel/process/cred.rs`. One value per process, behind one lock, so a change
of identity is all-or-nothing — the previous arrangement wrote `uid` and `euid`
as separate relaxed stores and a concurrent permission check could read a pair
that was never a real identity. Threads share their process's credential.

```rust
struct Cred {
    uid, euid, suid,          // POSIX saved-set-id, so a process can lower
    gid, egid, sgid,          // itself for one operation and come back
    groups: [u16; 16], ngroups,
    permitted,                // what may be made effective
    effective,                // what is checked now
    bounding,                 // the ceiling; only ever shrinks, survives exec
    umask,
}
```

Ids are 16 bits; `0xFFFE` is the largest valid one, and an on-disk owner that
does not fit folds to `nobody` (65534).

### Capabilities

**`euid == 0` is not magic.** "root" is the identity that happens to hold every
capability. Each privileged call site asks for the one it needs, so a program
can be given exactly the authority it requires — `maywm` needs to command the
compositor, and under this model it does not also get to read `/etc/shadow`.

| | | | |
|---|---|---|---|
| `setuid` | `setgid` | `kill` | `dac_override` |
| `dac_read` | `fowner` | `chown` | `sys_time` |
| `sys_admin` | `sys_boot` | `net_raw` | `net_bind` |
| `mknod` | `may_priv` | `audit` | `gpu` |

What each one is checked by is listed in
[MikuOS_ABI.md §2.7](MikuOS_ABI.md#27-capabilities). Two are defined and
checked nowhere yet: `mknod` (no system call creates device nodes) and `audit`.
`gpu` guards the one GPU call that takes machine code, `gpu_program` (98): a
loaded program runs on the card and, unless the driver could give the process a
GPU address space of its own, can reach whatever is mapped in the shared one.

`capset` only ever narrows: each requested set must be a subset of what is
already held. Narrowing the *bounding* set confines the whole subtree, because
it survives both `fork` and `exec`.

A service's capabilities are written in its unit (`Capabilities=may_priv`), by
the names above; an unknown name refuses the unit rather than granting nothing
or everything. The greeter is the example: user `greeter`, `may_priv` and
nothing else.

### Transitions

POSIX's rules, deliberately — they are the ones with decades of analysis.

* `setuid(u)` — with `CAP_SETUID`: all three ids move, and **every capability
  is dropped when `u != 0`**. This is the irreversible form.
  Without the capability: effective id only, and only to one already held.
* `setresuid(r, e, s)` — all three at once, so there is no instant at which the
  process is a half-built identity. With `CAP_SETUID`, the capabilities are
  dropped when all three resulting ids are non-zero.
* `setgroups` needs `CAP_SETGID`.
* On `exec`: `permitted &= bounding; effective = permitted`. Nothing else.

---

## 5. `/etc/mikudo.conf`

```
rule    = ("permit" | "deny") option* ident ["as" target]
                                        ["cmd" path ["args" arg...]]
ident   = username | ":" groupname | uid | ":" gid
option  = "nopass" | "nolog" | "persist" | "keepenv"
        | "setenv" "{" (KEY | KEY=value)* "}"
path    = an absolute path | "kernel:" name
```

Three rules, and they are the whole of it:

1. **The last matching rule wins.** Not the most specific — the last. Read
   downwards and remember the most recent line that applies to you.
2. **No match is a denial.** The file grants; it never takes away a permission
   that would otherwise exist, because there is none.
3. **A parse error denies everything** until it is fixed, and says so on the
   console and in the journal, with the line number. A policy whose author's
   intent is unclear grants nothing.

`args` pins **all** the arguments, in order and in number — a bare `args` means
none at all. A prefix is never enough: allowing one would let `cmd /bin/mount
args /mnt/usb` be satisfied by `mount /mnt/usb --bind /`.

The shipped file is one line:

```
permit persist :wheel
```

There is deliberately **no way to write "any command except X"**. Negation
reads clearly and matches wrongly: `cmd !/bin/sh` is defeated by `/bin/mayterm`, by
a copy of a shell under another name, and by any of the dozen programs that
will start one for you. The same goes for the machine's own commands: to take
one back from a blanket rule, write a `deny` *after* it —
`deny :wheel cmd kernel:mkfs.ext4` — and the last match will be the denial.

Load-time checks, all required before a single rule is believed: a regular
file, `st_uid == 0`, not group- or other-writable (it ships 0640), at most
64 KiB, valid UTF-8 with no NUL, at most 256 rules, at most 32 pinned
arguments and 32 `setenv` entries per rule.

---

## 6. Passwords

Argon2id (RFC 9106), `m=19456 KiB, t=3, p=1` by default — OWASP's profile for a
machine that is not a dedicated authentication server — with a 16-byte salt and
a 32-byte tag.

```
$argon2id$v=19$m=19456,t=3,p=1$<salt>$<tag>
!    locked: no password matches, ever
*    the same, spelled the way some other systems spell it
```

PBKDF2 over the HMAC that already existed in `lib/net` would have been ten
lines. It is also, by construction, cheap in parallel — its whole cost is
iterations of a small function over a tiny state, which is the shape a GPU runs
tens of thousands of at once. Argon2 is memory-hard, and the one resource a GPU
does not have thousands of is memory controllers. The `id` variant indexes
data-independently on the first passes (no side channel from the password) and
data-dependently later (a time-memory tradeoff is expensive).

Every stored hash carries the parameters it was made with, so raising the cost
on a machine that can afford it does not invalidate the passwords already set.
The parameters a stored hash may name are bounded (at most 1 GiB of memory, 64
passes, 64 lanes), so a planted line cannot make `mikudod` exhaust the
machine checking it.

A new password must be at least eight characters and may not contain a colon,
a newline or a NUL; changing your own needs the old one, changing somebody
else's is a root operation the policy decides.

The implementation is checked against **RFC 9106 §5.3's published tag**, and
BLAKE2b against RFC 7693's published digests. `cd lib/admin/mikuauth && cargo test`.

---

## 7. Accounts

```
/etc/passwd      0644 root   name:x:uid:gid:description:home:session
/etc/group       0644 root   name:x:gid:member,member
/etc/shadow      0640 root   name:hash:lastchg:min:max:warn:inactive:expire:
/etc/mikudo.conf 0640 root
```

The last `passwd` field is the program a graphical login starts — the
session, not a shell. Every session's `SHELL` is `/bin/msh`.

Shipped:

| Account | uid | Shadow | Groups | What it is |
|---|---|---|---|---|
| `root` | 0 | **locked** | | the identity that holds every capability; reached through `mikudo` |
| `miku` | 1000 | password `miku` | `wheel`, `users` | the person using the machine |
| `alunwrd` | 1001 | locked, unless the image was built with the owner's own `personal/shadow` line | `wheel`, `users` | the machine owner's account and desktop |
| `greeter` | 990 | locked | `session` | the account both login prompts run as |
| `nobody` | 65534 | locked | | the owner of nothing |

| Group | gid | Authority |
|---|---|---|
| `wheel` | 10 | may use `mikudo`: the one line in `/etc/mikudo.conf` |
| `session` | 44 | may ask `mikudod` to open a session — which still costs that person's password |
| `shadow` | 42 | may read `/etc/shadow`; nothing uses it yet |
| `users` | 100 | none |

**Nobody logs in as root.** Its shadow entry is `!`, and the way to become root
is `mikudo`, which records who did it and which line of the policy allowed it.
Unlocking root would give the machine a second way in that leaves no such
record. Nobody logs in as `greeter` either: it is the program that asks who is
logging in, not somebody who does.

> **`miku`'s password is `miku`.** It is written here because a machine you
> cannot log into is not a machine, and a default password that is not
> announced is one somebody else knows first. `mikudod` says so, and journals
> it, on every login with it until it is changed with `passwd`.

`/etc/shadow` is rewritten through a temporary and a `rename`, because `rename`
is the one filesystem operation that is atomic: a crash halfway through a
direct rewrite leaves a truncated `/etc/shadow`, and that is a machine nobody
can log into. On a default boot `/etc` is the read-only copy on the boot
medium; `passwd` and `useradd` refuse there rather than appearing to work and
forgetting at the next boot. A persistent root is `root=<drive>:<lba> rootrw`
on the boot line.

---

## 8. What else changed

A permission model is only worth what the checks around it are worth. These
were all live defects that a single-user machine could not expose:

| | |
|---|---|
| `sys_kill` had **no permission check at all** | Any process could kill any pid — including `mikudod`, the display server, or PID 1. Now: a process whose real or saved uid is your effective uid, or `CAP_KILL` |
| **No directory search permission** | Path resolution walked wherever it was pointed, so a `0700` directory hid nothing: not the names inside it, not the sizes, not the shape of the tree below it. The largest single hole |
| **Supplementary groups never reached the VFS** | `ctx.cred.groups` stayed empty forever, so `in_group()` could only match `egid` and the group permission triple was half dead |
| **No sticky bit** | `/tmp` is mode 1777 and nothing consulted the bit, so any program could remove any other's files there — the classic setup for a symlink swap |
| **`chown` did not clear set-id bits**, and had no "may not give a file away" rule | |
| **32-bit on-disk owners were truncated** | An ext4 volume naming uid 65536 mounted with **root-owned files on it**, and every check waved them through. They fold to `nobody` now |
| **`exec` checked nothing** | Any file a caller could read, they could run. The execute bit decided nothing and `MNT_NOEXEC` was never consulted |
| **`MNT_NOEXEC` / `MNT_NODEV` were declared and read by nothing** | |
| **The socket table was 64 slots, first come first served** | One user's program could exhaust it and stop the display server accepting anybody. It is 256 now, with at most 160 for one account and four display connections per process |
| **Syscalls 47–55 had no userspace binding** | They were implemented in the kernel and unreachable from ring 3 — which is *why* every process on MikuOS was root |
| **`exec` handed the console to every program it started** | It called `set_foreground` unconditionally, so the moment mikuD started its first ELF service the daemon owned the keyboard and the shell stopped receiving anything. Keys still echoed, so the console looked alive and ran nothing. Ownership of the terminal is the caller's business now |
| **`mke2fs -d` gave every shipped file the build host's uid** | On a developer's machine that is uid 1000 — which is `miku`. So `/etc/shadow` shipped owned by the unprivileged user (`cat` succeeded, with the mode and the check both working perfectly — she was the owner), and `mikudo` refused to run because `/etc/mikudo.conf` was not root's. Invisible on the host. The image is now made by installing packages, and modes and owners come from the packages (`builder/src/pkg.rs`) |
| **`/dev/blkN` and `/dev/net0` were 0666** | Any process could read `/etc/shadow` straight off the raw disk, or put a forged frame on the wire. Both are root's and 0600, and `/dev/net0` also needs `CAP_NET_RAW` |
| **`logout` in a desktop terminal returned a root shell** | See [Logging out](#logging-out) |
| **A terminal named by id could be reached by anyone** | A terminal id carries a generation, so one kept across a hangup names nothing; only the master's holder, its descendants or `CAP_SYS_ADMIN` may attach to one or open its slave; there is no input-injection request (`TIOCSTI`) at all |
| **A service unit's `User=` and `Capabilities=` were read and ignored** | Every unit loaded from disk ran as root with every capability whatever it said. They are applied now, and a `User=` naming no account refuses the unit instead of running it as root |

---

## 9. Availability

`mikudod` serves one connection at a time — the machine's only route to root
is a single queue, by design, so that no two requests can interleave inside the
process that changes identities. So every wait in it has a deadline:

| wait | limit | why |
|---|---|---|
| reading a request | 5 s | a client that connects and sends nothing, or sends three bytes of a four-byte length prefix and stops, would otherwise hold the queue open against everybody |
| reading a password | 60 s | the same, with a person at the other end who has to find the keyboard, read the prompt, and get it right - and on the greeter, click the field first |
| password attempts | 3 | then the connection is dropped and the attempt journalled |

Both waits are `poll` with the remaining time, not `recv` — `recv` blocks with
no way to say for how long, so a timeout checked only *between* reads is
decoration.

`mikudod` is not critical to booting: a machine with no way to become root is a
usable machine with a loud message in the journal, and refusing to finish
booting would leave nobody able to fix it. mikuD restarts it if it exits.

The socket table also has a per-account ceiling (160 of 256 slots), so one
account cannot exhaust it and stop the display server accepting anybody.

---

## 10. Verifying it

```bash
cd lib/admin/mikuauth && cargo test      # about a hundred cases: RFC vectors, hostile corpora
```

* `tests/argon2.rs` — RFC 9106 §5.3's published tag, RFC 7693's digests, and
  what happens when a stored hash names a cost that would exhaust the machine.
* `tests/db.rs` — the passwd/group/shadow parsers against embedded NULs,
  100 000-character lines, colons in gecos, uid 65536, uid −1, CRLF, duplicate
  entries.
* `tests/conf.rs` — every example in `doas.conf(5)`, plus unterminated
  `setenv {`, 10 000 rules, relative command paths, argument-prefix matching.
* `tests/proto.rs` — truncated messages, length fields describing more than
  arrived, non-UTF-8 where a path is expected.
* `tests/session.rs` — the session protocol between the prompts and the broker.
* `tests/shipped.rs` — the files that actually ship, parsed by the code that
  will read them at boot.

In the kernel, headless — put the suite on the boot line of a scratch ISO
([wiki/testing.md](wiki/testing.md#the-developers-qemu-harness)):

```
run=cred_test        # or `cred test` in the kernel console
```

Note there is **no serial input path** on MikuOS — nothing reads the UART's RX
register — so a scripted session is driven by `sendkey` and read back on
`-serial stdio`. Type slowly: faster than about 0.1 s per key and the emulated
PS/2 controller drops some.

Identity, the capability set, the file-permission rules, and the invariant —
asserted directly, including against a mode-4755 file.

The decisions themselves are in the kernel log: every line `mikudod` writes
starts `[mikudo]` — who asked, as whom, for what, and which rule decided —
including every denial and every failed authentication, whatever `nolog` says.
`mikudo dmesg -g mikudo` reads them. The log is in memory: it survives a warm
reboot only when the boot line has `klogpstore`.

---

## 11. Using it

```
id                       your uid, groups, session and capabilities
mikudo <command>         run it as root, after proving who you are
mikudo -u ada <command>  as somebody else
mikudo -l                what you are allowed to do
mikudo -c <command>      would this be allowed? run nothing
mikudo -n <command>      never prompt; fail if a password would be needed
mikudo -s                a shell as the target user
mikudo -L                forget the remembered authentication
mikudo nvidia log err    one of the kernel's own commands (kernel:nvidia)
passwd                   change your password
su [-] [user]            a thin front for mikudo, kept for muscle memory
mikudo useradd ada       add an account (it ships locked; passwd it next)
logout                   at the text console, end the session
```

The service, login and package machinery that this model protects is described
in [wiki/system.md](wiki/system.md).
