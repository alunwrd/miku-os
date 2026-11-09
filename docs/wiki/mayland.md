# Mayland

Mayland is the MikuOS display system: a protocol, a compositor and the desktop policy around them. It copies Wayland's model and its wire format byte for byte, but the display server is part of the kernel rather than a separate process, so shared memory is named by a handle in the message stream instead of a file descriptor. Clients are ordinary ring-3 programs (`/bin/maywm`, `/bin/aosh`, `/bin/mayterm`, `/bin/maygreet`) that connect over a socket and talk through `libmay`, the same crate the kernel half is built from. This page describes the protocol, the compositor, the window manager and its config language, and how to work on any of them.

## Contents

- [What Mayland is](#what-mayland-is)
- [The layer tree](#the-layer-tree)
- [The protocol](#the-protocol)
  - [Transport and wire format](#transport-and-wire-format)
  - [Interfaces and versions](#interfaces-and-versions)
  - [Who may bind what](#who-may-bind-what)
  - [Surfaces, buffers and shared-memory pools](#surfaces-buffers-and-shared-memory-pools)
  - [Buffers in video memory](#buffers-in-video-memory)
  - [Windows, popups and the clipboard](#windows-popups-and-the-clipboard)
  - [Seat: pointer and keyboard](#seat-pointer-and-keyboard)
  - [The layer role](#the-layer-role)
  - [Outputs](#outputs)
  - [may_desktop and may_wm](#may_desktop-and-may_wm)
  - [may_console](#may_console)
  - [Errors and limits](#errors-and-limits)
- [Rendering](#rendering)
- [The window manager: maywm](#the-window-manager-maywm)
- [may.conf: the config language in full](#mayconf-the-config-language-in-full)
- [mayterm, maygreet and maysession](#mayterm-maygreet-and-maysession)
- [Displays and modes](#displays-and-modes)
- [Locks](#locks)
- [The `mayland` command](#the-mayland-command)
- [The selftest](#the-selftest)
- [Writing a client with libmay](#writing-a-client-with-libmay)
- [Performance notes](#performance-notes)
- [Diagnostics and troubleshooting](#diagnostics-and-troubleshooting)
- [Verification status](#verification-status)
- [Known gaps](#known-gaps)
- [Code map](#code-map)

## What Mayland is

| Part | Where | What it does |
|---|---|---|
| Display server | `kernel/mayland/server` | Connections, object ids, surfaces, buffers, pools, focus. Sends and receives; decides nothing |
| Desktop policy | `kernel/mayland/shell` | Tiling arithmetic, workspaces, animations, closing-window afterimages, the wallpaper |
| Compositor | `kernel/mayland/render` | The frame loop: damage, copy engine, compute pass, cursor, processor blending |
| Input | `kernel/mayland/input` | Keyboard and mouse from the interrupt to the focused surface; chords; pointer grabs |
| Protocol library | `lib/gfx/mayland` (crate `libmay`) | Wire format, protocol table, request encoder, client connection, keymaps, config grammar, drawing toolkit |
| Window manager | `system/src/maywm` | Reads `/etc/mayland`, holds `may_wm`, sends layout, params, key chords, window rules, monitor lines |
| Desktop shell | `aosh/` | Bar, launcher, overview, written in QML. See [aosh.md](aosh.md) |
| Terminal | `system/src/mayterm` | Draws a terminal's character grid; hosts `/bin/msh` on its own pty |
| Greeter | `system/src/maygreet.rs` | The login screen |
| Session starter | `system/src/maysession.rs` | Starts the programs listed in `/etc/mayland/session.d` |

The compositor runs on a kernel thread. `mayland::compositor_thread` (`kernel/mayland/mod.rs`) is started by mikuD for the graphical target: it calls `render::start`, then wakes once a second (250 ticks at the 250 Hz timer) to restart compositing if it stopped by itself. A stop requested with `mayland stop` is respected and not undone. If compositing cannot start (no framebuffer) the thread prints `[mayland] cannot compose (...) - no graphical session` and parks; mikuD's fallback notices the greeter never came up. `mayland::init` publishes the screen size to `may_output` once the console has a framebuffer and reserves the wallpaper's memory early, before the GSP firmware load fragments physical memory.

## The layer tree

`kernel/mayland` is cut into layers. The cut is the point: each answers a different question.

| Layer | Contents | Depends on |
|---|---|---|
| `proto` | Wire format, protocol table, request encoder, keymap, object table, outgoing queue, in-kernel client (`client.rs`). Re-exported from `libmay` | nothing above it |
| `gfx` | Canvas, glyphs, widgets (`font`, `paint`, `text`, `ui`). Re-exported from `libmay` | nothing above it |
| `server` | `state` (types behind ids, leases), `dispatch` (bytes to calls), `iface/*` (one file per interface), `surface` (commit, configure, frame ends), `scene` (flattened scene for the renderer), `focus`, `shm`, `selection`, `access`, `held` | `proto`, and calls *up* into `shell` |
| `shell` | `layout` (master/dwindle/float), `workspace`, `anim`, `ghost` (closing afterimages), `wallpaper`, `popup`, `demo` | `server` state |
| `render` | `damage`, `effects`, `outline`, `cursor`, `compose`, `present`, `texture`, `base`, `phases` | `server` state; the only layer that touches the GPU |
| `input` | Keyboard routing, `pointer`, `grab`, `conf` (input params), `switch` (layout switching) | `server` entry points |
| `selftest` | The protocol selftest, filed by subject | everything |

**The send-or-decide line.** A function belongs in `server` if it sends or receives a message (for example `set_focus`, which sends enter/leave) and in `shell` if it decides something (for example `switch_workspace`, which decides what is on screen). When a request implies a decision, the server calls up into the shell through a few named functions (`layout::arrange`, `workspace::switch_workspace`, `ghost::ghost_window`). That seam is deliberate.

**Path rule.** Inside a layer, refer to a sibling with `super::sibling`. Across layers, use the full `crate::mayland::layer::module`. A `super::super::` means a file is in the wrong directory.

**Visibility rule.** `server/state.rs` has two tiers: `pub(in crate::mayland)` for the fields the shell and renderer read (geometry, animation, workspace flags) and `pub(in crate::mayland::server)` for everything else (connection buffers, leases, buffer ownership, object ids). If the shell needs something from the second tier, the build fails; move the work into the server instead of widening the field.

Design history: [../../dev/mayland-architecture.md](../../dev/mayland-architecture.md).

## The protocol

### Transport and wire format

A client opens a Mayland connection with `socket(AF_MAYLAND, SOCK_STREAM, 0)`, where `AF_MAYLAND` is 42 (`lib/gfx/mayland/src/conn.rs`), then uses `send` (58) and `recv` (59). Object id 1 is the display and exists before anything is said. A process may hold at most 4 Mayland sockets (`MAX_MAYLAND_PER_PID` in `kernel/net/socket.rs`); they come out of the same socket table as TCP.

The encoding (`lib/gfx/mayland/src/wire.rs`) is Wayland's:

```
u32  sender object id
u32  (size << 16) | opcode      size includes the 8-byte header
...  arguments, each 4-byte aligned
```

| Type | Meaning |
|---|---|
| `i` | int32 |
| `u` | uint32 |
| `f` | 24.8 signed fixed point stored as int32 (`wl_fixed_t`) |
| `o` | object id, 0 is null |
| `n` | new_id the sender is creating |
| `s` | u32 length including the trailing NUL, the bytes, zero-padded to 4; length 0 is a null string |
| `a` | u32 byte count, the bytes, padded to 4 |

There is no `h` (file descriptor) type. The largest message is 4096 bytes including the header (`MAX_MESSAGE_SIZE`, libwayland's value). Interface names, opcode numbers and argument orders follow `wayland.xml`, with `may_` in place of `wl_`; the two deliberate differences are that `may_shm.create_pool` and `may_keyboard.keymap` carry a u32 shared-memory handle where Wayland carries a descriptor.

### Interfaces and versions

The table is `lib/gfx/mayland/src/protocol.rs`. Versions are what the code implements.

| Interface | Version | Requests (opcode) | Events (opcode) |
|---|---|---|---|
| `may_display` | 1 | `sync` 0, `get_registry` 1 | `error` 0, `delete_id` 1 |
| `may_registry` | 1 | `bind` 0 | `global` 0, `global_remove` 1 |
| `may_callback` | 1 | — | `done` 0 |
| `may_compositor` | 1 | `create_surface` 0, `create_region` 1 | — |
| `may_shm` | 1 | `create_pool(new_id, uint handle, int size)` 0 | `format` 0 |
| `may_shm_pool` | 1 | `create_buffer(new_id, offset, width, height, stride, format)` 0, `destroy` 1, `resize` 2 | — |
| `may_buffer` | 1 | `destroy` 0 | `release` 0 |
| `may_surface` | 1 | `destroy` 0, `attach` 1, `damage` 2, `frame` 3, `set_opaque_region` 4, `set_input_region` 5, `commit` 6, `set_buffer_transform` 7, `set_buffer_scale` 8, `damage_buffer` 9, `offset` 10, `set_fence` 11 | `enter` 0, `leave` 1 |
| `may_region` | 1 | `destroy` 0, `add` 1, `subtract` 2 | — |
| `may_seat` | 1 | `get_pointer` 0, `get_keyboard` 1, `release` 2 | `capabilities` 0, `name` 1 |
| `may_pointer` | 1 | `set_cursor` 0, `release` 1 | `enter` 0, `leave` 1, `motion` 2, `button` 3, `axis` 4 |
| `may_keyboard` | 1 | `release` 0 | `keymap` 0, `enter` 1, `leave` 2, `key` 3, `modifiers` 4, `repeat_info` 5 |
| `may_output` | 2 | `release` 0 | `geometry` 0, `mode` 1, `done` 2, `scale` 3, `name` 4 (v2), `description` 5 (v2) |
| `may_xdg` | 1 | `destroy` 0, `create_positioner` 1, `get_xdg_surface` 2, `pong` 3 | `ping` 0 |
| `may_xdg_surface` | 1 | `destroy` 0, `get_toplevel` 1, `get_popup` 2, `set_window_geometry` 3, `ack_configure` 4 | `configure` 0 |
| `may_toplevel` | 1 | `destroy` 0, `set_parent` 1, `set_title` 2, `set_app_id` 3, `show_window_menu` 4, `move` 5, `resize` 6, `set_max_size` 7, `set_min_size` 8, `set_maximized` 9, `unset_maximized` 10, `set_fullscreen` 11, `unset_fullscreen` 12, `set_minimized` 13 | `configure(width, height, states)` 0, `close` 1 |
| `may_positioner` | 1 | `destroy` 0, `set_size` 1, `set_anchor_rect` 2, `set_anchor` 3, `set_gravity` 4, `set_constraint_adjustment` 5, `set_offset` 6, `set_reactive` 7, `set_parent_size` 8, `set_parent_configure` 9 | — |
| `may_popup` | 1 | `destroy` 0, `grab` 1, `reposition` 2 | `configure` 0, `popup_done` 1, `repositioned` 2 |
| `may_desktop` | 5 | `get_windows` 0, `focus` 1, `close` 2, `workspace` 3, `move_to_workspace` 4, `signal` 5 (v3), `capture` 6 (v5) | `window` 0, `windows_done` 1, `opened` 2, `closed` 3, `focused` 4, `workspace_changed` 5, `work_area` 6, `signalled` 7 (v3), `keyboard_layout` 8 (v4), `captured` 9 (v5) |
| `may_wm` | 1 | `move` 0, `resize` 1, `set_layout` 2, `set_gaps` 3, `bind_chord` 4, `unbind_chord` 5, `set_param` 6, `set_window_param` 7, `set_window_state` 8, `swap` 9, `set_curve` 10, `set_animation` 11, `set_border` 12, `begin_move` 13, `begin_resize` 14, `warp_pointer` 15, `reset` 16, `set_wallpaper` 17, `set_layer_rule` 18 | `chord` 0 |
| `may_console` | 1 | `set_size` 0, `detach` 1, `paste` 2, `bind` 3, `set_view` 4 | `grid` 0, `damage` 1, `gone` 2 |
| `may_layer` | 2 | `get_layer_surface(new_id, surface, band)` 0, `destroy` 1, `get_focus_grab` 2 (v2) | — |
| `may_layer_surface` | 1 | `set_anchor` 0, `set_size` 1, `set_exclusive` 2, `set_keyboard` 3, `destroy` 4, `ack_configure` 5, `set_namespace` 6 | `configure` 0, `closed` 1 |
| `may_focus_grab` | 1 | `add_surface` 0, `remove_surface` 1, `commit` 2, `destroy` 3 | `cleared` 0 |
| `may_clipboard` | 1 | `set` 0, `get` 1, `clear` 2 | `offer` 0, `data` 1, `lost` 2 |
| `may_output_manager` | 1 | `create_configuration(new_id, serial)` 0, `stop` 1 | `head` 0, `done(serial)` 1, `finished` 2, `mode` 3 |
| `may_output_configuration` | 1 | `set` 0, `disable_head(string)` 1, `apply` 2, `test` 3, `destroy` 4 | `succeeded` 0, `failed(reason)` 1, `cancelled` 2 |

A client that binds an older version of `may_desktop` or `may_output` is sent the older event shapes (for example a version-1 `window` event has no app id).

### Who may bind what

Access is a filter on the registry, as in `wl_display_set_global_filter`. It is consulted in exactly two places in `server/iface/display.rs`: the loop that advertises globals and `bind`. A hidden global is never advertised, and binding it by a guessed name is a fatal protocol error. The decision is `server/access.rs`.

| Tier | Interfaces | Who |
|---|---|---|
| public | `may_compositor`, `may_shm`, `may_seat`, `may_output`, `may_xdg`, `may_clipboard`, `may_console` | anything connected |
| privileged | `may_desktop`, `may_layer` | a process holding `CAP_MAY_PRIV`, or the kernel's own clients (pid 0) |
| control, one holder | `may_wm` | privileged, and exactly one client; holding it is commanding |
| control | `may_output_manager` | privileged, and only the holder of `may_wm` (mode and refresh are policy) |
| per terminal | `may_console` | public as a global; `bind(tty)` decides: your own pty (you hold its master), or the system console (needs `CAP_MAY_PRIV`) |

`CAP_MAY_PRIV` follows the seat (`kernel/process/session.rs`). The greeter holds it while nobody is logged in; when a session opens, the kernel moves it to the session leader, `maysession`, which hands it to the programs in `/etc/mayland/session.d` and to nothing else. `spawn::run` drops it from programs those start, so an application opened from the launcher cannot cover the screen or read the keyboard. A pid with no live process is refused (`sched::cred_checked`), so a dead connection cannot be promoted. The privilege model as a whole is described in [../MikuOS_Security.md](../MikuOS_Security.md).

**Leases.** `may_wm` and each terminal's `may_console` are leases in `server/state.rs`. Every "who holds it" question goes through `state::holder`, which drops the lease of a dead client, and the lease is recorded only after `bind_new` succeeds. Before this, one deliberately failed bind could take `may_wm` for the life of the machine.

### Surfaces, buffers and shared-memory pools

**Pools.** A pool is physically contiguous memory with a u32 handle (`kernel/mayland/server/shm.rs`). The client gets it mapped into its address space; the compositor reaches it through the kernel's direct map, and the copy engine reads it by physical address, so a commit becomes one engine copy with nothing touching the pixels on the way.

| Syscall | Number | libmay | Effect |
|---|---|---|---|
| `may_pool` | 72 | `Pool::new(size)` | Allocate a contiguous pool and map it; returns handle and address |
| `may_unpool` | 73 | `Pool::free()` | Destroy a pool you own |
| `may_map` | 74 | `Pool::adopt(handle, size)` | Map a pool the compositor made for you (the keymap, a clipboard offer, a capture) |
| `gpu_alloc` | 121 | `Pool::video(size)` | A pool of video memory, see below |

The full syscall reference is in [../MikuOS_ABI.md](../MikuOS_ABI.md).

| Limit | Value | Source |
|---|---|---|
| Largest pool | 64 MiB | `MAX_POOL_BYTES` |
| Pools live at once, system-wide | 64 | `MAX_POOLS` |
| Pools per process | 16 | `MAX_POOLS_PER_OWNER` (owner 0, the kernel, is exempt) |

A handle is not a capability, so ownership is recorded and checked: `map_into` refuses a pool to any process but its owner and refuses a second address space. A destroyed pool's frames are not returned to the allocator at once. They go first out of the GPU's page tables (`unbind_range`), then onto a retired list, and `shm::reap` frees them after the next frame's engine wait proves the engine has stopped reading them. With no compositor running they are freed immediately. `server::disconnect` destroys every pool a client made.

**Lent memory (`shm::adopt`).** Memory the kernel keeps for its own lifetime is lent rather than pooled. `adopt(owner, phys, virt, size)` makes a pool that borrows existing contiguous kernel memory and frees nothing when destroyed. The console's screen-sized shadow and the wallpaper's buffer (reserved once at boot, lent to every `mayland start`) use it. Before this the wallpaper's pool died at the first `mayland stop` and later starts had no wallpaper.

**Buffers and their lifetime.** `create_buffer` names a rectangle of a pool (`FORMAT_ARGB8888` = 0 or `FORMAT_XRGB8888` = 1, as in `wl_shm`). A client attaches a buffer, damages, asks for a `frame` callback and commits. The compositor sends `release` when it has finished reading the buffer; the copy engine is waited on at the top of the next frame, which is why `server::frame_composed` and `server::frame_done` are two calls at two ends of the frame. A client draws only into buffers it has had back.

**A committed buffer must be whole.** The compositor keeps a damage debt per scanout surface (`render/damage.rs`, `OWED`) and pays it by re-reading the client's current buffer over the whole owed region, not only the rows the client damaged. A client that commits a half-painted buffer shows two alternating pictures. mayterm had exactly this bug until 2026-09-11.

**Fences.** `set_fence(fence_lo, fence_hi)` attaches a GPU fence the commit's pixels are waiting on, for a client that drew with `gpu_submit`. The compositor absorbs the wait with a budget per frame.

**Other surface rules.** A surface without a role is not shown. A buffer committed before the first `ack_configure` is refused (`ERR_UNCONFIGURED_BUFFER`). The only buffer transform accepted is `TRANSFORM_NORMAL`; rotation would cost a pass the copy engine cannot do. Regions are limited to 32 rectangles each and 64 per client.

### Buffers in video memory

Since 2026-10-20 a pool can be video memory (`dev/gpu-window-buffers.md`), which is this system's analogue of DMA-BUF limited to one case: a window whose buffer is in VRAM. `Pool::video(size)` calls `gpu_alloc` (121); the NVIDIA driver hands out a block under BAR1 (`drivers::gpu::nvidia::user_vram`), and `shm::adopt_video` makes it a pool under the same table limits. The owner maps it write-combining through BAR1: stores are fast, loads cross the bus one at a time. It returns `ENODEV` on a machine with no card or no BAR1 window and `EDQUOT` when programs hold all the VRAM they may.

The compositor reads such a buffer in place: the frame sets `gpu = vram` and `phys = 0` on the presentable. The processor never reads VRAM directly; `render/texture.rs` mirrors only the rectangles it has to blend, and downloads a window for the closing afterimage and for `may_desktop.capture`. Any new processor reader of window pixels must handle `phys == 0`. The test client is `gpudemo` (`apps/src/gpudemo.rs`), which draws a window with the `fill` GPU program; it exits on `backend none` in QEMU. See [../Nvidia_gb206.md](../Nvidia_gb206.md) for the allocator.

### Windows, popups and the clipboard

**Toplevels.** `may_xdg.get_xdg_surface` then `get_toplevel` give a surface the window role. The compositor sizes tiled windows itself and tells the client through `may_toplevel.configure(width, height, states)` followed by `may_xdg_surface.configure(serial)`; states are `MAXIMIZED` 1, `FULLSCREEN` 2, `RESIZING` 3, `ACTIVATED` 4. `close` asks a window to go; it never takes it. `set_title` and `set_app_id` are what window rules and the bar match on.

**Popups.** A positioner is a recipe (size, anchor rectangle, anchor, gravity, constraint adjustment, offset) and `get_popup` places a surface against its parent with it. Anchor and gravity take `NONE`, `TOP`, `BOTTOM`, `LEFT`, `RIGHT`, `TOP_LEFT`, `BOTTOM_LEFT`, `TOP_RIGHT`, `BOTTOM_RIGHT` (0 to 8); constraint adjustment is a mask of `SLIDE_X` 1, `SLIDE_Y` 2, `FLIP_X` 4, `FLIP_Y` 8, `RESIZE_X` 16, `RESIZE_Y` 32. `grab` takes the pointer until the popup is dismissed (`popup_done`). Popup chains are at most 8 deep (`MAX_POPUP_DEPTH` in `input/grab.rs`); a chain that does not end is refused rather than spun on. Dragging a window and an open menu are one pointer question, answered by one lock and one enum in `input/grab.rs`. When a popup maps or a surface leaves under a stationary pointer, `focus::repoint` recomputes the crossing, because crossings are otherwise only computed on motion.

**Clipboard.** `may_clipboard` copies Wayland's rules, not its shape (which needs file descriptors): only a client that was just typed at may `set` the selection, and only the focused client may `get` it. Two selections exist, `CLIPBOARD` 0 and `PRIMARY` 1. A selection is at most 64 KiB, with at most 8 MIME types of up to 128 bytes each (`server/selection.rs`). The content is copied twice: a reader is handed a pool made in its own name (`data(which, handle, size)`), and its previous offer pool is freed first so a paste loop cannot use up its pool allowance.

### Seat: pointer and keyboard

`may_seat` advertises `CAP_POINTER` 1 and `CAP_KEYBOARD` 2. The pointer sends enter/leave/motion/button/axis in surface coordinates; `set_cursor` replaces the arrow with the client's own surface or with nothing.

The keyboard sends `keymap(format, handle, size)` with format `KEYMAP_MAY_V2` (3): a pool the compositor made, which the client maps with `Pool::adopt`. It carries the seat's layouts (up to 4 groups, layout codes such as `us` and `ru`) and is decoded by `libmay::keymap`. Keys are Linux evdev keycodes; `modifiers` carries the modifier mask and the active group. `repeat_info(rate, delay)` is pushed whenever the config changes it. Layouts, typematic repeat and the input path are described in [input-and-terminals.md](input-and-terminals.md).

### The layer role

`may_layer.get_layer_surface(new_id, surface, band)` gives a surface the layer role, shaped after wlr-layer-shell. A layer surface is sticky (on every workspace), is not in the focus cycle, and is sized by the compositor through the same configure handshake as a window.

| Band | Value | Typical use |
|---|---|---|
| `BACKGROUND` | 0 | wallpaper |
| `BOTTOM` | 1 | desktop widgets under the windows |
| `TOP` | 2 | bars, the launcher |
| `OVERLAY` | 3 | lock and login screens |

`Server::stack()` sorts surfaces by `(band, z)`; that is the whole stacking model.

| Request | Arguments | Meaning |
|---|---|---|
| `set_anchor` | mask of `TOP` 1, `BOTTOM` 2, `LEFT` 4, `RIGHT` 8 | Which edges the surface is attached to |
| `set_size` | width, height | 0 on an anchored axis means "stretch" |
| `set_exclusive` | int | Pixels of the edge reserved from the tiling work area (`layout::Reserved`); -1 is the default (none) |
| `set_keyboard` | `NONE` 0, `ON_DEMAND` 1, `EXCLUSIVE` 2 | Whether the surface takes the keyboard |
| `set_namespace` | string | The name layer rules match (aosh sends `WlrLayershell.namespace`, for example `quickshell:overview`) |

A layer surface that takes the keyboard leaves `Server::active` (the window drawn as active and reported to the bar) alone, as Hyprland does.

**Focus grab** (`may_layer` v2, Hyprland's `hyprland_focus_grab_v1`). While a client's grab is committed, the pointer reaches only the listed surfaces (at most 16), and a press anywhere else ends the grab with `cleared` and goes no further. aosh's overview closes on an outside click this way.

**The keyboard lock.** An `EXCLUSIVE` keyboard request from an `OVERLAY` surface becomes a lock only when all three hold: the band is `OVERLAY`, the client holds `CAP_MAY_PRIV` (or is pid 0), and no session owns the seat (`may_lock` in `server/iface/layer.rs`). Under the lock `input::intercept` swallows every scancode, including Ctrl+Alt+Esc, F12, Ctrl+Alt+Tab and every window-manager chord, because behind the login screen is a root console. Inside a session the same request is the launcher and stays interruptible. A watchdog (`iface::layer::watchdog`, once a frame) releases a lock held by a process that is gone, or by one that has not committed a frame within 10 s (`LOCK_FIRST_FRAME_MS`), and asks the watcher thread to stop compositing so the console is painted again.

### Outputs

`may_output` (v2) tells every client the screen size and refresh (`mode`), the connector name (`name`, for example `DP-3`, stable for the machine's life) and the monitor's make and model (`description`).

`may_output_manager` and `may_output_configuration` are wlr-output-management in two interfaces (stage K3 of [../../dev/kms-architecture.md](../../dev/kms-architecture.md)). The manager sends every connector as `head` (name, description, flags `CONNECTED` 1, `ENABLED` 2, `PRIMARY` 4, current and preferred mode, adaptive-sync state `VRR_UNSUPPORTED` 0 / `VRR_OFF` 1 / `VRR_ON` 2, bit depth), each mode as `mode(head, index, width, height, refresh)`, then `done(serial)`. A head is a name and a mode is an index; neither is a server-made object. A configuration is built with `set`, then `test` or `apply`; `KEEP` (-1) leaves a field as it is. The server's lock is a spin lock and a mode change sleeps on RPC, so `test` and `apply` are handed to a kernel task and the answer arrives later as `succeeded`, `failed(reason)` or `cancelled`. Only the holder of `may_wm` may bind the manager. The display layer itself (`drivers::gpu::kms`) is described in [../Nvidia_gb206.md](../Nvidia_gb206.md).

### may_desktop and may_wm

**`may_desktop`** (v5) is the facts and the verbs a user asks for. Any number of privileged clients bind it; a bar and the window manager hear the same events.

- `get_windows(serial)` is answered by one `window` per window (client, surface, title, app id, x, y, size, flags, workspace) and `windows_done`. Flags: `FOCUSED` 1, `STICKY` 2, `BACKDROP` 4, `FLOATING` 8, `FULLSCREEN` 16, `PINNED` 32, `NO_FOCUS` 64.
- `focus`, `close`, `workspace(index)`, `move_to_workspace` are user intent.
- `signal(string)` (v3) says one word to every listener and means nothing by it; listeners receive `signalled`. This is how `bind = $mod, D, signal, launcher toggle` reaches aosh's `IpcHandler` without the compositor knowing what a launcher is.
- `capture(client, surface, pool, offset, serial)` (v5) copies a window's last committed pixels, unscaled, into the asker's own pool, answered by `captured`. aosh's overview uses it.
- `work_area` reports the tiling area after reserved edges; `keyboard_layout` (v4) reports the active layout for a bar.
- There are 10 workspaces (`desktop::WORKSPACES`).

**`may_wm`** (v1) is policy. Exactly one client holds it (maywm). Its requests:

| Request | What it sets |
|---|---|
| `set_layout` | `LAYOUT_FLOAT` 0, `LAYOUT_MASTER` 1, `LAYOUT_DWINDLE` 2 |
| `set_gaps` | outer and inner gaps |
| `set_param(param, value)` | one numbered knob, see the table below |
| `set_window_param`, `set_window_state` | one window's own params and its `FLOATING`/`FULLSCREEN`/`PINNED`/`NO_FOCUS` state (what window rules become) |
| `bind_chord`, `unbind_chord` | key chords matched in the keyboard interrupt; at most 64 (`MAX_CHORDS`); a match arrives as the `chord` event |
| `set_curve`, `set_animation`, `set_layer_rule` | named Bézier curves (16), animation classes, `layer_rule::NOANIM` |
| `set_border` | border size and the two colours, alpha carried end to end |
| `move`, `resize`, `swap`, `begin_move`, `begin_resize`, `warp_pointer` | geometry and pointer grabs |
| `set_wallpaper` | the picture behind everything |
| `reset` | everything back to the compositor's defaults, so a reload re-applies a description instead of stacking changes |

Chord flags (`protocol::wm::chord`): `REPEAT` 1 (fire while held), `RELEASE` 2 (fire on release), `PASSTHROUGH` 4 (also deliver the key), `LOCKED` 8, `IGNORE_MODS` 16.

Params (`protocol::wm::param`), with the clamps applied by the kernel:

| Param | Number | Kernel default | Range |
|---|---|---|---|
| `GAPS_OUT` | 0 | 16 | pixels |
| `GAPS_IN` | 1 | 12 | pixels |
| `SPLIT_RATIO` | 2 | 1.0 | 0.1 to 1.9 |
| `MASTER_FACTOR` | 3 | 0.5 | 0.1 to 0.9 |
| `EFFECTS` | 16 | off | 0 or 1 (the blur/blend pass) |
| `ALPHA` | 17 | 1.0 | 0 to 1, the active window's opacity |
| `ROUNDING` | 18 | 12 | pixels |
| `SHADOW` | 19 | 0.45 | 0 to 1 |
| `SHADOW_SIZE` | 20 | 24 | pixels |
| `BLUR_SIZE` | 21 | 2 | tap offset per level |
| `ROUNDING_POWER` | 22 | 2.0 | 1 to 10 |
| `INACTIVE_ALPHA` | 23 | 1.0 | 0 to 1 |
| `DIM` | 24 | 0 | 0 to 1 |
| `ANIM_SPEED` | 32 | | |
| `ANIM_CURVE` | 33 | | |
| `ANIM_ENABLED` | 34 | | |
| `REPEAT_RATE` | 48 | 25 Hz | 0 to 100 |
| `REPEAT_DELAY` | 49 | 600 ms | 0 to 4000 |
| `FOLLOW_MOUSE` | 50 | 0 | non-zero: the keyboard follows the pointer |
| `SENSITIVITY` | 51 | 0 | factor `1 + value`, clamped to 0.25 to 4 |
| `NATURAL_SCROLL` | 52 | 0 | 0 or 1 |
| `KB_LAYOUTS` | 53 | `us` | packed layout list |
| `KB_SWITCH` | 54 | 0 | 1: Alt+Shift switches layouts |

Defaults live beside the state they belong to (`Effects::DEFAULT`, `Border::DEFAULT`, `Params::DEFAULT`, `input::conf::reset`) so `reset` and a fresh boot agree. A request carrying a number is a change, not a description, which is why a reload starts with `reset`.

### may_console

`may_console` lets a client draw a terminal: the kernel keeps the character grid (pen, wrapping, re-flow, scrollback, line discipline), and the client keeps the pixels. Nothing in it carries glyphs or colours of the client's own.

1. `bind(tty)` is sent once, first. A client may bind a pty whose master it holds (it created it), or the system console if it holds `CAP_MAY_PRIV`. A second bind is a protocol error.
2. The server answers `grid(handle, size, cols, rows)`: a pool holding the cell grid, allocated once at the console's ceiling and never re-allocated. `size` is capacity; `cols` and `rows` are what is in use, row-major from the front.
3. `set_size(cols, rows)` says how many cells fit; the client owns the arithmetic because it owns the font. A size with no room for a character is refused.
4. `damage(row0, row1, cursor_col, cursor_row)` arrives at most once a frame with the union of changed rows.
5. `set_view(rows_back)` scrolls through history; 0 is live. Typing returns to 0; output does not.
6. `paste(string)` injects text as if typed (recorded and replayed a frame later). `detach` gives the console back. `gone` says the pty's master closed; the pool stays mapped and stops changing.

A cell is 16 bytes (`CELL_BYTES`): the character as a little-endian u32 codepoint, foreground r, g, b, background r, g, b, attributes as a little-endian u16, then 5 bytes of padding. Attribute bits: `BOLD` 1, `DIM` 2, `ITALIC` 4, `UNDERLINE` 8, `BLINK` 16, `REVERSE` 32, `HIDDEN` 64, `STRIKE` 128, `BG_SET` 256 (the background was set at all). The terminal side is described in [input-and-terminals.md](input-and-terminals.md).

### Errors and limits

A handler that returns an error has decided the client is broken: the error is queued against the object it came in on (`may_display.error(object, code, message)`), the connection is marked dead, and nothing more is dispatched for it. A request that legitimately cannot apply returns success and does nothing. Generic codes: `ERR_INVALID_OBJECT` 0, `ERR_INVALID_METHOD` 1, `ERR_NO_MEMORY` 2, `ERR_IMPLEMENTATION` 3.

| Limit | Value | Where |
|---|---|---|
| Queued outgoing bytes per connection | 256 KiB; over it the connection is killed rather than an event dropped | `proto/outq.rs` `MAX_QUEUED` |
| Mayland sockets per process | 4 | `net/socket.rs` |
| Pools | 64 total, 16 per process, 64 MiB each | `server/shm.rs` |
| Selection | 64 KiB, 8 MIME types | `server/selection.rs` |
| Region rectangles / regions per client | 32 / 64 | `server/state.rs` |
| Popup depth | 8 | `input/grab.rs` |
| Key chords | 64 | `protocol::wm::MAX_CHORDS` |
| Focus-grab surfaces | 16 | `protocol::focus_grab::MAX_SURFACES` |
| Layer rules | 32 | `shell/anim.rs` |
| Named curves | 16 | `shell/anim.rs` |
| Windows in the compute pass | 12 | `drivers/gpu/nvidia/blur.rs` `MAX_WINDOWS` |
| Largest window copy in VRAM | 16 MiB of buffer (a 4K window is read from client memory) | `render/texture.rs` |

## Rendering

### The frame

Every frame takes the same five steps on either path (`render/mod.rs`):

1. wait for the previous frame's engines and hand buffers back (`frame_done`);
2. drain input, tick animations, pump the console and wallpaper;
3. ask the server for the scene, flattened (`presentable_for_frame`);
4. work out what changed and copy exactly that;
5. say the frame happened (`frame_composed`).

The frame takes its list of damage under the server lock, composes without it, and retires only the damage it read (`damage_read`). On the card, frame callbacks go out at the top of the frame, so a client commits while `present` runs; anything committed after the frame took its list belongs to the next frame.

**Two paths.** With the NVIDIA card, frames are paced by the raster and copied by the copy engine, with the effects pass on the compute engine. Without it (QEMU, the firmware framebuffer, Bochs), frames are paced by the scheduler tick, 4 ticks (16 ms at 250 Hz, `FRAME_TICKS`), and copied by the processor (`blit_batch_cpu` into `drivers::gpu::softscan`). `bring_up` is the shared tail of both starts. Frame timestamps come from the TSC (`layout::now_us`), not the 4 ms tick, so clients animating off callback differences see even steps at 240 Hz.

**Stacking.** `present` copies surfaces bottom to top where they meet the damage; the last one written wins. The damage is the union of what clients said they drew, where a surface was and is if it moved, and where a surface used to be if it is gone. More than 16 damaged rectangles in a frame (`MAX_RECTS`) are merged.

**Window copies in VRAM (`render/texture.rs`).** On the card every window has a copy in video memory, updated only with what the client changed, and the GPU reads windows from there. More than 8 damaged rectangles in one commit go up as one. The boot flag `notex` turns this off and reads every window from its client's memory.

### The effects pass: blur, xray, shadows, borders and the window shape

Effects are on when `decoration:blur:enabled` (param `EFFECTS`) is set. On the card the pass is a chain of compute launches (`drivers::gpu::nvidia::blur`, shaders in `kernel/drivers/gpu/nvidia/shaders/`):

- **Below the windows.** The wallpaper and the run of `BACKGROUND` and `BOTTOM` layer surfaces are folded into one VRAM image, updated only where a member changed (`render/base.rs`, `shaders/over.cu`, `blur::submit_base`), and blurred once with a Kawase blur into a screen-sized "blurred" image. This is Hyprland's order (wallpaper, BACKGROUND, BOTTOM, pre-blur, windows) with `xray` on: see-through windows and layers show this blurred backdrop rather than what is directly behind them. Design: [../../dev/mayland-below-windows.md](../../dev/mayland-below-windows.md).
- **Each window** is one composite launch: blurred backdrop where it is see-through, the window's pixels at its opacity, the squircle outline and border, and the drop shadow in its margin. The margin is cut to the screen and to effect windows below (`effects::margin_for`).
- **See-through surfaces above** (layers over windows) are taken by the pass and stacked: such a surface reads the frame below it. Overlapping windows' clips settle both ways: a lower window redoes what an upper one reads, and an upper window redoes what a lower one writes over it (`effects::choose`, `settle_stacked_clips`). An opaque plain surface is refused (`Refusal::Plain`), and an undrawn surface between two pass windows refuses (`Refusal::Between`). Design: [../../dev/mayland-layers-above.md](../../dev/mayland-layers-above.md).
- **`effects::TAKEN`** decides who dresses a window: what the pass took is drawn by the pass; what it refused is drawn by the processor (`compose::dressed_by`).
- **Inter-launch ordering.** QMDs in one submission run concurrently, so the chain puts `INVALIDATE_SHADER_CACHES` between launches. See [../Nvidia_gb206.md](../Nvidia_gb206.md).

**The window shape.** Windows are outlined as Hyprland v0.56.2 does it: a superellipse ("squircle") corner whose radius is `rounding · rounding_power / 2` (22 at power 2.5 is 27.5 px), cut one pixel inside; a blended border whose colour carries its alpha (alpha 0 means solid); `inactive_opacity` and `dim_inactive`/`dim_strength` on every window but the active one. The one definition is `libmay::shape` (`lib/gfx/mayland/src/shape.rs`); the processor reads it through tables (`render/outline.rs`) and the card through `composite.cu`. Change it there and nowhere else. Design: [../../dev/mayland-window-shape.md](../../dev/mayland-window-shape.md).

**The processor path** (`render/compose.rs`) blends, pixel by pixel in integer arithmetic, whatever is see-through and not taken by the pass: fades, shadows, rounded corners, translucent layers. In QEMU it is the only path.

**What QEMU cannot show.** QEMU has no compute pass, so there is no blur, no GPU fade, no VRAM window copies and no folded backdrop, and frame callbacks go out after `frame_composed` rather than at the top of the frame. (On 2026-10-07 no window borders were drawn in QEMU at all; since 2026-10-24 the processor draws the outline and border itself through `render/outline.rs`, which is what the QEMU checks of the window shape looked at.) Blend-order, border-colour and pass bugs therefore appear only on the card; a clean QEMU screendump says nothing about them. The pass decisions themselves are covered by `selftest/below.rs` without hardware.

### Cursor

On the GB206 the head's cursor plane draws the pointer on every boot that does not carry `nohwcursor` (the default since 2026-09-11). On QEMU and on any head without a plane, the compositor composes a software cursor patch (`render/cursor.rs`) by rebuilding the rectangle under the pointer every frame, because the copy engine cannot blend. The patch is the fallback; do not try to make it reproduce the effects pass exactly.

### Animations

Animations are curves the compositor ticks itself (`shell/anim.rs`, `layout::tick`); the algorithms only write a goal (`layout::apply` is the only thing that moves a window). Classes are `windows`, `windowsIn`, `windowsOut`, `layers`, `layersIn`, `layersOut`, `fade`, `fadeIn`, `fadeOut`, `fadeLayers`, `fadeLayersIn`, `fadeLayersOut`, `workspaces`, `workspacesIn`, `workspacesOut`. Styles are `slide`, `fade` and `popin <n>%`. An animation's alpha has its own clock (`alpha_ms`, `alpha_curve`), so a layer can grow over 270 ms while its alpha rises in 50. A closing window leaves an afterimage (`shell/ghost.rs`) that is animated out. A layer opens at its first buffer (`layout::layer_shown`). `fade` and `popin` need the pass for GPU blending; on the processor path they are blended pixel by pixel. Layers do not animate out yet.

### The slow-frame log

`render/phases.rs` times each step of a frame (engines, handback, pumps, curves, fences, scene, copies, blend, handover). Two lines go to `dmesg debug`: `a frame took N us on the processor: ...` past the slow threshold, with every phase, and `the screen went N us without a frame ...` past 2.5 raster periods, with which counters moved meanwhile.

## The window manager: maywm

`/bin/maywm` (`system/src/maywm`) holds `may_wm` and `may_desktop` and drives the desktop through them. It does not compute geometry: the compositor decides how big a tiled window is and where it goes, as Hyprland's dwindle and master live in Hyprland. maywm chooses the layout, its numbers, the exempt windows and what keys do. Two consequences: the desktop still works with maywm not running, and a window reaches the screen without a round trip to ring 3.

| File | Role |
|---|---|
| `main.rs` | Startup, the built-in fallback config, the event loop, `apply` (reset then re-send everything), reload |
| `config.rs` | The dictionary: what each config line means to a window manager |
| `binds.rs` | Key names, modifiers, dispatchers and their arguments |
| `rules.rs` | Window rules, applied once per window (and again for `floating:` rules when floating changes) |
| `outputs.rs` | `monitor =` lines applied through `may_output_manager` |
| `state.rs` | What the compositor has said is on the screen |

**Layouts** (`kernel/mayland/shell/layout.rs`):

| Layout | Behaviour |
|---|---|
| `master` | One window on the left (width `master:mfact`), the rest stacked on the right. The kernel's default |
| `dwindle` | Every new window splits the one it opened on along that window's longer side, by `dwindle:split_ratio`; the arrangement is a tree. The shipped config's choice |
| `float` | Windows keep the size they drew and are placed out of each other's way |

Every algorithm measures against the work area (the screen less `gaps_out` and less what layer surfaces reserve), never the screen.

**Workspaces.** Ten. A workspace argument may be a number (`3`), relative (`+1`, `-1`; `r` and `m` prefixes are accepted and mean the same with one output), occupied-only (`e+1`, `e-1`: counting only workspaces with windows; with none occupied it falls back to a plain step), or `previous`/`prev`. An absolute number out of range, or `previous` with no history, does nothing.

**Chords.** maywm's binds are compiled into chords the kernel matches in the keyboard interrupt, so they work whoever has the keyboard. The compositor keeps two chords for itself, matched first and impossible to bind over: Ctrl+Alt+Esc (and F12) take the keyboard back from a client, and Ctrl+Alt+Tab passes it to the next window. Both are inert under the login screen's lock. A modifier-only release binding (`bindr = SUPER, Super_L`) fires only on a tap of that key alone.

**Mouse binds.** A mouse button is an ordinary key: evdev numbers buttons above every keyboard key, so `mouse:272` (left) and `mouse:273` (right) need no separate table. `movewindow` and `resizewindow` bound to a button begin a pointer drag; bound to a keyboard key they become `swapwindow` and `resizeactive`.

**Without a config.** If no file under `/etc/mayland` can be read, maywm uses a built-in config (`BUILT_IN` in `main.rs`): `$mod = SUPER`, gaps 12/16, master layout, a 2 px border, Ctrl+1..9/0 for workspaces and a handful of Super binds. It is parsed on every boot, so a dictionary regression prints a complaint even on a machine with no config.

**Reload.** `bind = $mod SHIFT, R, reload` re-reads every file of every layer into a fresh config. Only if it came from a file is it swapped in; `apply` sends `may_wm.reset`, then every setting, chord, rule and curve again, and the `monitor =` lines are re-applied. If nothing could be read, maywm says `config: nothing could be read from /etc/mayland - the desktop is unchanged` and keeps the old one. There is no socket to command maywm from outside (no `hyprctl`).

**exec-once.** Lines run once after the desktop is up, not on reload. Programs maywm starts do not inherit `CAP_MAY_PRIV`.

## may.conf: the config language in full

The grammar is `lib/gfx/mayland/src/conf.rs` (shared by every reader); the window manager's dictionary is `system/src/maywm/config.rs`; the file list is `lib/gfx/mayland/src/manifest.rs`. `/etc/mayland/REFERENCE` is a shipped summary that nothing parses; where it disagrees with this page, the code (and this page) is current.

### Where the config lives

| Layer | Path | Who writes it |
|---|---|---|
| 1 | `/etc/mayland/default.d/*.conf` | the build; rewritten on every image |
| 2 | `/etc/mayland/may.conf` | this machine |
| 3 | `/etc/mayland/conf.d/*.conf` | you; nothing is ever installed here |

Files are read in that order, and in name order inside a directory; **the later line wins**. Only `*.conf` is read. A file in `conf.d` with the same name as one in `default.d` replaces it (the shipped one is not read at all), so an empty `conf.d/50-keys.conf` switches off every shipped binding. At most 24 files make up a config (`MAX_FILES`), 16 per directory (`MAX_IN_DIR`). At startup and after every reload maywm lists every file it read.

Two other directories live beside it and are read by other programs: `/etc/mayland/session.d/*.conf` (`maysession`, see below) and `/etc/mayland/apps.d/*.conf` (the launcher's program list, read by aosh; see [aosh.md](aosh.md)).

Shipped files:

| File | Contents |
|---|---|
| `default.d/10-theme.conf` | `theme { }`: wallpaper and palette, read by maywm (wallpaper) and aosh (`Theme`) |
| `default.d/15-greeter.conf` | `greeter { }`: the login screen's palette and portrait |
| `default.d/20-look.conf` | `general`, `decoration`, `dwindle` |
| `default.d/30-input.conf` | `input { }` |
| `default.d/40-anim.conf` | `animations`, `bezier`, `animation` |
| `default.d/50-keys.conf` | `bind`, `bindm`, `binde`, the `resize` submap |
| `default.d/60-rules.conf` | `windowrule`, `layerrule` |
| `may.conf` | `$mod = SUPER` |

### Lexical rules

- A line is `key = value`, a block opener `name {`, or `}`. Blocks nest two deep; the section is the block path joined with dots (`decoration.blur`).
- `#` begins a comment at the start of a line or after a space or tab. So `#rrggbb` after `= ` is a comment; write colours as `rgb()`, `rgba()` or `0x`.
- A number may have a sign and a fractional part. `true`, `yes`, `on` read as 1 and `false`, `no`, `off` as 0.
- A colour is `rgb(RRGGBB)`, `rgba(RRGGBBAA)`, `0xRRGGBB`, `0xRRGGBBAA`, or bare hex digits (6 or 8). A colour without an alpha is solid. `rgba(...)` alpha means see-through, not darker.
- Text values (rule patterns, commands, paths) are at most 64 bytes (`MAX_TEXT`).

### Variables and `source`

```
$mod = SUPER
bind = $mod, Q, killactive
source = /etc/mayland/extra.conf
```

- `$name = value` defines a variable. Variables are collected from every file of every layer before any line is dispatched, so a file read later can redefine one used earlier (`$mod = ALT` in `conf.d` changes every shipped bind). The later definition wins.
- A variable's value may not contain `$` (refused with "a variable's value may not contain another variable"); this makes cycles unwritable.
- At most 32 variables. A line containing `$` may be at most 256 bytes after expansion; lines without `$` are not measured.
- `source = <path>` reads another file in place, up to 4 deep. The path is not variable-expanded. The file must be one the manifest already read.

### List keywords

These keys are lists rather than settings. A list keyword with an empty value (`bind =`) **clears** that list.

| Keyword | Clears |
|---|---|
| `bind`, `bindm`, `binde`, ... | all bindings and submaps |
| `windowrule`, `windowrulev2` | window rules |
| `layerrule` | layer rules |
| `animation` | animations |
| `bezier` | curves |
| `exec`, `exec-once` | startup commands |
| `monitor` | monitor lines |
| `unbind` | (removes one binding; see below) |

### Settings

| Section | Key | Effect | Shipped value |
|---|---|---|---|
| `general` | `gaps_in` | pixels between windows | 4 |
| `general` | `gaps_out` | pixels between windows and the screen edge | 4 |
| `general` | `layout` | `master`, `dwindle`, `float`/`floating` (anything else is master) | dwindle |
| `general` | `border_size` | border width in pixels; 0 is none | 1 |
| `general` | `col.active_border` | colour, alpha blended | `rgba(41484d77)` |
| `general` | `col.inactive_border` | colour, alpha blended | `rgba(181c2033)` |
| `decoration` | `rounding` | corner, pixels (the drawn radius is `rounding · rounding_power / 2`) | 22 |
| `decoration` | `rounding_power` | superellipse power, 1 to 10 | 2.5 |
| `decoration` | `active_opacity` | the active window's opacity | 1.0 |
| `decoration` | `inactive_opacity` | every other window's opacity | 0.9 |
| `decoration` | `dim_inactive` | darken inactive windows | true |
| `decoration` | `dim_strength` | how much | 0.05 |
| `decoration` | `shadow_strength` | 0 to 1 | — |
| `decoration` | `shadow_range` | pixels | — |
| `decoration.shadow` | `range` | same as `shadow_range` | — |
| `decoration.blur` | `enabled` | turn the effects pass on (blur, blended corners, shadows) | true |
| `decoration.blur` | `size` | blur tap offset | 8 |
| `dwindle` | `split_ratio`, `default_split_ratio` | 0.1 to 1.9, 1.0 is even | 1.0 |
| `master` | `mfact` | master width, 0.1 to 0.9 | — |
| `animations` | `enabled` | animations on or off | true |
| `animations` | `speed`, `curve` | global speed and curve | — |
| `input` | `kb_layout` | `us`, `ru`, or a comma list of up to four, in switching order | `us,ru` |
| `input` | `kb_options` | `grp:alt_shift_toggle` or empty (no switch) | `grp:alt_shift_toggle` |
| `input` | `repeat_rate` | Hz, 0 to 100; programs the keyboard's own typematic | 30 |
| `input` | `repeat_delay` | ms, 0 to 4000 | 400 |
| `input` | `follow_mouse` | non-zero: keyboard follows the pointer | 0 |
| `input` | `sensitivity` | -0.75 to 3 (factor `1 + value`) | 0 |
| `input` | `natural_scroll` | flip the wheel | false |
| `misc` | `vrr` | 0 or 1: variable refresh for outputs whose monitor line does not say | — |
| `theme` | `wallpaper` | path to a QOI file; no line means the built-in gradient | `/etc/mayland/wallpapers/miku.qoi` |
| `submap` (top level) | `submap = <name>` ... `submap = reset` | lines between belong to that key map | — |

`kb_variant`, `kb_model`, `kb_rules` and `kb_file` are refused with "there is no xkb here". `ru` is xkb's `ru(winkeys)`; Ctrl and every bind read the first layout whatever is active.

The `theme` block also holds the palette (`page`, `card`, `accent`, `on_accent`, `text`, `text_muted`, `border`), read by aosh; `greeter` holds the login screen's palette (`page`, `card`, `border`, `text`, `text_muted`, `field`, `accent`, `on_accent`) and `image`, read by maygreet. maywm passes over `theme` (except `wallpaper`), `panel` and `greeter` silently. Nothing reads `panel` any more: its reader (mayshell) was deleted when aosh replaced it.

Wallpapers must be QOI. The build converts anything in `assets/wallpapers/` into `/etc/mayland/wallpapers/<name>.qoi` and anything in `assets/icons/` into 64 px icons; see [building.md](building.md). The picture is scaled to cover the screen and cropped evenly.

### Key bindings

```
bind[flags] = MODS, KEY, DISPATCHER[, ARGUMENT]
unbind = MODS, KEY
```

**Flags** (letters after `bind`, up to 8):

| Letter | Meaning |
|---|---|
| `e` | repeat while held |
| `r` | fire on release |
| `n` | do not swallow the key (the focused client also gets it) |
| `i` | ignore other modifiers |
| `l` | accepted (there is no lock screen to bind through) |
| `m` | accepted; a mouse binding is a binding whose key is a button |

Any other letter is complained about ("this letter after `bind` does nothing").

**Modifiers**, separated by spaces or `+`, case-insensitive: `SUPER` (also `MOD`, `LOGO`, `WIN`), `SHIFT`, `CTRL` (`CONTROL`), `ALT` (`MOD1`). An empty modifier field means none (used inside submaps).

**Keys**, case-insensitive: a single character; `Return`/`Enter`, `Escape`/`Esc`, `Tab`, `Space`, `Backspace`; `F1`..`F12`; `Up`, `Down`, `Left`, `Right`, `Home`, `End`, `PageUp`/`Prior`, `PageDown`/`Next`, `Insert`, `Delete`/`Del`; `minus`, `equal`, `comma`, `period`, `slash`, `semicolon`, `apostrophe`, `grave`, `bracketleft`, `bracketright`, `backslash`; the modifier keys themselves `Super_L`, `Super_R`, `Control_L`, `Control_R`, `Shift_L`, `Shift_R`, `Alt_L`, `Alt_R`; mouse buttons `mouse:272` and up, or `mouse_left`, `mouse_right`, `mouse_middle`.

**Dispatchers** (`binds.rs`):

| Dispatcher | Argument | Effect |
|---|---|---|
| `exec`, `execr` | command line (a path; no `PATH`, no shell) | start a program; `execr` is the same as `exec` |
| `killactive`, `closewindow` | — | ask the active window to close |
| `killwindow` | window match | close a named window |
| `workspace` | workspace argument | switch workspace |
| `movetoworkspace` | workspace argument | send the active window there and follow it |
| `movetoworkspacesilent` | workspace argument | send it there and stay |
| `movefocus`, `cyclenext` | `l`/`left`/`up`/`prev` for back, anything else forward | move the keyboard |
| `focuswindow`, `focuswindowbyclass` | window match | focus a named window |
| `focuscurrentorlast` | — | toggle to the previously focused window |
| `swapwindow` | direction | trade places with a neighbour |
| `swapnext` | — | trade places with the next window |
| `togglefloating` | — | float or tile |
| `setfloating`, `settiled` | — | float, or tile, unconditionally |
| `fullscreen` | — | toggle fullscreen |
| `pin` | — | toggle pinned (on every workspace) |
| `moveactive` | `dx dy` | move the active window |
| `resizeactive` | `dx dy` | resize the active window |
| `centerwindow` | — | centre a floating window |
| `splitratio` | `±0.05` | change the dwindle split |
| `layout`, `layoutmsg` | `master`, `dwindle`, `float`, or nothing to cycle | change layout |
| `movewindow` | — | with a mouse button: drag; with a key: `swapwindow` |
| `resizewindow` | — | with a mouse button: drag-resize; with a key: `resizeactive` |
| `movecursor` | `x y` | warp the pointer |
| `movecursortocorner` | 0 to 3 (anticlockwise from bottom-left) | warp the pointer to a corner |
| `submap` | name, or `reset` | change the active key map |
| `signal` | words | say them on `may_desktop` (for example `launcher toggle`, answered by aosh) |
| `reload`, `reloadconfig` | — | re-read the config |

At most 64 bindings in a config (`MAX_BINDS`) and 64 chords live in the compositor at once. A submap is bound *instead of* the default map when active (the default map's chords stay live), so a config can have more bindings than the compositor holds at once. At most 8 named submaps. A submap with no binding that leaves it is reported at startup, because the symptom is a keyboard that seems dead.

`unbind = MODS, KEY` removes a binding read earlier in the same submap.

Example, from the shipped `50-keys.conf`:

```
bind = $mod, Return, exec, /bin/mayterm
bind = $mod, D, signal, launcher toggle
bind = $mod SHIFT, R, reload
bindm = ALT, mouse:272, movewindow
bind = CTRL, 1, workspace, 1
bind = $mod SHIFT, TAB, workspace, e+1

bind = $mod, R, submap, resize
submap = resize
binde = , H, splitratio, -0.05
binde = , Left, resizeactive, -40 0
bind  = , Escape, submap, reset
submap = reset
```

Workspaces are on Ctrl rather than Super in the shipped config, because under a virtual machine the host window manager takes Super.

### Window rules

```
windowrule = EFFECT [ARG], class:PATTERN
windowrule = EFFECT [ARG], title:PATTERN
windowrule = EFFECT [ARG], floating:0|1
windowrule = EFFECT [ARG], PATTERN          (class assumed)
```

The pattern is a substring match, so `class:term` catches `mayterm`; `.*` matches every window and is the only pattern syntax. A rule applies once when a window is first seen; a `floating:` rule runs again whenever the window starts or stops floating. At most 32 rules.

| Effect | Argument | Meaning |
|---|---|---|
| `float`, `floating` | | float |
| `tile`, `tiled` | | tile |
| `fullscreen` | | fullscreen |
| `pin`, `pinned` | | on every workspace |
| `nofocus`, `noinitialfocus` | | never takes the keyboard |
| `center`, `centre` | | centre it |
| `size` | `W H` | initial size |
| `move` | `X Y` | initial position |
| `workspace` | `N` | open on workspace N |
| `opacity` | `A [I]`, each optionally followed by `override` | active and inactive opacity |
| `rounding` | `N` | this window's corner |
| `roundingpower`, `rounding_power` | `N` | this window's power |
| `noblur`, `no_blur` | | no blur for this window |
| `blur` | | effects on for this window |
| `noshadow`, `no_shadow` | | no shadow |
| `nodim`, `no_dim` | | not dimmed when inactive |

### Layer rules

```
layerrule = noanim, NAMESPACE
layerrule = noanim, namespace:NAMESPACE
```

`noanim` (or `no_anim`) is the only layer rule: layer surfaces with that namespace open and close with no compositor animation. At most 32. The shipped `60-rules.conf` gives `noanim` to `quickshell:overview`, `quickshell:actionCenter`, `quickshell:lockWindowPusher`, `quickshell:overlay` and `quickshell:polkit`.

### Animations and curves

```
animations { enabled = true }
bezier = NAME, X1, Y1, X2, Y2
animation = CLASS, ON, SPEED, CURVE[, STYLE]
```

- `CLASS` is one of the 15 classes listed under [Animations](#animations); an unknown class is ignored.
- `ON` is 0 or 1. `SPEED` is in hundreds of milliseconds (4 is 400 ms; clamped to 0 to 60 s).
- `CURVE` is a `bezier` name; an unknown name is curve 0. At most 16 curves and 16 animation lines.
- `STYLE` is `slide`, `fade` (also the default when omitted) or `popin N%`.

### exec-once

```
exec-once = /bin/program arg
```

Up to 8 lines (`MAX_EXEC`), each at most 64 bytes, run once after the desktop is up. `exec` is accepted as a synonym. The shipped desktop starts its programs from `session.d` instead.

### monitor

```
monitor = NAME, MODE, POSITION, SCALE[, vrr, 0|1][, bitdepth, 8|10]
monitor = NAME, disable
```

Hyprland's line (`lib/gfx/mayland/src/monitor.rs`). `NAME` is the connector as Linux names it (`DP-3`, `HDMI-A-1`, `Virtual-1`; `display` lists them), `desc:` followed by the start of the monitor's description, or empty for every other output. Rules are matched by name first, then description, then the catch-all.

| Field | Accepted |
|---|---|
| `MODE` | `preferred` (or empty), `highrr`, `highres`, `maxwidth`, or `WxH[@R]`; the closest of the monitor's own modes, width first, then height, then refresh; no `@` means 60 Hz |
| `POSITION` | `auto` or `XxY` (kept, not used: one output is composed) |
| `SCALE` | `1` or `auto` |
| `vrr` | 0 or 1 (2 and 3 are refused) |
| `bitdepth` | 8 or 10 |

Refused by name, with a reason: `transform`, `mirror`, `cm`, `sdrbrightness`, `sdrsaturation`, `icc`, `addreserved`, custom `modeline`s and scales other than 1. `disable` is read and reported as having no effect yet. At most 8 monitor lines. They are applied when the desktop first comes up, on reload, and when a monitor is plugged in; not on every change, so a `display mode` typed by hand stays until then. With no monitor line and no `misc:vrr`, outputs stay as the boot made them.

### What maywm says back

Every complaint names its file and line, at most 16 per load (then "...and more; the rest of the file is read in silence"):

```
  maywm: /etc/mayland/conf.d/99-mine.conf:4: this desktop has no such setting - gaps_inn
  maywm: /etc/mayland/conf.d/99-mine.conf:4: this setting - already set in /etc/mayland/conf.d/20-look.conf:7
  maywm: monitor DP-2: nothing is plugged into it - connected: DP-3
```

- Two files of the **same** layer setting one thing is reported; a higher layer overriding a lower one is silent.
- A section Hyprland has and this desktop does not is reported once, not once per line: `misc` (except `vrr`), `debug`, `cursor`, `render`, `group`, `gestures`, `binds`, `scrolling`, `xwayland`, `ecosystem`, `opengl`, `experimental`. So are the keywords `env`, `workspace`, `permission`, `plugin` and `exec-shutdown`.
- Grammar notes: unknown variable, line too long after expansion, too many variables, `source` too deep or unreadable, too many files, path too long, variable inside a variable.

These lines go to maywm's standard output, which is the console (visible in `dmesg` and on serial).

### The relation to Hyprland

The language is `hyprland.conf`'s shape, so lines can be copied from a Hyprland config. The shipped numbers (gaps, rounding 22 at power 2.5, 1 px translucent border, inactive opacity 0.9 and dim 0.05, curves, layer rules, no blur on windows, no shadow on tiled windows) come from the owner's Hyprland setup (v0.56.2, configured in Lua under `~/.config/hypr`), read on 2026-10-23. Differences: no `hyprctl` (reload is a key), no `env`, no plugins, one output composed, a two-deep block nesting limit, and the refusals listed above.

## mayterm, maygreet and maysession

### mayterm

`/bin/mayterm` (`system/src/mayterm`) is a terminal window that owns pixels, never text. The kernel keeps the grid, the line discipline and scrollback; mayterm reads the cell grid through `may_console` and draws it with `libmay::text` (the same glyph code the kernel uses).

- `mayterm` creates a pty (`miku::tty::open_pty`, i.e. `/dev/ptmx`), sets its window size with `TIOCSWINSZ`, binds it with `may_console.bind`, and forks and execs `/bin/msh` on it.
- `mayterm console` draws terminal zero, the system console, instead. That needs `CAP_MAY_PRIV`, so it belongs in `session.d`, not in the launcher (the shipped `session.d/10-desktop.conf` has it commented out).
- Keys do not go through mayterm. `input::intercept` in the kernel routes keystrokes to the terminal of the focused console window, so Ctrl-C, the line editor and job control work unchanged.
- The pointer selects text (drawn inverted); a right-click menu is a real popup (`menu.rs`). Copy goes out through `may_clipboard`; paste goes back through `may_console.paste`, because the text has to arrive as input.
- The window is ARGB with the background at 0.85 opacity.
- A frame does not end until the slot owes nothing, because a committed buffer must be whole; at most 8 rows go in the first declared run, and a debt that is most of the screen is paid in one frame.

See [input-and-terminals.md](input-and-terminals.md) for ptys and the console.

### maygreet

`/bin/maygreet` (`system/src/maygreet.rs`) is the login screen: one `OVERLAY` layer surface with an exclusive keyboard, which takes the keyboard lock described above. It runs as its own user with only `CAP_MAY_PRIV`; it cannot read `/etc/shadow` or start programs. It sends the user name and password to `mikudod` through `mikuauth::client::open_session`; mikudod checks the password, starts the session program and asks the kernel to open the session. When the session opens, the kernel moves `CAP_MAY_PRIV` to the session leader and maygreet exits. Its palette and portrait come from the `greeter` block over `theme`. Login, mikudod and sessions are described in [system.md](system.md).

### maysession

`/bin/maysession` (`system/src/maysession.rs`) is the program field seven of `/etc/passwd` names for a graphical user, so mikudod starts it as that user at login. It reads `/etc/mayland/session.d/*.conf` in name order (files add to each other):

```
session {
    start = /bin/maywm
    start = /bin/aosh
    start = /bin/mayterm
    # start = /bin/mayterm console
    main = /bin/maywm
}
```

Every `start` program is given `CAP_MAY_PRIV`. `main` names the program whose exit ends the session; without it the first `start` is main.

## Displays and modes

- **Before the console exists**, `init_framebuffer` picks the screen mode from the monitor's EDID (`kernel/drivers/gpu/edid.rs`), so the console is created at its final size. EDID detailed timings are ranked by most pixels, then highest refresh, not by order.
- **QEMU and Bochs** (`kernel/drivers/gpu/bochs.rs`): the Bochs DISPI interface, PCI 1234:1111, EDID at BAR2+0x000 and registers at BAR2+0x500 (or ports 0x01CE/0x01CF). In QEMU this changes the firmware's 800x600x24 to 1280x800x32 (QEMU's generated EDID, "QEMU Monitor"; `-device VGA,xres=,yres=` changes it). 32 bpp matters: at 24 bpp the compositor cannot copy an XRGB row in one go.
- **The NVIDIA GB206** has its own mode set using the same ranking, and picks the best connected monitor. See [../Nvidia_gb206.md](../Nvidia_gb206.md).
- **Anything else** keeps the firmware's framebuffer, as Linux's `simpledrm` does.
- **The display layer** (`kernel/drivers/gpu/kms`) presents every connector with Linux names. `display` lists outputs, monitors and modes (the chosen one starred); `display mode <output> <n>`, `display on|off <output>`, `display vrr on|off [<output>]` and `display color [<output>] k=v ...` change them, each checked before anything moves, with `test` at the end to check only. Mayland follows changes through `kms::events::listen(server::outputs_changed)`.
- **Multiple outputs.** maywm's `monitor =` lines choose each connected output's mode, VRR and bit depth through `may_output_manager`. The desktop is composed on one output; `POSITION` is kept but not used.

## Locks

| Lock | What it protects | Notes |
|---|---|---|
| `SERVER` (`server/state.rs`) | the whole display server: clients, surfaces, leases | One spin lock. Nothing holding it may call a function that takes it again; internal functions take `&mut Server`, and only entry points at the edges of `server/mod.rs` and the functions the input layer calls take the lock |
| `POOLS` (`server/shm.rs`) | the pool table | Taken under `SERVER` by `dispatch_one` → `shm::create` |
| `RETIRED` | destroyed pools' frames awaiting the engine | |
| `EFFECTS`, `BORDER` (`render/effects.rs`) | the desktop's effect numbers | |
| `PARAMS`, `KIND` (`shell/layout.rs`) | layout numbers and kind | |
| `input/grab.rs` | pointer drag and popup grab | one lock for both |

**Lock order: `SERVER`, then `POOLS`.** Whatever holds `POOLS` (for example inside `shm::with_bytes_mut`) must never take `SERVER`. The compositor once asked `server::active_key()` from inside `with_bytes_mut`; the ABBA deadlock froze about six logins in ten once spawns became fast (fixed 2026-10-27 with `compose_focused`, which reads the focus before taking the pools).

**Interrupt context.** The keyboard and mouse interrupts never take the server lock: they push into lock-free rings that the compositor thread drains once a frame. Input params (`input/conf.rs`) are atomics for the same reason. `console_pump` asks `io::console` everything first and only then talks to the client, so nothing calls into the console while holding a Mayland lock.

## The `mayland` command

`mayland` is a kernel command (`kernel/shell/commands/mayland_cmds.rs`). From msh it is reached through the kernel command channel; commands that change the machine need `mikudo`. See [commands.md](commands.md).

| Command | Effect |
|---|---|
| `mayland` or `mayland status` | Globals with versions, connections (pid, objects, queued bytes), surfaces bottom to top with commits and frame callbacks, focus and active window, presentables, wallpaper, pointer and keyboard state, compositor statistics (frames, copies, dropped, stalls, frame time, engine waits), layout and work area, borders, animations, effects-pass state (mapped client buffers, folded backdrop, stacked surfaces), window copies in VRAM, raster position |
| `mayland start` | Start compositing |
| `mayland stop` | Stop compositing and give the screen back to the kernel console; the watcher does not restart it until asked |
| `mayland wm` | Start `/bin/maywm` in the background (`wm off` says to use `ps`/`kill`) |
| `mayland term` | Start `/bin/mayterm` (needs compositing running) |
| `mayland shell` | Start `/bin/aosh` with the caller's user environment |
| `mayland signal <words>` | Say words on `may_desktop` (`mayland signal launcher toggle`); prints how many clients heard |
| `mayland workspace [n]` | Show or switch the workspace |
| `mayland layout [master\|dwindle\|float]` | Show or set the layout |
| `mayland move <c:s> <x> <y>` | Place a window |
| `mayland focus [next\|<client>:<surface>]` | Move the keyboard |
| `mayland effects [on\|off]` | Turn the effects pass on or off |
| `mayland effects alpha\|shadow <0-100>`, `corner\|margin\|radius <n>` | Set an effect number directly |
| `mayland input [on\|off]` | Give the keyboard to the focused window (Ctrl+Alt+Esc takes it back) |
| `mayland pointer <x> <y>` | Put the pointer somewhere as if the mouse moved |
| `mayland frame [ms]` | Fire frame callbacks as if a frame had been presented (for a machine with no scanout) |
| `mayland demo [on\|off]` | Connect an in-kernel client drawing a moving square |
| `mayland shm` | List shared-memory pools (handle, owner pid, size, physical address) |
| `mayland test` | Run the protocol selftest |

## The selftest

`kernel/mayland/selftest` drives both halves of the protocol against each other with nothing mocked: the client encoder produces bytes, the server consumes exactly those bytes, and the client decoder reads back what the server sent. It touches no hardware. There are 125 test functions filed by subject (`access`, `encoding`, `buffers`, `windows`, `tiling`, `wm`, `conf`, `console`, `layer`, `pointer`, `keyboard`, `anim`, `gfx`, `outputs`, `popup`, `clipboard`, `compose`, `below`, `toolkit`); the report counted 164 cases, all passing, on 2026-10-24.

The run switches the layout to float and parks already-open windows, then restores both. Cases that need `may_wm` or `may_console` are skipped rather than evicting the running desktop's holders.

Run it with `run=mayland_test` on the GRUB command line (results to serial), or with `mayland test` at a root console. See [testing.md](testing.md) for the harness. `run=mayland_start` brings compositing up from the boot line.

## Writing a client with libmay

A client is a ring-3 program built for the `x86_64-miku-app` target with `libmay` as a path dependency (`libmay = { path = "../lib/gfx/mayland" }`; add `features = ["userspace"]` if it reads config files through `manifest`). libmay is `core`-only and does not allocate. See [sdk.md](sdk.md) for building programs. `apps/src/maydemo.rs` is a complete, small example (a bouncing square with two buffers); the skeleton is:

```rust
use libmay::conn::{Conn, Event, Pool};
use libmay::protocol::{self, Interface};
use libmay::requests::Requests;

const W: i32 = 320;
const H: i32 = 140;
const STRIDE: i32 = W * 4;
const BUF: usize = (STRIDE * H) as usize;

let mut c = Conn::open().expect("no display server");
let (registry, globals) = c.handshake();          // registry + globals
let shm  = c.bind(registry, globals.name(Interface::Shm), Interface::Shm, 1);
let comp = c.bind(registry, globals.name(Interface::Compositor), Interface::Compositor, 1);
let xdg  = c.bind(registry, globals.name(Interface::Xdg), Interface::Xdg, 1);

// Pixels: one pool, two buffers
let pool = Pool::new(BUF * 2).expect("no pool");
let pool_obj = c.create_pool(shm, pool.handle, (BUF * 2) as i32);
let bufs = [
    c.create_buffer(pool_obj, 0, W, H, STRIDE, protocol::shm::FORMAT_XRGB8888),
    c.create_buffer(pool_obj, BUF as i32, W, H, STRIDE, protocol::shm::FORMAT_XRGB8888),
];

// A window: surface + xdg_surface + toplevel, then wait for the first configure
let surface = c.create_surface(comp);
let xdg_surface = c.get_xdg_surface(xdg, surface);
let toplevel = c.get_toplevel(xdg_surface);
c.set_title(toplevel, "hello");
c.flush();

let mut evs = [Event::Other { object: 0, opcode: 0 }; 32];
'cfg: loop {
    let n = c.read_events(&mut evs);
    for e in &evs[..n] {
        if let Event::XdgConfigure { serial, .. } = *e {
            c.ack_configure(xdg_surface, serial);
            break 'cfg;
        }
    }
}

// Draw into a buffer the compositor has released, then:
c.attach(surface, bufs[0], 0, 0);
c.damage(surface, 0, 0, W, H);
let frame_cb = c.frame(surface);   // Event::Done { id == frame_cb } = draw the next one
c.commit(surface);
c.flush();
// Event::BufferRelease { id } = that buffer may be drawn into again
```

Rules a client must keep:

- Draw only into a buffer that has been released, and commit only whole buffers.
- Do not commit before the first `ack_configure`.
- Redraw only when something changed; keep the frame-callback permission across frames you skip. Latch input as edges, not levels.
- Read `Event::Error` and print it; a protocol error closes the connection.
- `libmay::ui`, `libmay::paint` and `libmay::text` give an immediate-mode toolkit (buttons, fields, lists) for small programs; the desktop's own screens are QML in aosh.

## Performance notes

- A client that redraws and damages its whole window every frame costs the processor compositor a whole-window copy each frame. Measured on 2026-08-11: a 768x568 installer window cost 7 ms per frame idle; with redraw-on-change it fell to 0 rectangles and 31 µs.
- Frame cost and frame rate are separate numbers. Read `mayland status` twice a known number of seconds apart and divide the frame counter difference to get the rate.
- On the card, the processor must not read video memory: reading write-combining BAR1 memory pixel by pixel costs tens of milliseconds. Windows come from VRAM copies and the processor gets mirrors.
- The GB206 idles at PCIe Gen1 x8 (about 2 GB/s), so per-frame GPU reads of system memory were the animation stutter; window copies in VRAM fixed it. See [../Nvidia_gb206.md](../Nvidia_gb206.md).
- A full-screen see-through layer under the windows used to push every window onto the processor (7.5 ms frames at 240 Hz on 2026-10-20); the folded backdrop fixed it.
- A workspace switch's first frame went from 141 to 61 ms in QEMU when per-pixel soft-float fades and shadows in `compose.rs` became integer arithmetic (2026-10-24).
- A whole-bar CPU repaint costs about 58 µs; painting is not the bottleneck. Benchmark `paint.rs` natively rather than in TCG, where the in-guest frame counter spreads 16x.

## Diagnostics and troubleshooting

| Symptom | Check |
|---|---|
| Desktop up, a command printed nothing on screen | Expected: with compositing up and no mayterm, kernel text goes to the serial port only. `mayland status` says "nobody is drawing it". Start `mayterm` (`$mod Return`, the launcher, or `mayland term`) |
| Screen frozen, machine alive | `mayland stop` from a root console gives the screen back to the kernel console; a panic also repaints it (`render::emergency`) |
| Keyboard dead under a window | Ctrl+Alt+Esc (or F12) takes the keyboard back unless the login screen holds the lock |
| A config line does nothing | Look for `maywm:` lines on the console or in `dmesg`; every unrecognised line is named with its file and line |
| Keys stopped working after editing a submap | A submap with no way out is reported at startup; `$mod SHIFT R` re-reads the config if the default map is reachable |
| A window shows two alternating pictures | The client committed a half-painted buffer; see "A committed buffer must be whole" |
| A menu opens but its items do nothing | Pointer crossings after a surface arrives under a still pointer; `focus::repoint` |
| Blur, borders or fades wrong only on the card | QEMU has no compute pass; reason through `render/present.rs` and put it on the card checklist |
| Slow or stuttering frames | `dmesg debug` for the slow-frame lines; `mayland status` for frame time, engine waits and how many windows went to the processor |
| A client's pools refused | 16 per process, 64 system-wide; `mayland shm` lists them |
| Display server objected | maywm prints `the display server objected: <message> - object N code C` |

Boot flags that affect Mayland: `notex` (no VRAM window copies), `nohwcursor` (software cursor on the GB206), `noflipsema`, and `run=mayland_test` / `run=mayland_start`. The full list is in [boot.md](boot.md).

## Verification status

| Feature | Status |
|---|---|
| Protocol, tiling, layers, popups, clipboard, config (`mayland_test`) | 164/164 in QEMU, 2026-10-24 |
| Window shape (squircle, blended border, inactive opacity and dim) | QEMU-verified 2026-10-24 (processor path); the compute pass's version checked by a CUDA host harness; not yet run on the card |
| Layers above windows in the pass, layer rules, fadeLayers classes | QEMU-verified 2026-10-24; card unrun |
| Folded backdrop below the windows (xray) | Card 2026-10-20: smooth, but folded widgets were striped; caused by overlapping compute launches, fixed the same day; re-check on the card pending |
| Window buffers in video memory | Card 2026-10-20: a gpudemo window read in place, 0 Kpx uploaded; the close afterimage and the capture not yet seen on the card |
| Output manager and `monitor =` (K3) | Written and QEMU-verified 2026-10-01; not run on the card |
| Hardware cursor plane on the GB206 | Default since 2026-09-11 |
| Lock order fix (`compose_focused`) | QEMU, 2026-10-27 |

## Known gaps

- One output is composed; `POSITION` in a monitor line is kept but not used, and `monitor = NAME, disable` has no effect yet.
- No scaling, rotation (`TRANSFORM_NORMAL` only), mirroring or colour management through the protocol (`display color` sets the head's colour).
- Layer surfaces do not animate out.
- Only one layer rule (`noanim`).
- No `hyprctl`-style socket to maywm; reload is a key.
- No file descriptors over the socket; shared memory only. Pools are physically contiguous kernel memory, so their number and size are capped.
- Clipboard selections are capped at 64 KiB.
- No touchpad, no acceleration curve, no xkb (layouts are `us` and `ru`).
- `/etc/mayland/REFERENCE` is out of date in places: it describes a `panel` block nobody reads, says `layerrule` is unsupported, says borders ignore their alpha, and omits `signal`, `unbind`, `noshadow`, `nodim`, `floating:` rules and the newer animation classes.

## Code map

| Path | Contents |
|---|---|
| `kernel/mayland/mod.rs` | Layer overview, `compositor_thread`, `init` |
| `kernel/mayland/proto/` | `client.rs` (in-kernel client), `objects.rs`, `outq.rs` |
| `kernel/mayland/server/mod.rs` | Server entry points, `set_output`, `window_box` |
| `kernel/mayland/server/state.rs` | `Server`, `Client`, `Surface`, leases, `SERVER` |
| `kernel/mayland/server/dispatch.rs` | Bytes to handler calls |
| `kernel/mayland/server/iface/` | `display`, `compositor`, `buffers`, `xdg`, `seat`, `outputs`, `desktop`, `wm`, `console`, `layer`, `clipboard` |
| `kernel/mayland/server/access.rs` | Registry filter |
| `kernel/mayland/server/shm.rs` | Pools, `adopt`, `adopt_video`, retire/reap |
| `kernel/mayland/server/surface.rs`, `scene.rs`, `focus.rs`, `selection.rs` | Commit and configure, the flattened scene, focus and crossings, the selection |
| `kernel/mayland/shell/` | `layout.rs`, `workspace.rs`, `anim.rs`, `ghost.rs`, `wallpaper.rs`, `popup.rs`, `demo.rs` |
| `kernel/mayland/render/` | `mod.rs` (frame loop, start/stop), `present.rs`, `damage.rs`, `effects.rs`, `compose.rs`, `outline.rs`, `cursor.rs`, `texture.rs`, `base.rs`, `phases.rs` |
| `kernel/mayland/input/` | `mod.rs` (keyboard routing, chords, escape keys), `pointer.rs`, `grab.rs`, `conf.rs`, `switch.rs` |
| `kernel/mayland/selftest/` | The protocol selftest |
| `kernel/shell/commands/mayland_cmds.rs` | The `mayland` command |
| `kernel/shell/commands/display_cmds.rs` | The `display` command |
| `kernel/drivers/gpu/{edid,bochs,display,softscan,scanout}.rs`, `kms/` | Mode selection, Bochs DISPI, the software scanout, the display layer |
| `lib/gfx/mayland/src/` | `wire`, `protocol`, `requests`, `conn`, `keymap`, `conf`, `manifest`, `monitor`, `shape`, `qoi`, `font`, `paint`, `text`, `ui`, `ease`, `spawn` |
| `system/src/maywm/` | The window manager |
| `system/src/mayterm/` | The terminal (`grid.rs`, `size.rs`, `menu.rs`) |
| `system/src/maygreet.rs`, `system/src/maysession.rs` | Greeter and session starter |
| `apps/src/maydemo.rs`, `apps/src/gpudemo.rs` | Example clients |
| `etc/mayland/` | Shipped config |
