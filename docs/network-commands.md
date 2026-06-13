## Network Commands

> Network is initialized automatically on boot. Supports Intel E1000, Realtek RTL8139/RTL8168, VirtIO Net.

### Basic

| Command | Syntax | Description |
|---|---|---|
| `dhcp` | `dhcp` | Request IP address via DHCP |
| `net` | `net [status]` | Show network adapter info: driver, link, MAC, IP, gateway, DNS, tx/rx counters |
| `net poll` | `net poll` | Receive and process pending packets |
| `net pci` | `net pci` | List detected PCI network cards |
| `net arp` | `net arp` | Show ARP table |
| `net dns` | `net dns [<ip>]` | Show or set DNS server |
| `net ip` | `net ip <ip> <gw> <mask>` | Set IP address manually |
| `net send` | `net send <ip> <port> <msg>` | Send a UDP packet |

### Diagnostics

| Command | Syntax | Description |
|---|---|---|
| `ping` | `ping <host or ip> [count]` | Send ICMP echo requests |
| `traceroute` | `traceroute <host or ip>` | Trace route to host |
| `tr` | `tr <host or ip>` | Alias for `traceroute` |

```bash
ping google.com
ping 8.8.8.8 5
traceroute google.com
```

### HTTP / HTTPS

| Command | Syntax | Description |
|---|---|---|
| `fetch` | `fetch <host or url> [port]` | Fetch a URL over HTTP or HTTPS (TLS 1.3) |
| `wget` | `wget <url> [-O <file>]` | Download a file |
| `curl` | `curl <url> [-X method] [-d data] [-o file] [-I]` | HTTP client |

```bash
fetch https://example.com
wget https://example.com/file.txt -O /file.txt
curl https://example.com -X GET
```

### Time

| Command | Syntax | Description |
|---|---|---|
| `ntp` | `ntp [server]` | Synchronize time via NTP |

```bash
ntp
ntp time.google.com
```

---