# The system: services, users, login, privilege and packages

How a MikuOS machine is put together above the kernel's mechanisms: mikuD,
the init and service supervisor; the accounts and their files; how a person
logs in at the console or on the graphical greeter; how privilege is brokered
by `mikudod` and `/etc/mikudo.conf` instead of setuid binaries; the package
manager `mikupkg`; the installer; the files under `/etc`; and the path from
power-on to a desktop. The privilege model itself, with its reasoning, is in
[MikuOS_Security.md](../MikuOS_Security.md); this page summarises it and links
there.

## Contents

- [mikuD](#mikud)
  - [Targets](#targets)
  - [Unit files](#unit-files)
  - [The built-in services](#the-built-in-services)
  - [Lifecycle, restarts and stopping](#lifecycle-restarts-and-stopping)
  - [Service commands](#service-commands)
  - [The journal](#the-journal)
- [Users and groups](#users-and-groups)
- [Authentication](#authentication)
  - [mikuauth](#mikuauth)
  - [Logging in at the console](#logging-in-at-the-console)
  - [Logging in on the greeter](#logging-in-on-the-greeter)
  - [Sessions and the seat](#sessions-and-the-seat)
  - [The noauth escape hatch](#the-noauth-escape-hatch)
- [Privilege](#privilege)
  - [mikudo and mikudod](#mikudo-and-mikudod)
  - [/etc/mikudo.conf](#etcmikudoconf)
  - [The decision journal](#the-decision-journal)
  - [Capabilities](#capabilities)
- [mikupkg](#mikupkg)
- [The installer](#the-installer)
- [/etc reference](#etc-reference)
- [Time and the hostname](#time-and-the-hostname)
- [Boot to desktop, from mikuD's view](#boot-to-desktop-from-mikuds-view)
- [Troubleshooting](#troubleshooting)
- [Where the code is](#where-the-code-is)

---

## mikuD

mikuD (`kernel/mikud/`) is the init and service supervisor. It runs in the
kernel, as the kernel thread `mikud`, and is **pid 1**: orphaned processes are
reparented to it and it reaps them. Services are entries in a fixed table
(`MAX_SERVICES` = 32) and are either kernel threads (an `entry` function) or
ring-3 programs (`exec_start_path`), started with the identity and
capabilities their entry names.

### Targets

| Target | Unit-file spellings | Meaning |
|---|---|---|
| `sysinit` | `sysinit`, `sys-init` | early system setup |
| `multi-user` | `multi-user`, `multiuser`, `multi` | the text system: keyboard, console, network, disks |
| `graphical` | `graphical`, `gui` | `multi-user` plus the compositor and the greeter |
| `rescue` | `rescue`, `single` | rescue |

The kernel picks the default target at boot (`kernel/main.rs`): **graphical**
when there is a framebuffer, `/bin/maygreet` exists and `noauth` is not on the
boot line; **multi-user** otherwise. `sv isolate <target>` switches at run time.
A service only the graphical target wants that cannot be kept running (its
restarts exhausted) makes mikuD fall back to multi-user, so a broken greeter
gives a working text console rather than a black screen.

### Unit files

Services can also be defined by `.service` files in **`/etc/mikud/`**, read
by `sv scan` (every `*.service` there) or `sv load <path>`. None ship: every
service of the stock system is registered in code. The format is INI-like;
lines starting with `#` or `;` are comments, an unknown section is an error,
an unknown key is ignored, and a key's value is everything after the first
`=`, trimmed. The unit's name is the file name without `.service`.

```ini
[Unit]
Description=example daemon
After=netd
Wants=usbd
Requires=mikudod
Conflicts=rescue-shell
ConditionPathExists=/etc/example.conf
ConditionPathExists=!/etc/example.disabled
ConditionServiceActive=netd

[Service]
Type=simple
ExecStart=/bin/exampled
Restart=on-failure
RestartSec=1
Priority=5
WatchdogSec=0
TimeoutStartSec=10
TimeoutStopSec=1
RemainAfterExit=false
Critical=false
User=nobody
Capabilities=net_bind
Environment=EXAMPLE_LEVEL=2

[Install]
WantedBy=multi-user
```

| Section | Key | Value | Default |
|---|---|---|---|
| `[Unit]` | `Description` | text | empty |
| | `After` | space-separated service names: hard dependencies, started first | none |
| | `Requires` | the same; merged with `After` | none |
| | `Wants` | soft dependencies, started if possible | none |
| | `Conflicts` | services that may not run at the same time | none |
| | `ConditionPathExists` | a path that must exist; `!path` must not; repeatable | none |
| | `ConditionServiceActive` | a service that must be running; repeatable | none |
| `[Service]` | `Type` | `simple`, `oneshot`, `forking`, `notify` (else the unit is refused) | `simple` |
| | `ExecStart` | the program's absolute path (no arguments) | none: a unit needs a name or this |
| | `Restart` | `always`, `never`/`no`, `on-failure`, `on-success`, `on-abnormal` | `never` |
| | `RestartSec` | delay before a restart | 200 ms |
| | `Priority` | scheduling priority, 0-255 | 5 |
| | `WatchdogSec` | time without a watchdog ping before the service counts as failed; 0 off | 0 |
| | `TimeoutStartSec` | time a `Type=notify` service has to report ready | 10 s |
| | `TimeoutStopSec` | time between `SIGTERM` and the kill (capped at 1 s) | 10 s |
| | `RemainAfterExit` | `true`/`yes`/`1`/`on`: a oneshot stays "active (exited)" | false |
| | `Critical` | the same booleans: may not be stopped except at shutdown | false |
| | `User` | an account name from `/etc/passwd`; a name that does not exist refuses the unit (it is never run as root instead) | root |
| | `Capabilities` | capability names as `id` prints them, separated by spaces or commas; an unknown name refuses the unit; empty means none at all | the user's default |
| | `Environment` | `KEY=VALUE`, repeatable (at most 8) | none |
| `[Install]` | `WantedBy` | a target name | `multi-user` |

**The `*Sec` keys are times**, as in systemd: a bare number is seconds, and
`ms`, `s` or `min` may follow it (`RestartSec=200ms`, `TimeoutStartSec=1min`).
They are converted to timer ticks when the unit is loaded; a non-zero time
shorter than a tick becomes one tick. (Until 2026-11-03 the number was stored
as raw 4 ms ticks, so `WatchdogSec=30` meant 120 ms.) At most 8 dependencies,
4 conditions and 8 environment variables are kept per service.

### The built-in services

Registered in `kernel/main.rs` before mikuD starts:

| Service | Kind | Target | Restart | Priority | Critical | Depends on | What it does |
|---|---|---|---|---|---|---|---|
| `kbd` | kernel thread | multi-user | always | 2 | yes | | keyboard input (PS/2), feeding the input ring |
| `mikudod` | `/bin/mikudod`, root | multi-user | always | 4 | no | | the privilege and login broker |
| `shell` | kernel thread | multi-user | always | 3 | yes | `kbd` | the in-kernel console |
| `netd` | kernel thread | multi-user | on-failure | 1 | no | | DHCP after the NIC comes up, in the background ([network.md](network.md)) |
| `usbd` | kernel thread | multi-user | on-failure | 2 | no | | the xHCI host and USB HID input; not registered with `nousb` on the boot line |
| `bdflush` | kernel thread | multi-user | always | 1 | no | | block-cache write-back ([storage.md](storage.md)) |
| `kswapd` | kernel thread | multi-user | always | 1 | no | | page reclaim and swap ([memory.md](memory.md)) |
| `compositor` | kernel thread | graphical | always | 3 | no | | Mayland's compositor ([mayland.md](mayland.md)) |
| `greeter` | `/bin/maygreet`, user `greeter`, capabilities `may_priv` only | graphical | always | 5 | no | `mikudod`, `compositor` | the graphical login; condition: seat 0 free |

`greeter` is registered only if `/etc/passwd` has a `greeter` account; without
one the machine has no graphical login rather than a login screen running as
root. Its `SeatFree` condition is what stops `Restart=always` bringing it back
over the desktop the moment somebody logs in, while still bringing it back
when they log out.

### Lifecycle, restarts and stopping

- **Starting**: dependencies first; conditions checked; a program service is
  started through the kernel's spawn (`exec_as`) with its credential applied
  before it runs, never taking the console's keyboard.
- **States**: stopped, starting, activating (a oneshot running), running,
  reloading, stopping, failed.
- **Restart**: per the policy, after `RestartSec`. **Burst protection**: a
  service that fails 5 times within 2500 ticks (10 s) is marked failed and not
  restarted again. Only failures count: a clean exit restarted by policy (the
  greeter after every login) is not a burst.
- **Stopping**: dependents first, then `SIGTERM`, then a kill if the service
  has not exited within `min(TimeoutStopSec, 250)` ticks. A critical service
  refuses to stop except at shutdown (`force-stop` overrides).
- **Shutdown** (`power`, `reboot`, `poweroff`): every service in reverse
  priority order, non-critical first, then critical; then the machine is reset
  or powered off.
- **Watchdog and notify**, as sd_notify's `WATCHDOG=1` and `READY=1`. A
  service reports on itself with `mikuctl notify --ready` / `--watchdog`, or
  by sending the line `sv notify READY=1` / `sv notify WATCHDOG=1` through
  `miku_kcmd` (104) itself. No privilege is needed: the kernel works out which
  service is speaking from the caller (`mikud::notify_from`). It is the service
  whose main process is the caller's process, or an ancestor of it below mikuD,
  up to 32 levels, so a script's helper counts too. A process that belongs to
  no service is told so and changes nothing.
  - A `Type=notify` unit stays `starting` until it reports ready, and is killed
    when `TimeoutStartSec` passes first.
  - With `WatchdogSec` above 0, a running service that has not pinged for that
    long is killed and restarted by its policy; start counts as the first ping.
  - The `sv notify` line always returns success to `kcmd`: a refusal is a
    message on the caller's terminal, not an error code.

### Service commands

The kernel console's `sv` (privileged verbs need `CAP_SYS_ADMIN`, so `mikudo
sv ...` from a desktop), and the ring-3 front end `mikuctl`:

| Command | What it does | Needs |
|---|---|---|
| `sv`, `sv list`, `sv ls` | every service: state, pid, target | nothing |
| `sv status [name]` | one service in detail, or all | nothing |
| `sv tree <name>`, `sv rdeps <name>` | dependencies, reverse dependencies | nothing |
| `sv analyze` | boot timing per service | nothing |
| `sv cat <name>` | the service as a unit file | nothing |
| `sv start\|stop\|restart\|reload <name>` | lifecycle | **P** |
| `sv enable\|disable <name>` | whether it starts with its target | **P** |
| `sv mask\|unmask <name>` | forbid or allow starting it at all | **P** |
| `sv force-stop <name>` (`kill`) | stop even a critical service | **P** |
| `sv journal [name]` (`log`) | the event log | **P** |
| `sv target [name]`, `sv isolate <target>` | show or switch the target | **P** |
| `sv load <path.service>`, `sv scan` | load unit files | **P** |
| `sv timer <list\|start\|stop\|remove> [name]`, `sv socket <list\|stop\|remove> [name]` | timer and socket units | **P** |
| `mikuctl start\|stop\|restart\|reload\|enable\|disable <service>` | the same, from `/bin`, vocabulary checked in ring 3, forwarded through `kcmd` | **P** (`mikudo mikuctl restart netd`) |
| `mikuctl status [service]`, `mikuctl list` | read-only | nothing |
| `sv notify [READY=1] [WATCHDOG=1]`, `mikuctl notify --ready\|--watchdog` | from inside a service: it has started, or it is still alive ([above](#lifecycle-restarts-and-stopping)) | nothing; acts only on the caller's own service |

The timer and socket tables (`kernel/mikud/timer.rs`, `socket.rs`, at most 16
each) are always empty: nothing calls `register_timer` or `register_socket`,
a unit file has no `[Timer]` or `[Socket]` section, and the network stack never
calls `activate_socket`. `sv timer` and `sv socket` can only list nothing.

### The journal

mikuD keeps a ring of the last 128 lifecycle events (start, stop, exit codes,
restarts, failures), shown by `sv journal [name]`. Service output that is not
for a terminal goes to the kernel log through `klog` (75); read it with
`mikudo dmesg` ([diagnostics.md](diagnostics.md)).

---

## Users and groups

Three files, parsed by `lib/mikuauth/src/db.rs` (lines up to 4096 bytes,
fields up to 256, ids up to `0xFFFE`):

| File | Mode | Format |
|---|---|---|
| `/etc/passwd` | 0644 | `name:x:uid:gid:description:home:session` |
| `/etc/group` | 0644 | `name:x:gid:member,member,...` |
| `/etc/shadow` | 0640 | `name:hash:last_change:min:max:warn:inactive:expire:reserved` |

`#` lines are comments. The second `passwd` field is always `x`; the hash is
in `/etc/shadow`, which only `mikudod` reads or writes (through a temporary file
and a `rename`). **The last `passwd` field is the session program**, not a
shell: what a graphical login starts (`/bin/maysession` for people,
`/bin/false` for accounts nobody logs in as); the default when it is empty is
`/bin/maysession`, and a session's `SHELL` is always `/bin/msh`.

**The shipped accounts** (`etc/passwd`, `etc/group`, `etc/shadow`):

| Account | uid:gid | Home | Session | Shadow | Groups |
|---|---|---|---|---|---|
| `root` | 0:0 | `/root` | `/bin/mayterm` | `!` (locked: nobody logs in as root) | |
| `miku` | 1000:1000 | `/home/miku` | `/bin/maysession` | Argon2id of **`miku`** | `wheel`, `users` |
| `alunwrd` | 1001:1001 | `/home/alunwrd` | `/bin/maysession` | `!` unless the image is built with `personal/shadow` | `wheel`, `users` |
| `greeter` | 990:990 | `/` | `/bin/false` | `!` | `session` |
| `nobody` | 65534:65534 | `/` | `/bin/false` | `!` | |

| Group | gid | Meaning |
|---|---|---|
| `root` | 0 | |
| `wheel` | 10 | may use `mikudo` (the one rule in `/etc/mikudo.conf`) |
| `shadow` | 42 | may read `/etc/shadow`; unused (mikudod reads it as root) |
| `session` | 44 | may ask `mikudod` to open a session: the login prompts, running as `greeter` |
| `users` | 100 | |
| `miku`, `alunwrd`, `greeter`, `nobody` | 1000, 1001, 990, 65534 | personal groups |

**Logging in as `miku` with password `miku` works on a fresh image**, and the
login says so on every login until `passwd` changes it (`mikuauth`'s
`SHIPPED_PASSWORD`).

**Homes**: when a graphical session starts, `mikudod` creates the home if it
does not exist (parents 0755 as root, the home 0700 as the user) and copies
`/etc/skel` into it, files 0644 and directories 0755, owned by the user. An
existing home is left alone. An image built with `personal/` puts its owner's
desktop files in `/etc/skel`.

**Adding an account**: `mikudo useradd [-u uid] [-g gid] [-G group,...] [-s
session] [-d home] name`, then `mikudo passwd name` (or the user runs
`passwd`). The account starts locked. `useradd -D name` deletes. On a default
boot `/etc` is the read-only boot copy: changes need a writable root
(`root=<drive>:<lba> rootrw`), see [the installer](#the-installer).

---

## Authentication

### mikuauth

`lib/mikuauth` is the one implementation of "who is this": the hash, the three
account files, the `/etc/mikudo.conf` grammar, and the socket protocol between
the clients and `mikudod`. It runs in ring 3 (`mikudod`, `login`, `passwd`,
`maygreet`) and on the build host.

- **Hashes**: Argon2id (RFC 9106), stored in the standard string
  `$argon2id$v=19$m=19456,t=3,p=1$<salt-b64>$<tag-b64>`; new passwords use
  `m=19456` KiB, `t=3`, `p=1`, a 16-byte salt and a 32-byte tag. Every stored
  hash carries its parameters, so the cost can be raised without invalidating
  existing ones; a stored hash naming more than 1 GiB, 64 passes or 64 lanes is
  refused. Comparison is constant-time. Two values are not hashes: `!` and `*`
  are locked (no password matches); an empty field means no password, which
  nothing ships and `useradd` never writes, and which only the console accepts.
- **Password rules** (`mikudod`): at least 8 characters, within the protocol's
  maximum, and no colon, newline or NUL.
- **Tests**: `cd lib/mikuauth && cargo test`, about a hundred cases: RFC 9106
  §5.3's published Argon2id tag, RFC 7693's BLAKE2b digests, hostile
  `passwd`/`group`/`shadow` corpora, every `doas.conf(5)` example, protocol
  truncation, and the files that actually ship (`tests/shipped.rs`).

### Logging in at the console

On a multi-user boot (or when the graphical target fell back), the kernel
console asks who is there:

1. The console's `login` command (run automatically at boot, or typed) starts
   `/bin/login` as the **`greeter`** account with no capabilities, and waits.
2. `login` prompts for a name and, with the echo off, a password, and sends
   both to `mikudod` over `/run/mikudo.sock` (`Op::OpenSession`, kind console,
   seat 0).
3. `mikudod` checks the caller is in the `session` group, reads
   `/etc/passwd`, `/etc/group` and `/etc/shadow`, verifies the password (3
   attempts, each journalled on failure), and calls `session(open, ...)` (92).
4. The kernel records the session; a console session has no leader process:
   the console thread wears the identity, so every VFS check from the console
   is now made as the user. The prompt becomes the user's.
5. `logout` closes the console session and the console is root again (a
   text console is physical access to the machine), ready for the next
   `login`. Typed in a terminal window of a graphical session, where there is
   no console session to close, `logout` instead drops that shell's
   capabilities and never raises them.

### Logging in on the greeter

1. mikuD starts `greeter` (`/bin/maygreet`, user `greeter`, capability
   `may_priv` only) once `mikudod` and the compositor run and seat 0 is free.
2. `maygreet` draws one surface over everything and holds the keyboard; the
   compositor pins focus to it and makes window-manager chords inert while it
   is up.
3. It asks `mikudod` exactly what `login` asks (`Op::OpenSession`, kind
   graphical).
4. `mikudod` authenticates, makes the home if needed, and **starts the session
   leader**: it forks a middle process, which forks the leader and exits, so
   the leader is an orphan reparented to mikuD. The leader drops to the user
   (`setgroups`, `setresgid`, `setresuid`), then execs the `passwd` session
   program (`/bin/maysession`) with an environment built from nothing:
   `PATH=/sbin:/bin`, `HOME`, `USER`, `LOGNAME`, `SHELL=/bin/msh`.
5. `mikudod` calls `session(open)` with that leader. The kernel stamps the
   session's credential onto the leader and moves `CAP_MAY_PRIV` from the
   greeter to it.
6. `maysession` sets `XDG_CURRENT_DESKTOP=Mayland`, reads
   `/etc/mayland/session.d/*.conf` (the stock one starts `maywm`, `aosh` and
   `mayterm`, with `main = /bin/maywm`), starts each with `may_priv`, and then
   only reaps its children. **The session lasts as long as `maysession`
   runs**, and `maysession` ends when the `main` program exits (or, without a
   `main`, the first program started; or when no child is left). Then the
   kernel closes the session, the seat is free again, and mikuD's
   `Restart=always` brings the greeter back.

**The session stamp race.** Steps 4 and 5 race: the leader changes its own
identity right after `fork`, and the kernel's `session(open)` stamps the same
identity onto it. If the kernel wins, the leader is already the user without
`CAP_SETGID`, and its own `setgroups` fails with `EPERM` for a change that has
already happened. So the leader attempts the steps in order and then **judges
the result** (reads back its uid, euid, gid and groups) instead of failing on
each step's return. The race became reproducible on 2026-09-16, when serial
tracing stopped slowing every program start by about 175 ms.

### Sessions and the seat

A session is a kernel object (`kernel/process/session.rs`, at most 8): an
identity, a kind (console or graphical), a seat, a leader and when it was
opened. `who` lists them; `session(current)` walks a process's parents to the
session leader, which is how `mikudod` attributes a request to a login. The
details are in [ABI §4.10](../MikuOS_ABI.md#92-session) and
[MikuOS_Security.md §3](../MikuOS_Security.md#3-sessions).

### The noauth escape hatch

`noauth` on the GRUB command line skips every login: the default target is
multi-user, the console stays root (`[shell] 'noauth' - the console stays
root` on serial), and no greeter starts. It is the developer's way in when
the login path itself is broken; anyone who can edit the boot line could boot
the machine into anything anyway. See [boot.md](boot.md).

---

## Privilege

The rule: **privilege is granted by a privileged process to a child, never
acquired from a file.** There is no setuid bit, no setgid bit and no file
capability; `exec` only ever keeps or narrows the capability sets. The full
argument, the advisories it rules out and the checks are in
[MikuOS_Security.md](../MikuOS_Security.md).

### mikudo and mikudod

`/bin/mikudo` is an ordinary program with no authority. It connects to
`/run/mikudo.sock` and sends a request (target user, command, arguments,
working directory, flags). `/bin/mikudod`, started by mikuD as root with every
capability and an environment mikuD built, is the one process on the machine
that turns one identity into another. For each connection it:

1. asks the kernel who connected (`peercred`, recorded at connect time, so pid
   reuse cannot fool it; the request carries no uid to lie in);
2. reads the request (5 s limit) into fixed storage;
3. reads `/etc/passwd`, `/etc/group` and `/etc/mikudo.conf`, checking the
   policy file is a regular root-owned file not group- or other-writable;
4. decides: the last matching rule wins, no match denies, a parse error denies
   everything;
5. asks the client for the password and checks it against `/etc/shadow`
   (60 s, 3 attempts), unless the rule says `nopass` or a `persist` token for
   this caller and session is still valid (5 minutes, in memory, on the
   monotonic clock);
6. forks; the child drops to the target (`setgroups`, `setresgid`,
   `setresuid`, then verifies), changes directory, and execs the resolved path
   with an argv and environment built here (`PATH=/sbin:/bin`, `HOME`, `USER`,
   `LOGNAME`, `SHELL=/bin/msh`, plus what `keepenv`/`setenv` allow);
7. journals the decision and which line made it, and reports the exit status.

A command that is not a file in `/bin` is run as one of the kernel console's
commands through `kcmd`, as the target identity, so `mikudo mount ...` and
`mikudo dmesg` work; in the policy such a command is named `kernel:<name>`.
`mikudod` also serves `passwd` (your own password needs the old one; somebody
else's is a root operation and asks the policy), login sessions, closing a
session, the session list and power requests. It serves one connection at a
time, which is why every wait in it has a deadline.

`mikudo`'s options: `-u user` (default root), `-n` never prompt, `-s` the
target's shell, `-l` list what you may do, `-c` check without running, `-L`
forget this terminal's persist token, `--` end of options. `su [-] [user] [-c
command]` is a front end that execs `mikudo`.

### /etc/mikudo.conf

The grammar is doas.conf's (`lib/mikuauth/src/conf.rs`):

```text
rule    = ("permit" | "deny") option* ident ["as" target] ["cmd" path ["args" arg...]]
ident   = username | ":" groupname | uid | ":" gid
option  = "nopass" | "nolog" | "persist" | "keepenv"
        | "setenv" "{" (KEY | KEY "=" value)* "}"
path    = an absolute path | "kernel:" name
```

| Part | Meaning |
|---|---|
| `permit` / `deny` | grant or refuse what the rest of the line matches |
| ident | the caller: a user name, a uid, `:group`, or `:gid` |
| `as target` | only when becoming this user (default: any target) |
| `cmd path` | only this command, as an absolute path or `kernel:<name>` (default: any command) |
| `args a b c` | only with exactly these arguments, in order, and the same count; a bare `args` means no arguments. A prefix is never enough |
| `nopass` | no password |
| `nolog` | no journal line for a permitted run (denials are always journalled) |
| `persist` | remember a successful authentication for 5 minutes, for this caller on this terminal |
| `keepenv` | keep the caller's environment (within validation) |
| `setenv { KEY KEY=value }` | pass `KEY` from the caller, or set `KEY=value` |

The three rules: **the last matching rule wins**; **no match is a denial**;
**a syntax error anywhere denies everything** until fixed, and says which
line. There is no negation ("any command except X"). Limits: 64 KiB, 256 rules,
32 pinned arguments and 32 `setenv` entries per rule. The file must be a
regular file owned by root and not group- or world-writable (it ships 0640).

The shipped policy is one line:

```text
permit persist :wheel
```

More, as the file's comments suggest:

```text
permit nopass :wheel cmd /sbin/fstrim args /             # one command, exact arguments, no password
permit nopass deploy as root cmd /bin/mikuctl args restart netd
permit :wheel as root cmd /bin/mikupkg-apply             # installing software
permit nopass :wheel cmd kernel:nvidia args list         # a kernel command, read-only
deny :wheel cmd kernel:mkfs.ext4                          # later lines win: take one back
permit :wheel setenv { EDITOR VISUAL } cmd /bin/visudo
```

`mikudo -l` prints what the caller may run; `mikudo -c cmd` says whether a
command would be allowed.

### The decision journal

Every decision `mikudod` makes is written to the kernel log as a line starting
`[mikudo]`: who asked, as whom, what, and which rule decided; failed
authentications; logins (`miku logged in (graphical session 2, seat 0)`) and
logouts; a login with the shipped password; refused sessions. Denials are
always journalled, whatever `nolog` says. Read it with `mikudo dmesg -g
mikudo`. The journal lives in the kernel log's rings, so it is in memory and
lost at reboot (keep it across a warm reboot with `klogpstore`).

### Capabilities

Sixteen capabilities, listed with what checks each in
[ABI §2.7](../MikuOS_ABI.md#27-capabilities). Who holds what by default:

| Process | Capabilities |
|---|---|
| root (mikuD's services that name no user, `mikudod`, programs run by `mikudo` as root) | all |
| a logged-in session's leader and the programs of the session | `may_priv` only (it follows the seat) |
| a program started from the launcher or a keybind (`libmay::spawn`) | none (`may_priv` is dropped) |
| `greeter` | `may_priv`, until a session opens on the seat |
| any other user process | none |
| a service with `Capabilities=` | exactly that list |

`CAP_GPU` (loading GPU machine code, syscall 98) is not part of any default
set; grant it with a unit's `Capabilities=gpu` or run the program through
`mikudo`.

---

## mikupkg

The package manager (`lib/mikupkg`, `/bin/mikupkg`, `/bin/mikupkg-apply`);
the design is [dev/package-architecture.md](../../dev/package-architecture.md).

**What a package is.** A `.mpk` container (magic `MIKUPKG\0`) laid out so the
header, the metadata, the file table and the signature all come **before**
the payload: a reader verifies the signature and the whole table within a few
kilobytes and hashes the payload as it writes it. One signature (ECDSA P-256
over SHA-256) covers `SHA-256(metadata || table)`, and the table holds a
SHA-256 per file, so a changed payload byte is caught by its file hash and a
changed table by the signature; both checks live in one function
(`trust::package`). Metadata is a typed record list in which the high bit of a
tag means *critical*: an unknown critical tag refuses the package, an unknown
other one is reported and skipped. A package has dependencies, provides,
conflicts and **triggers from a closed vocabulary** compiled into
`mikupkg-apply`: no install scripts.

**The image is built by installing packages.** `cd builder && cargo run`
builds the programs, packs them into `.mpk` files in `target/pkgs/`, and
installs them into the boot image through the same applier the machine runs,
so the image's modes and owners come from the packages and it ships
`/pkg/db`, a database that describes it. The groups:

| Package | Contents | Depends on |
|---|---|---|
| `miku-base` | the commands in `utils/`, `msh`, `mikupkg`, `/etc/os-release`, `/etc/mikupkg` | |
| `miku-system` | `mikudo`, `mikudod`, `login`, `passwd`, `id`, `su`, `useradd`, the account files and `/etc/mikudo.conf` | `miku-base` |
| `miku-desktop` | `maywm`, `mayterm`, `maygreet`, `maysession`, `aosh`, `/etc/mayland`, `/etc/aosh` | `miku-base`, `miku-system` |
| `miku-net` | the network tools and `/etc/ssl` | `miku-base` |
| `miku-demos` | display clients, ABI smoke tests, the `std*` programs | `miku-base` |
| `zed` | the Zed editor | `miku-desktop` |
| `miku-firmware` | GPU firmware | |
| `miku-personal` | the owner's desktop, from `personal/` (only when it exists) | `miku-desktop` |

**Commands** (the full list in [commands.md](commands.md#packages)):

| Verb | Needs |
|---|---|
| `list [-e\|-d]` (explicit or dependency), `info <pkg>`, `files <pkg>`, `owner <path>`, `why <pkg>`, `verify [pkg]`, `search <text>` | nothing; work on any boot, from `/pkg/db` |
| `add <file.mpk>...`, `remove <pkg>...`, `tx recover`, `clean` | a writable root, and `mikudo` for `mikupkg-apply` |
| `--dry-run` (`-n`), `--allow-unsigned` | global flags |

**The privilege split.** `/bin/mikupkg` runs as whoever typed the command and
does all the parsing: the container, the index, the configuration, the version
solving. It stages the bytes and a plan, then asks `mikudo` to run
`/bin/mikupkg-apply` as root, which **believes none of it**: it verifies each
package's signature against the keyring it reads itself and hashes every staged
file against the signed table before placing anything. A transaction is
journalled (`/pkg/journal`) so an interrupted one can be put back (`tx
recover`).

**Configuration**: three layers, later wins, like `/etc/mayland`:
`/etc/mikupkg/default.d/*.conf` (shipped), `/etc/mikupkg/mikupkg.conf` (this
machine), `/etc/mikupkg/conf.d/*.conf` (yours). Keys: `db` (`/pkg/db`),
`cache` (`/pkg/cache`), `journal` (`/pkg/journal`), `keyring`
(`/etc/mikupkg/keys`), `max_index_age` (7d), `allow_downgrade` (false), and
`repo NAME { url, key, priority, mirror }` blocks. An unknown line is reported;
an unknown line inside a `repo` block stops the run, because a repo block says
whose signatures the machine believes.

**Not built yet**: fetching from a repository and publishing to one, so `sync`,
`install` and `upgrade` do not exist; no repository is configured. Packages
cannot ship shared libraries (the ten in `/lib` are inside the kernel image),
paths are limited to 255 bytes and 32 levels, and there is no content-addressed
store.

---

## The installer

`install` is a kernel console command (**P**), so from a desktop it is
`mikudo install ...` (`kernel/shell/commands/install_cmds.rs`); `install gui`
opens the same steps as a window (`kernel/setup/`, which also asks for a
language and a time zone).

```
install plan <drive>          what it would do, writing nothing; prints the exact 'go' line
install go <drive> <size>     do it; the size must be the one 'plan' printed
install gui                   the same, as a window
```

What an installed disk looks like:

| Where | What |
|---|---|
| LBA 0 | GRUB's boot code and the protective MBR |
| LBA 2048, 2 MiB | GPT partition 1, BIOS boot: `core.img`, written raw |
| LBA 6144 to the end | partition 2, **ext4** labelled `mikuos`: `/boot`, `/bin`, `/etc`, `/lib/firmware` and the package database |

`install go` partitions with GPT, formats partition 2, copies the boot
medium's files and the bootloader, and writes a boot line with
`root=LABEL=mikuos`.

**Disk safety.** Nothing on MikuOS picks a disk or writes to one implicitly:

- `install` never chooses a disk. `plan` writes nothing; `go` needs the drive
  **and** its size in MB, and refuses if the size does not match what the disk
  reports now (so a renumbered drive is not overwritten by mistake).
- Filesystems mount **read-only** unless asked: `ext2mount ... rw`, `fs.rw`,
  and on the boot line `root=<drive>:<lba>` is read-only without `rootrw`.
- A default ISO boot never touches a disk: `/bin` and `/etc` are the read-only
  boot module, and programs that try to write `/etc` say so (`passwd` refuses
  rather than forgetting the password at reboot).
- `mikupkg` refuses to change anything without a writable root, and never
  remounts on its own.
- `mkfs.*`, `gpt.*`, `dd`, `blk*` need `CAP_SYS_ADMIN` and name the drive
  explicitly.

Booting an installed disk, or making the root writable on a live boot:

```text
root=LABEL=mikuos rootrw      the installed disk, writable
root=0:6144 rootrw            drive 0, the filesystem at LBA 6144, writable
root=0:6144                   the same, read-only
```

Storage, partitions and `mkfs` are in [storage.md](storage.md); boot flags in
[boot.md](boot.md).

---

## /etc reference

The repository's `etc/` is the image's `/etc` (read-only on a default boot).

| Path | Package | Purpose and format |
|---|---|---|
| `/etc/passwd`, `/etc/group`, `/etc/shadow` | miku-system | accounts ([above](#users-and-groups)) |
| `/etc/mikudo.conf` | miku-system | privilege policy ([above](#etcmikudoconf)), mode 0640 |
| `/etc/os-release` | miku-base | `NAME="MikuOS"`, `ID=mikuos`, `PRETTY_NAME="MikuOS"`, the freedesktop format; nothing in the tree reads it |
| `/etc/ssl/certs/ca-bundle.pem` | miku-net | the TLS trust store: the Mozilla CA list from `ca-certificates`, 121 roots, PEM; `lib/net` reads it at run time and refuses TLS without it ([network.md](network.md)) |
| `/etc/ssl/certs/README.md` | miku-net | where the bundle came from |
| `/etc/mikupkg/default.d/10-paths.conf` | miku-base | package manager paths and trust settings ([above](#mikupkg)) |
| `/etc/mikupkg/conf.d/`, `/etc/mikupkg/mikupkg.conf`, `/etc/mikupkg/keys` | | local package configuration and keyring (empty or absent by default) |
| `/etc/mayland/may.conf` | miku-desktop | the desktop's configuration head; layers `default.d/*.conf` (shipped), `may.conf`, `conf.d/*.conf` (yours), later wins ([mayland.md](mayland.md)) |
| `/etc/mayland/default.d/` | miku-desktop | `10-theme`, `15-greeter`, `20-look`, `30-input`, `40-anim`, `50-keys`, `60-rules` (and `70-negi` with `personal/`) |
| `/etc/mayland/session.d/10-desktop.conf` | miku-desktop | what a session starts: `session { start = /bin/maywm  start = /bin/aosh  start = /bin/mayterm }` |
| `/etc/mayland/apps.d/` | miku-desktop | launcher entries: `app { name = ... icon = ... exec = ... }` |
| `/etc/mayland/REFERENCE` | miku-desktop | the configuration reference |
| `/etc/aosh/` | miku-desktop | the desktop shell's QML (`shell.qml`, `Bar.qml`, `Launcher.qml`, `Chip.qml`, `Pill.qml`, `hello.qml`) and `negi/` ([aosh.md](aosh.md)); user copies go in `~/.config/aosh` |
| `/etc/aosh/default` | miku-personal | which aosh configuration to run (only with `personal/`) |
| `/etc/skel/` | miku-personal | what a new home starts with |
| `/etc/hostname` | miku-personal | the machine's name, one word (only with `personal/hostname`) |
| `/etc/mikud/*.service` | (none ship) | unit files for `sv scan` ([above](#unit-files)) |
| `/etc/hosts` | (none ships) | read by `getaddrinfo` before DNS, if present |

Machine state lives outside `/etc`: `/pkg/db`, `/pkg/cache`, `/pkg/journal`
(packages), `/run/mikudo.sock` (the broker's socket), `/tmp` (tmpfs, cores),
`/home` (homes).

---

## Time and the hostname

- **The wall clock** is set at boot from the RTC (`/proc/clock` says
  `source: rtc`), and can be corrected from NTP with `mikudo ntpdate [server]`
  (which needs `CAP_SYS_TIME`) or by any program with that capability through
  `clock_settime` (79). Until it is set, `CLOCK_REALTIME` fails. The time is
  kept in UTC.
- **The time zone** is a kernel offset in minutes (`/proc/clock`'s `tz_min`),
  set by the installer's time-zone screen. `libc_miku`'s `localtime` is UTC
  regardless; `date` prints UTC.
- **The hostname** is `/etc/hostname` when it exists (an image built with
  `personal/hostname`); `uname` reads it through `libc_miku`. The `msh` prompt
  always says `os`.

---

## Boot to desktop, from mikuD's view

1. The kernel brings up memory, devices, the VFS (with `/bin`, `/etc` and
   `/lib/firmware` from the boot module), the GPU's display, and registers the
   services above. It decides the target: graphical or multi-user
   ([Targets](#targets)), and logs `[mikud] booting to graphical (screen true,
   greeter true, noauth false)`.
2. mikuD starts as pid 1 and brings up the target's services, dependencies
   first: `kbd`, `usbd`, `netd`, `bdflush`, `kswapd`, `mikudod`, `shell`
   (after `kbd`), and for the graphical target `compositor` and then `greeter`
   (after `mikudod` and `compositor`, while seat 0 is free).
3. **Multi-user**: the console's `shell` asks who is there (`login`), unless
   `noauth`.
4. **Graphical**: `maygreet` shows the login. A login makes `mikudod` start
   `/bin/maysession` as the user and open the session; the greeter exits and is
   not restarted while the seat is taken. `maysession` starts `maywm`, `aosh`
   and `mayterm` from `/etc/mayland/session.d`, and `mayterm` runs `msh` on a
   new pty.
5. **Logout**: the window manager exits (the desktop's log-out), so
   `maysession` exits; the kernel closes the session and frees the seat; mikuD
   restarts `greeter`.
6. **Shutdown or reboot**: `power` (93), the console's `poweroff`/`reboot`, or
   a session's power request through `mikudod`: mikuD stops every service in
   reverse order, non-critical first, then powers off or resets.

The boot sequence itself (GRUB, the kernel's boot steps and timings) is in
[boot.md](boot.md).

---

## Troubleshooting

| Problem | Look at |
|---|---|
| The greeter never appears | serial: `[mikud] booting to multi-user (screen .., greeter .., noauth ..)` says why; `[mikud] no 'greeter' account in /etc/passwd`; `sv status greeter` |
| Login refused | `mikudo dmesg -g mikudo` (from another session) or serial: `failed graphical login for 'miku' (3 attempts)`, `login refused for ...: <why>`, `the kernel refused ...'s session` |
| `mikudo` refuses everything | a syntax error in `/etc/mikudo.conf` (the journal names the line), or the file's owner or mode is wrong; `mikudo -l` |
| `mikudo` says the daemon is not running | `sv status mikudod`; `[mikudod] not running as root - refusing to start` |
| `passwd`, `useradd` fail to save | `/etc` is the read-only boot copy: boot with `root=... rootrw` |
| A service keeps failing | `sv status <name>`, `sv journal <name>`; after 5 failures in 10 s it stays failed until `sv start` |
| A kernel command says "you are not root" | it needs `CAP_SYS_ADMIN`: prefix `mikudo` |
| Locked out entirely | boot with `noauth` |

---

## Where the code is

| What | Where |
|---|---|
| mikuD: main loop, lifecycle, restarts, shutdown | `kernel/mikud/daemon.rs` |
| Service model, policies, states | `kernel/mikud/service.rs` |
| Unit files | `kernel/mikud/unit.rs` |
| Targets, timers, sockets, journal, API | `kernel/mikud/{target,timer,socket,journal,api}.rs` |
| Built-in services and the target decision | `kernel/main.rs` |
| `sv`, `login`, `logout`, `who`, `install` | `kernel/shell/commands/mod.rs`, `install_cmds.rs`; `kernel/setup/` |
| Console login at boot, `noauth` | `kernel/shell/mod.rs` (`ask_who_is_there`) |
| Sessions | `kernel/process/session.rs` |
| Credentials and capabilities | `kernel/process/cred.rs` |
| The broker | `system/src/mikudod.rs` |
| Clients | `system/src/{mikudo,login,passwd,su,useradd,id,maygreet,maysession}.rs` |
| Hashes, account files, policy grammar, protocol | `lib/mikuauth/src/` |
| Package manager | `lib/mikupkg/`, `system/src/mikupkg/`, `system/src/mikupkg-apply.rs`, `builder/src/pkg.rs` |
| Writable root, `root=`, `rootrw` | `kernel/kcore/firmware/load.rs`, `kernel/fs/ext/mount.rs` |
| `/etc` | `etc/` in the repository |
