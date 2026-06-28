## /proc Virtual Filesystem

> Read-only virtual filesystem mounted at boot. Use `cat` to read files.

| File | Command | Description |
|---|---|---|
| `version` | `cat /proc/version` | OS name and version string |
| `uptime` | `cat /proc/uptime` | System uptime in ticks and seconds |
| `meminfo` | `cat /proc/meminfo` | Heap usage and VNode stats |
| `mounts` | `cat /proc/mounts` | Currently mounted filesystems |
| `cpuinfo` | `cat /proc/cpuinfo` | CPU architecture info |
| `stat` | `cat /proc/stat` | System statistics |
| `heap` | `cat /proc/heap` | Heap allocator detailed stats |

---