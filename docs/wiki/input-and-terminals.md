# Input and terminals

This page covers how a keystroke or a mouse movement gets from the hardware to a program, and what a terminal is on MikuOS: the PS/2 and USB drivers, keyboard layouts and key repeat, the kernel console, pseudo-terminals, the line discipline, terminal ioctls, and job control. Keyboard input from both PS/2 and USB is turned into Set-1 scancodes and fed into one ring; from there the compositor takes the keys a window should get, and the rest go to the terminal that has the keyboard. A terminal is a character grid owned by the kernel plus a line discipline; whoever draws it (mayterm, or the kernel console itself) only reads the grid.

## Contents

- [The input path at a glance](#the-input-path-at-a-glance)
- [PS/2 keyboard and mouse](#ps2-keyboard-and-mouse)
- [USB: xHCI, HID and usbd](#usb-xhci-hid-and-usbd)
- [Key decoding and layouts](#key-decoding-and-layouts)
- [Key repeat](#key-repeat)
- [The mouse](#the-mouse)
- [The kernel console](#the-kernel-console)
- [When the desktop is up](#when-the-desktop-is-up)
- [VT escape sequences](#vt-escape-sequences)
- [Terminals and pseudo-terminals](#terminals-and-pseudo-terminals)
- [The line discipline and termios](#the-line-discipline-and-termios)
- [Terminal ioctls and the tty syscall](#terminal-ioctls-and-the-tty-syscall)
- [Sessions, process groups and job control](#sessions-process-groups-and-job-control)
- [Keyboard shortcuts](#keyboard-shortcuts)
- [Diagnostics and testing](#diagnostics-and-testing)
- [Limits and gaps](#limits-and-gaps)
- [Code map](#code-map)

## The input path at a glance

```
PS/2 IRQ 1 ──────────────┐
                         ├─> io::input::stdin::push(scancode)   (keyboard interrupt / usbd thread)
usbd: HID report diff ───┘        │
                                  ├─ mayland::input::intercept(byte)   chords, Ctrl+Alt+Esc, Alt+Shift,
                                  │      │                              grab: key goes to the focused surface
                                  │      └─> lock-free ring ─> compositor thread ─> may_keyboard.key
                                  │
                                  └─> 64-byte stdin ring
                                          │
                              kbd_thread (pc-keyboard decoder, MikuUs104 layout)
                                          │
                     tty::active() is a pty, or the console has a foreground job?
                         yes ─> tty::feed_char / feed_key ─> line discipline ─> program's read()
                         no  ─> the kernel shell's own line editor (handle_keypress)

PS/2 aux / USB mouse ─> drivers::input::mouse (atomics) ─> compositor drains once a frame ─> may_pointer
```

Two rules shape all of it:

- **Interrupt context takes no locks.** `stdin::push` runs in the keyboard interrupt with interrupts off. It and `mayland::input::intercept` use only atomics and lock-free rings; the compositor thread drains them once a frame. The cost is at most one frame of latency.
- **One decoder.** Both keyboards become Set-1 scancodes before anything decodes them, so USB keys behave exactly like PS/2 keys everywhere.

## PS/2 keyboard and mouse

`kernel/drivers/input/ps2.rs` configures the 8042 controller before IRQ 1 is enabled, because GRUB and UEFI leave it in an unusable state (port 1 disabled, scanning off, the port-1 IRQ masked in the config byte). On UEFI desktops the 8042 is either a real controller on the LPC/eSPI bus or the firmware's "USB Legacy Support" emulation. The status register's bit 5 tells keyboard bytes from mouse (auxiliary port) bytes.

The mouse is initialised on port 2. The driver sends the IntelliMouse "knock" (sample rates 200, 100, 80); a wheel mouse then reports device id 3 and sends four-byte packets instead of three. `kernel/drivers/input/mouse.rs` assembles packets in the interrupt: bit 3 of the first byte is the only frame marker, so a stream that slipped by a byte is detected and resynchronised there; an overflowed packet's motion is dropped; Y is inverted to screen direction. Movement accumulates as a running total that a thread takes; buttons are a level.

**Testing USB input in QEMU.** `i8042=off` is a QEMU machine option, not a kernel flag: `-machine q35,i8042=off` removes the emulated PS/2 controller so QEMU's `sendkey` cannot be serviced by it and must go through xHCI. See [Diagnostics and testing](#diagnostics-and-testing).

## USB: xHCI, HID and usbd

`kernel/drivers/input/usb/` is a small USB stack for input devices.

| File | Role |
|---|---|
| `usb/mod.rs` | Scans PCI for xHCI controllers (class 0x0C, subclass 0x03, prog-if 0x30); the `usbd` service thread |
| `usb/xhci.rs` | xHCI host controller driver, polled (no interrupts) |
| `usb/hid_kbd.rs` | HID boot-protocol keyboard: diffs 8-byte reports into Set-1 make/break scancodes |
| `usb/hid_report.rs` | A minimal HID report-descriptor reader, enough to find buttons, X, Y and the wheel of a pointer with no boot protocol (tablets, touch screens, QEMU's `usb-tablet`) |
| `usb_handoff.rs` | Takes USB controllers from the firmware at boot (stops SMIs) |

Why it exists: taking the controllers from the firmware also ends the BIOS "USB Legacy Support" PS/2 emulation, so on real hardware a USB keyboard would go dead the moment the kernel starts. usbd drives xHCI natively and feeds the same stdin ring the PS/2 interrupt uses.

`usbd` is registered with mikuD. It claims each xHCI controller from the firmware (USBLEGSUP, before the controller reset, as xHCI 1.2 §4.22.1 requires), enables memory decode and bus mastering, brings the controller up, enumerates root ports, and then pumps every controller's event ring at the scheduler tick rate (250 Hz, so at most 4 ms of input latency). Interfaces are classified by their HID boot protocol (1 keyboard, 2 mouse); an interface without the boot subclass is still used if its report descriptor describes a pointer, and absolute pointers report where the pointer *is* rather than how far it moved.

Scope: xHCI only (every board since about 2012 routes all ports through it); root ports only, so a keyboard behind an external hub or dock is not reached. The `nousb` boot flag skips the USB stack. The `usb` command lists controllers and devices from status lines the usbd thread writes.

## Key decoding and layouts

`kbd_thread` (`kernel/io/input/keyboard.rs`) pops scancodes from the stdin ring (64 bytes) and decodes them with the `pc-keyboard` crate, using this system's own layout `MikuUs104` (`kernel/io/input/layout.rs`) instead of the crate's `Us104Key`.

**The `\` and `|` key.** `pc-keyboard` 0.7 decodes Set-1 scancode 0x2B to `KeyCode::Oem7`, but its US layout has an arm only for `Oem5`, where `\` and `|` live, so those two characters were silently lost. `MikuUs104` delegates every keycode to `Us104Key` and maps `Oem7` to `\` or `|` by the shift state. A diff of the whole table showed this is the only printable key Set 1 produces that the crate's US layout drops (fixed 2026-09-17). A layout is the right place because it is handed the modifiers, and because `\` must arrive through `tty::feed_char` like every character (non-character keys go through `feed_key`, which passes them only in raw mode).

**Layouts.** The tables are `libmay::keymap::LAYOUTS` (`lib/gfx/mayland/src/keymap.rs`), shared with every window: `us` (English US) and `ru` (Russian, xkb's `ru(winkeys)`). The seat has up to four layout groups (`MAX_GROUPS`); the default is `us,ru`. The active group is an atomic read by both the keyboard interrupt and the terminal decoder. The config sets the list and the switch through maywm (`input { kb_layout = us,ru; kb_options = grp:alt_shift_toggle }`, see [mayland.md](mayland.md)).

**Switching** (`kernel/mayland/input/switch.rs`). With `grp:alt_shift_toggle`, the group moves on when the second of Alt and Shift goes down while the first is held, either order, either side. It is watched in `intercept`, the one place every key passes, so it works in a window as well as at a console. The keys are not swallowed. A latch requires the pair to be released before the next switch, because the keyboard repeats make codes while keys are held. Windows are told through `may_keyboard.modifiers` (the group) and the bar through `may_desktop.keyboard_layout`. Ctrl and every key binding read the first layout whatever is active.

**Modifiers.** `kernel/io/input/mods.rs` records which modifier keys are physically down, fed from the crate's decoded `KeyEvent` (the crate's own modifier state is private and never sees left Alt). Shift+PageUp is built on it.

**Non-character keys** become the CSI sequences every terminal sends:

| Key | Sequence |
|---|---|
| Up, Down, Right, Left | `ESC [A`, `ESC [B`, `ESC [C`, `ESC [D` |
| Home, End | `ESC [H`, `ESC [F` |
| PageUp, PageDown | `ESC [5~`, `ESC [6~` |
| Insert, Delete | `ESC [2~`, `ESC [3~` |

Other non-character keys send nothing. These reach a program only in raw mode.

## Key repeat

Nothing in MikuOS times a held key. The PS/2 keyboard's own typematic generates repeated make codes, and every layer above only reacts to them. Consequences:

- `input { repeat_rate, repeat_delay }` reaches the keyboard through `ps2::set_typematic` (command 0xF3: rate in bits 0 to 4, delay in bits 5 and 6), as well as the `repeat_info` event clients are told. Sending only the event would tell a client a number nothing obeys. The kernel's default is 25 Hz after 600 ms; the shipped config sets 30 Hz after 400 ms.
- A key binding is matched on the make code, so without care every binding would repeat while held. A per-chord "held" mark makes a plain `bind` fire once per press; `binde` opts into repeat. The mark is cleared on the keycode alone, ignoring modifiers, because people lift Super before Q.
- The Alt+Shift layout switch needs its latch for the same reason.
- The USB HID keyboard path diffs reports into make and break codes and has no repeat timer, so a held key on a USB keyboard does not repeat.

## The mouse

Both mice feed `kernel/drivers/input/mouse.rs` (`feed_hid` for relative motion, `feed_abs` for absolute pointers). The compositor drains it once a frame (`kernel/mayland/input/pointer.rs`), scales motion by `input:sensitivity` (factor `1 + value`, clamped to 0.25 to 4, remainders kept per axis), flips the wheel for `natural_scroll`, and delivers `may_pointer` events to the surface under the pointer. Who owns the pointer during a window drag or an open popup is one question answered by `kernel/mayland/input/grab.rs`. With `follow_mouse` set, the keyboard follows the pointer. The cursor is drawn by the GPU's cursor plane on the GB206, or composed in software elsewhere; see [mayland.md](mayland.md).

`mayland status` prints the pointer's position, what it is over, packets and moves counted, and buttons held, or `no mouse - neither the 8042 nor USB produced one`.

## The kernel console

The kernel console is terminal zero (`TtyId` `SYSTEM`, 0x0100). Its text and pixels live in `kernel/io/console/mod.rs` behind the `WRITER` lock, where every `print!` in the kernel reaches it, including from interrupt handlers, the scheduler and the panic handler. It is the only terminal that survives the compositor stopping, and it is where a panic goes.

**The grid.** The text is a `TextGrid` of cells (a Unicode codepoint, foreground and background colour, attribute bits), sized from the framebuffer up to a ceiling of 256 columns by 96 rows. 256 rows that have scrolled off the top are kept as history, which is what lets a widened console re-flow its lines and what `may_console.set_view` scrolls through. Output is wrapped on the column. The bell (`\x07`) is counted on the grid for whoever draws it; `\f` and `\v` advance a line; DEL and C1 controls are not printed.

**The panel and its shadow.** On real hardware the firmware framebuffer is a PCI aperture (on the GB206, BAR1): writes are fast with write-combining, every read is an uncached round trip. Scrolling inside it used to read 8 MiB per line, 0.78 s a line on the GB206 and about 50 s of a 60 s boot. Since 2026-09-29 `console::shadow_panel()` runs right after the framebuffer is remapped write-combining: the console draws into a shadow in RAM, and the `WRITER` guard copies damaged rows to the panel when it is released. Nothing reads the panel; anything that composes the screen with the processor takes it from `console::panel()`. The `fbdirect` boot flag goes back to drawing straight into the framebuffer. `detach_framebuffer`/`attach_framebuffer` stop and resume framebuffer output (serial continues) while GSP-RM remaps BAR1 during display init, because stores into a BAR being remapped can wedge the link.

**The kernel shell.** On the console, with no foreground program, keys go to the kernel shell's own line editor (`kernel/shell/mod.rs` `handle_keypress`): Left, Right, Home, End, Delete, Backspace, Up and Down for history, Enter, Ctrl-C. It is the emergency console and keeps the commands that need ring 0 (the NVIDIA driver, block layer, mkfs, the selftests). On a text boot `login` runs first; the `noauth` boot flag boots straight to a root console. The shell runs as whoever owns the terminal it is typed at. The shell's commands are in [commands.md](commands.md); `msh`, the ring-3 shell, is in [msh.md](msh.md); login is in [system.md](system.md).

**Serial.** Everything printed to the console also goes to the serial port (COM1, 115200 baud). The log format and tags are described in [boot.md](boot.md) and [diagnostics.md](diagnostics.md).

## When the desktop is up

When compositing starts, `render::bring_up` calls `io::console::set_headless(true)`. From then until `mayland stop`, kernel text goes to the serial port and is kept in the grid, but nothing paints it. The in-kernel console window was deleted on 2026-08-20; the grid is shown only by a program that binds it through `may_console`.

- Desktop up, no terminal showing the console, a command printed nothing on screen: that is correct. `mayland status` says "nobody is drawing it - N by M characters held, serial only".
- `mayterm console` draws terminal zero from the desktop (it needs `CAP_MAY_PRIV`, so it must be started from `/etc/mayland/session.d`, where the line is shipped commented out). Plain `mayterm` hosts a shell on its own pty instead.
- `mayland stop` gives the screen back: `render::tear_down` resumes the console flush and calls `set_headless(false)`, which repaints it.
- A panic calls `render::emergency()`: it forces the console's surface to the front, forces a console flush and tries `set_headless(false)`, all with try-locks, since a lock held by the thread that died is never released.
- Under the login screen's keyboard lock, Ctrl+Alt+Esc does nothing, because behind the greeter is a root console.

## VT escape sequences

The console's output parser is `Vt` in `kernel/io/console/mod.rs`. It is the only escape-sequence parser in the system; mayterm has none, because it reads the parsed grid.

**Implemented:**

| Sequence | Name | Effect |
|---|---|---|
| `CSI n A` / `B` / `C` / `D` | CUU, CUD, CUF, CUB | move the pen up, down, right, left (default 1) |
| `CSI n G` (or `` ` ``) | CHA | to column n on this row |
| `CSI n d` | VPA | to row n in this column |
| `CSI r;c H` / `f` | CUP, HVP | to row r, column c (1-based; 0 means 1) |
| `CSI n K` | EL | erase 0 to end of line, 1 to its start, 2 the whole line |
| `CSI n J` | ED | erase 0 to end of screen, 1 to its start, 2 all |
| `CSI ... m` | SGR | attributes and colours, below |
| `ESC ] ... BEL` or `ESC \` | OSC (and DCS, PM, APC) | swallowed whole, so a window-title sequence is not printed |

**SGR.** 0 reset; 1 bold; 2 dim; 3 italic; 4 underline; 5 and 6 blink; 7 reverse; 8 hidden; 9 strike; 21 and 22 not bold or dim; 23, 24, 25, 27, 28, 29 turn the matching attribute off; 30 to 37 and 90 to 97 foreground; 40 to 47 and 100 to 107 background; 39 and 49 default colours; `38;5;n` and `48;5;n` from the xterm 256-colour cube; `38;2;r;g;b` and `48;2;r;g;b` true colour. Colours are xterm's values.

**Robustness.** A sequence carrying a private or intermediate byte (`?`, `<` to `>`, space to `/`) is not read as the plain final byte, so `CSI ? 25 l` is not an erase. An ESC inside a sequence abandons it and starts a new one rather than printing the rest as text. A sequence longer than its bound (32 characters for CSI, longer for string sequences) is abandoned, so a program that dies mid-sequence cannot silence the next one. The parser's state lives on the grid.

**The boundary.** A new sequence is refused if it **allocates, reshapes the grid, or writes into the input queue**, not because it moves the pen. Refused by name for those reasons: DSR (`CSI 6n`, whose answer goes into the input ring and would be `TIOCSTI` needing no privilege), DECCOLM, `CSI t`, the alternate screen, DECSTBM (scroll regions). `CSI E` and `CSI F` are also not implemented. All cursor work goes through `TextGrid::set_pen` and `blank_span`, the only two functions allowed to write the pen or index cells. Clearing a terminal is a request (`tty(TTY_CLEAR)`), not a byte pattern, so a program cannot clear somebody else's screen by printing at it.

## Terminals and pseudo-terminals

`kernel/io/tty/mod.rs` keeps a table of terminals. Terminal zero is the console (`console::WRITER`, special in the type because of the print path); the others are pseudo-terminals with a grid and no pixels of their own.

| Fact | Value |
|---|---|
| Terminals | `MAX_TTY` = 8: the console and 7 ptys |
| Terminal id | `TtyId`: slot index in the low 8 bits, a generation above; never 0 (0 means "no controlling terminal"). A reused slot gets a new generation, so a stale id cannot name somebody else's terminal |
| Console id | `SYSTEM` = 0x0100 |

**Device nodes** (`kernel/fs/devfs.rs`):

| Node | Meaning |
|---|---|
| `/dev/ptmx` | Opening it creates a pty and returns its master end; the descriptor points at a per-slot master node, never at `ptmx` itself. Open it close-on-exec, or a child holds the terminal open |
| `/dev/ptsN` | The slave end of pty N (flat, `ptsN` not `pts/N`). Openable by name only by a process the terminal's family rule admits (`tty::may_use`) |
| `/dev/tty` | The caller's controlling terminal; `ENXIO` for a process without one |
| `/dev/console` | Terminal zero |

Master nodes cannot be opened by name: a master is something you are given by creating the pty, not something you take. `ioctl(fd, TIOCGPTN, &id)` on a master returns the `TtyId`.

**Lifetime.** The master count lives in the vnode's reference count (`VNode::inc_ref`/`dec_ref` call `tty::master_ref`/`master_unref`), so a forked or duplicated master is counted. The terminal hangs up when the last copy of the master descriptor closes, which also covers the holder being killed. The hangup is deferred to `tty::run_hangups`, run after the VFS lock is dropped. After hangup reads give EOF and writes EIO, and the slot is reused only after the last reference is dropped. A client drawing the terminal over `may_console` is sent `gone`.

**Output.** What a program prints is written through `TextGrid::write_str`, the single door to a pty's cells, and also into a ring the master reads, which is fed only while a master is open (otherwise a terminal with no master would stall its writers). Kernel-coloured lines (`cprintln!`) reach a master uncoloured; a program's own SGR does reach it.

**The family rule.** Only the terminal's master, or a process descended from it, may join it (`tty(TTY_ATTACH)`, `TIOCSCTTY`, opening `/dev/ptsN`). A terminal is never reachable by name or number from outside the family that made it. There is no `TIOCSTI` and no input-injection primitive; a master may write into its own pty only. There are no pluggable line disciplines. No input or output path allocates, because both are reached from interrupts.

**Drawing a terminal.** `/bin/mayterm` opens `/dev/ptmx`, sets the window size with `TIOCSWINSZ`, binds the terminal with `may_console.bind`, forks, and execs `/bin/msh` on it. When the window is resized it tells the kernel through `may_console.set_size`; the kernel re-wraps the lines and sends SIGWINCH to the foreground job. Keys reach the pty through the kernel (`tty::active()` follows the focused console window), not through mayterm. See [mayland.md](mayland.md) for `may_console` and mayterm.

**Scrolling back.** Shift+PageUp and Shift+PageDown move the view half a screen through history (`tty::set_view`) instead of reaching the program. Typing returns to the live screen; output does not.

## The line discipline and termios

`kernel/io/tty/ldisc.rs` is one state machine per terminal: handed a character, it answers with what to echo and which signal to raise, and does neither itself (`Feed`); the caller acts after the lock is gone. This keeps echo (another lock) and signals (the scheduler) out of the discipline's critical section.

| Buffer | Size |
|---|---|
| Read ring (what `read` returns) | 1024 bytes |
| Line buffer (canonical editing) | 256 bytes |

**Termios** is a POSIX subset, not Linux's 60-byte struct (there are no baud rates or modem lines):

| Field | Meaning | Cooked default |
|---|---|---|
| `icanon` | canonical mode: input a line at a time, with editing | on |
| `echo` | echo input | on |
| `isig` | INTR, QUIT and SUSP characters raise signals | on |
| `c_cc[7]` | the control characters | below |
| `vmin` | raw mode: fewest bytes a read returns; 0 with `vtime` 0 is a non-blocking read | 1 |
| `vtime` | raw mode: how long a read waits, tenths of a second; 0 is forever | 0 |
| `tostop` | stop a background job that writes (SIGTTOU) | off |

| Slot | Index | Default | Action |
|---|---|---|---|
| `VINTR` | 0 | 0x03 Ctrl-C | clears the line and pending input, echoes `^C`, SIGINT to the foreground |
| `VQUIT` | 1 | 0x1C Ctrl-\ | echoes `^\`, SIGQUIT |
| `VERASE` | 2 | 0x7F DEL (0x08 also accepted) | erase one character (a whole UTF-8 character, not a byte) |
| `VKILL` | 3 | 0x15 Ctrl-U | erase the line, rubbing out one cell per character |
| `VEOF` | 4 | 0x04 Ctrl-D | on an empty line, the next read returns 0; mid-line, flush what was typed |
| `VWERASE` | 5 | 0x17 Ctrl-W | erase trailing spaces, then a run of non-spaces |
| `VSUSP` | 6 | 0x1A Ctrl-Z | echoes `^Z`, SIGTSTP to the foreground |

A slot set to 0 (`VDISABLE`) disables that character. Signal characters are consumed before mode is considered. In raw mode (`icanon` off) every byte of every character goes straight to the read ring, echoed if `echo` is on, and escape sequences for non-character keys are delivered. In canonical mode Enter moves the line buffer into the read ring; Tab is data. A new foreground program inherits a cooked terminal.

## Terminal ioctls and the tty syscall

`ioctl` (syscall 71, `kernel/syscall/adapters/io.rs`) implements only terminal requests, with Linux's numbers:

| Request | Number | Argument | Rules |
|---|---|---|---|
| `TCGETS` | 0x5401 | `MikuTermios` (12 bytes: `icanon`, `echo`, `isig`, `vmin`, `vtime`, `tostop`, `c_cc[7]`, 1 pad byte) | any terminal descriptor |
| `TCSETS` | 0x5402 | `MikuTermios` | from the background: SIGTTOU unconditionally (returns `EINTR`) |
| `TIOCGWINSZ` | 0x5413 | `{ u16 rows, u16 cols }` | the size of the terminal this descriptor names |
| `TIOCSWINSZ` | 0x5414 | `{ u16 rows, u16 cols }` | master descriptor only (`EPERM` otherwise); re-wraps and sends SIGWINCH |
| `TIOCSCTTY` | 0x540E | — | make this terminal the caller's controlling terminal (same as `tty(TTY_ATTACH)`), subject to the family rule |
| `TIOCGPTN` | 0x80045430 | `u32` | master only: the `TtyId` |

Which terminal an ioctl is about is decided by the descriptor, not the fd number: fds 0, 1 and 2 with nothing redirected mean the caller's controlling terminal; a descriptor on `/dev/tty` or `/dev/ptsN` names that terminal; a master names its pty and carries the master's rights. A file redirected onto fd 0 gets `ENOTTY`. Anything else returns `ENOTTY` or `EINVAL`.

The `tty` syscall (99, `kernel/syscall/adapters/tty.rs`) does what ioctl does not:

| Op | Value | Meaning |
|---|---|---|
| `TTY_ATTACH` | 1 | the caller's controlling terminal becomes `arg` (family rule) |
| `TTY_SELF` | 3 | which terminal the caller is on; 0 is the console |
| `TTY_SETFG` | 4 | `tcsetpgrp`: hand the terminal's keys to process group `arg`; 0 means the caller's own group |
| `TTY_GETFG` | 5 | `tcgetpgrp` |
| `TTY_CLEAR` | 6 | blank the caller's terminal and home the pen |
| `TTY_OF_PID` | 7 | which terminal another process is on (for a daemon running a command for somebody) |

Ops 0 (`TTY_OPEN`) and 2 (`TTY_CLOSE`) were removed: a pty comes from `/dev/ptmx` and ends when its master closes. Old binaries calling them get `EINVAL`. The full syscall reference is [../MikuOS_ABI.md](../MikuOS_ABI.md); libc's `openpty` and `login_tty` are built on `/dev/ptmx`, `/dev/ptsN` and `TIOCSCTTY` (see [sdk.md](sdk.md)).

## Sessions, process groups and job control

The pieces, each implemented:

- **Controlling terminal.** `Process::ctty`, inherited on fork and kept across exec; `print!` routes output by it.
- **Process groups.** `setpgid` (syscall 101). A shell calls it on both sides of a fork so neither order leaves the child in the wrong group.
- **Foreground group.** `tty(TTY_SETFG, pgid)`; the line discipline sends INTR, QUIT and SUSP signals to the foreground group of the terminal they were typed at.
- **Stopping.** Ctrl-Z sends SIGTSTP. The scheduler has a distinct stopped state that only SIGCONT leaves; stopping another process is applied by the switch that next puts it away, so a program making syscalls stops at its next one and a pure loop at its next preemption. SIGSTOP is uncatchable. `wait4` reports stops with POSIX status words when asked with `WUNTRACED`.
- **Background reads and writes.** A background job reading its terminal gets SIGTTIN. A background job writing gets SIGTTOU only when `tostop` is set. `TCSETS` from the background is SIGTTOU unconditionally. On a terminal with no foreground job at all, a background reader gets end of input instead (there is nobody to bring it forward).
- **SIGWINCH** goes to the foreground job when the terminal's size changes.

In msh: `cmd &` runs in the background, Ctrl-Z stops the foreground job, `jobs` lists jobs, `fg [n]` hands the terminal to job n before sending SIGCONT, `bg [n]` continues it in the background, `wait` waits for all. On the kernel console with no foreground job, Ctrl-C sets the network stack's cancel flag and prints `^C`. Details: [msh.md](msh.md) for the shell, [processes.md](processes.md) for signals, process groups and `wait4`.

## Keyboard shortcuts

**Matched in the keyboard interrupt by the compositor, before anything else:**

| Keys | Effect | Notes |
|---|---|---|
| Ctrl+Alt+Esc, or F12 | take the keyboard back from a client | only while a client holds the keyboard; inert under the login screen's lock |
| Ctrl+Alt+Tab | move the keyboard to the next window | inert under the lock |
| Alt+Shift (either order) | next keyboard layout | with `kb_options = grp:alt_shift_toggle`; keys are not swallowed |
| maywm binds | whatever the config says | 64 chords at most; see [mayland.md](mayland.md) |

**Shipped maywm binds** (`/etc/mayland/default.d/50-keys.conf`, `$mod` = Super): Super+Return terminal; Super+D launcher; Super+Q close; Super+F fullscreen; Super+Shift+F float; Super+P pin; Super+M cycle layout; Super+C centre; Super+Shift+R reload the config; Super+Left/Right/J/K move focus; Super+Shift+Left/Right swap; Alt+left drag move, Alt+right drag resize; Ctrl+1..9, Ctrl+0 workspaces 1 to 10; Ctrl+Shift+digit move the window there; Super+Tab previous workspace; Super+Shift+Tab next occupied workspace; Super+R resize submap (H/L split ratio, arrows resize, Escape or Return to leave); Ctrl+W `/bin/zed`.

**In a terminal:**

| Keys | Effect |
|---|---|
| Ctrl-C, Ctrl-\, Ctrl-Z | SIGINT, SIGQUIT, SIGTSTP to the foreground job |
| Ctrl-D | end of input on an empty line |
| Ctrl-U, Ctrl-W, Backspace | erase line, word, character (canonical mode) |
| Shift+PageUp, Shift+PageDown | scroll the view half a screen through history |

**At the kernel console prompt:** Left, Right, Home, End, Delete, Backspace, Up/Down history, Enter, Ctrl-C.

## Diagnostics and testing

| Want | Do |
|---|---|
| Which USB devices were found | `usb`; serial lines tagged `[usb]` (controller, slot, first report of each device) |
| Whether a mouse is seen | `mayland status` (pointer section) |
| Keyboard layout state | `mayland status` (keyboard section); `may_desktop.keyboard_layout` in the bar |
| Terminal behaviour without booting the desktop | the tty selftest, `kernel/io/tty/selftest.rs` (run through the selftest suites; see [testing.md](testing.md)) |
| Who ran a kernel command from ring 3 | serial `[kcmd] pid=N ran '...'` |

**USB keyboard in QEMU.** Remove PS/2 so it cannot answer for USB:

```
qemu-system-x86_64 -machine q35,i8042=off -cdrom miku.iso -m 2G -smp 2 \
  -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0 \
  -serial file:serial.log -display none -monitor unix:mon.sock,server,nowait
```

Type with the monitor's `sendkey`. QEMU's xHCI reports no port power control and has no USBLEGSUP capability, so the firmware hand-off and port-power paths that matter on real hardware are not exercised. Add `-device usb-tablet,bus=xhci.0` for an absolute pointer.

**PS/2 mouse in QEMU.** The monitor's `mouse_move dx dy` is one PS/2 packet: a jump bigger than roughly 250 pixels is dropped, and back-to-back moves are coalesced, so pause about a second between moves and read the pointer's real position from a `screendump`. `mouse_button` is a bitmask; 2 is the right button.

## Limits and gaps

- 8 terminals in all (the console and 7 ptys). A program that opens many terminals (an editor's terminal panes) will hit it.
- Layouts: `us` and `ru` only; no xkb, no variants, no compose key, no dead keys.
- USB: xHCI root ports only (no hubs), polled at 250 Hz, boot-protocol keyboards; USB keyboards do not repeat held keys.
- No touchpad driver and no pointer acceleration curve.
- VT: no scroll regions, alternate screen, DSR, `CSI E`/`CSI F`, or window-size reports; OSC sequences are swallowed, not acted on.
- No `TIOCSTI`, no pluggable line disciplines, no baud rates or modem lines.
- The stdin scancode ring is 64 bytes; the read ring 1024; a canonical line 256 bytes.

## Code map

| Path | Contents |
|---|---|
| `kernel/drivers/input/ps2.rs` | 8042 init, keyboard and mouse ports, typematic |
| `kernel/drivers/input/mouse.rs` | Mouse packet assembly and the shared pointer state |
| `kernel/drivers/input/usb/` | xHCI, HID keyboard and pointer, usbd |
| `kernel/drivers/input/usb_handoff.rs` | Taking USB controllers from the firmware |
| `kernel/io/input/stdin.rs` | The 64-byte scancode ring both keyboards feed |
| `kernel/io/input/keyboard.rs` | `kbd_thread`: decode, route to a terminal or the kernel shell |
| `kernel/io/input/layout.rs` | `MikuUs104`, the seat's layout groups |
| `kernel/io/input/mods.rs` | Physically held modifiers |
| `kernel/io/input/user_stdin.rs` | The caller's terminal: `mine`, `ctty`, `termios`, `size`, foreground |
| `kernel/io/console/mod.rs` | The console: grid, history, VT parser, panel shadow, headless mode |
| `kernel/io/tty/mod.rs` | Terminal table, ptys, hangup, foreground, size, view |
| `kernel/io/tty/ldisc.rs` | Line discipline and termios |
| `kernel/io/tty/selftest.rs` | Terminal selftest |
| `kernel/syscall/adapters/io.rs` | `ioctl` |
| `kernel/syscall/adapters/tty.rs` | The `tty` syscall |
| `kernel/fs/devfs.rs` | `/dev/ptmx`, `/dev/ptsN`, `/dev/tty`, `/dev/console` |
| `kernel/mayland/input/` | Compositor side: chords, escape keys, layout switch, pointer, grabs, input params |
| `lib/gfx/mayland/src/keymap.rs` | Layout tables and the keymap wire format |
| `kernel/shell/mod.rs` | The kernel shell's line editor |
| `system/src/mayterm/` | The terminal window |

Design history: [../../dev/terminal-architecture.md](../../dev/terminal-architecture.md).
