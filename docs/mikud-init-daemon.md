## mikuD Init Daemon

mikuD is the init daemon (PID 1) - a full systemd-like service supervisor with Unix-style boundaries. It manages service lifecycle, dependency resolution, targets (runlevels), watchdog, notifications, socket activation, timers, ELF binary execution (ExecStart), and graceful shutdown with global timeout.

### Targets (Runlevels)

| Target | Value | Description |
|---|---|---|
| **SysInit** | 0 | System initialization |
| **MultiUser** | 1 | Multi-user mode (default) |
| **Graphical** | 2 | Graphical mode |
| **Rescue** | 3 | Rescue / single-user mode |

### Service Types and Restart Policies

| Type | Description |
|---|---|
| **Simple** | Long-running service (default) |
| **Oneshot** | Execute once, then mark completed |
| **Notify** | Service reports readiness via `notify_ready()` |
| **Forking** | Service forks child process |

| Restart policy | Behavior |
|---|---|
| **Always** / **Never** | Always restart / never restart |
| **OnFailure** / **OnSuccess** | Only non-zero / only zero exit code |
| **OnAbnormal** | Restart on signal or non-zero exit |

### Features

| Feature | Details |
|---|---|
| **ExecStart** | Launch ELF binaries from disk as services |
| **Watchdog** | Service must ping within timeout or gets restarted |
| **Notify** | sd_notify analog - service signals readiness |
| **Conditions** | ConditionPathExists, ConditionServiceActive, ConditionTargetActive |
| **Masking** | Completely prevent a service from starting |
| **Critical** | Protected services cannot be stopped by user |
| **Burst protection** | Max 5 restarts per 10 sec window |
| **Graceful shutdown** | Stop non-critical first, then critical, 30 sec global timeout |
| **Journal** | 128-entry ring buffer with severity levels (info/notice/warning/critical) |
| **Timers** | Interval / Oneshot / Realtime (max 16) |
| **Socket activation** | Stream (TCP) and Dgram (UDP), max 16 sockets |

### Unit File Example (`/etc/mikud/*.service`)

```ini
[Unit]
Description=My service
After=kbd network
Wants=logging
ConditionPathExists=/etc/config

[Service]
Type=simple
ExecStart=/usr/bin/myservice
Restart=always
WatchdogSec=100

[Install]
WantedBy=multi-user
```

### `sv` Shell Commands

| Command | Description |
|---|---|
| `sv list` | List all services with state, PID, restarts |
| `sv status <name>` | Detailed status + journal entries |
| `sv start/stop/restart <name>` | Service lifecycle |
| `sv reload <name>` | Send SIGHUP for config reload |
| `sv enable/disable <name>` | Enable/disable service |
| `sv mask/unmask <name>` | Prevent/allow service startup |
| `sv force-stop <name>` | Force kill (even critical services) |
| `sv journal [name]` | Show event log (last 20 or per service) |
| `sv target [name]` | Show or set active target |
| `sv analyze` | Boot timing analysis |
| `sv tree/rdeps <name>` | Dependency visualization |
| `sv cat <name>` | Show service unit config |
| `sv load <path>` / `sv scan` | Load / scan unit files |
| `sv timer list/start/stop` | Control timer units |

---