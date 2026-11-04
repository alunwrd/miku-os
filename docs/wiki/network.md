# Networking

MikuOS has an IPv4 network stack. The kernel owns the card, the demultiplexer, ARP, routing, ports and the socket table. TCP itself is a state machine in the `mikunet` crate (`lib/net`), and the kernel drives it. DNS, TLS, X.509, HTTP and NTP run in ring 3 in the same crate, so everything that parses bytes from the network can be tested on the build host with `cargo test`. Sockets are Linux-shaped: `AF_INET` stream, datagram and raw ICMP sockets, `AF_UNIX` stream sockets with peer credentials and descriptor passing, and `AF_MAYLAND` for the display server. This page describes each layer, the drivers, `netd`, the network programs, testing under QEMU, measured numbers and the gaps.

## Contents

- [Layers and code map](#layers-and-code-map)
- [NIC drivers](#nic-drivers)
- [Interrupts and polling](#interrupts-and-polling)
- [The receive path: rx::pump and the demux](#the-receive-path-rxpump-and-the-demux)
- [Ethernet and ARP](#ethernet-and-arp)
- [IPv4 and routing](#ipv4-and-routing)
- [ICMP](#icmp)
- [UDP](#udp)
- [TCP](#tcp)
- [Sockets](#sockets)
- [DHCP and netd](#dhcp-and-netd)
- [DNS](#dns)
- [NTP](#ntp)
- [TLS and certificates](#tls-and-certificates)
- [HTTP](#http)
- [Raw frames: /dev/net0 and net=raw](#raw-frames-devnet0-and-netraw)
- [Commands and programs](#commands-and-programs)
- [QEMU setups](#qemu-setups)
- [Tests](#tests)
- [Measured numbers](#measured-numbers)
- [Limits and known gaps](#limits-and-known-gaps)
- [Troubleshooting](#troubleshooting)

---

## Layers and code map

```
  ring 3        std::net / libc sockets / mikunet: DNS, TLS, X.509, HTTP/1.1, HTTP/2, NTP
  ------------------------------------------------- syscalls 56-66, 77, 80, 83-85, 114
  kernel/net/socket.rs   the socket table: TCP, UDP, ICMP, AF_UNIX, Mayland
  kernel/net/tcp.rs      TcpSocket: a blocking API over a mikunet Connection
  kernel/net/tcp_listener.rs
  ------------------------------------------------- on_segment / on_tick
  lib/net/src/stack      Connection (TCP state machine), wire formats; host-tested
  ------------------------------------------------- rx::pump
  kernel/net/rx.rs       the demux: one drain of the card, a queue per consumer
  kernel/net/mod.rs      NET (driver, addresses, ARP), tx_frame, routing, netd
  kernel/net/irq.rs      MSI/MSI-X wakeups
  ------------------------------------------------- trait NetworkDriver
  kernel/drivers/net     e1000, virtio-net, rtl8168, rtl8139
```

| Path | What |
|:--|:--|
| [`kernel/net/mod.rs`](../../kernel/net/mod.rs) | `NET` state (driver, MAC, IP, gateway, mask, DNS, ARP table, counters), `init` (driver probe), `tx_frame`, `udp_send_resolved`, `ip_send_ttl`, `resolve_arp`, `ephemeral_port`, `netd_thread`, the `net` and `dhcp` console commands |
| [`kernel/net/rx.rs`](../../kernel/net/rx.rs) | The receive demultiplexer |
| [`kernel/net/irq.rs`](../../kernel/net/irq.rs) | Card interrupts and netd's wait |
| [`kernel/net/socket.rs`](../../kernel/net/socket.rs) | Socket table and every socket operation |
| [`kernel/net/tcp.rs`](../../kernel/net/tcp.rs), [`tcp_listener.rs`](../../kernel/net/tcp_listener.rs) | TCP connections and listeners as the kernel drives them |
| [`kernel/net/dhcp.rs`](../../kernel/net/dhcp.rs) | The DHCP client |
| [`kernel/net/raw.rs`](../../kernel/net/raw.rs) | `/dev/net0` |
| [`kernel/net/selftest.rs`](../../kernel/net/selftest.rs) | `net test` / `run=net_test` |
| [`kernel/syscall/adapters/net.rs`](../../kernel/syscall/adapters/net.rs) | Socket system calls |
| [`kernel/drivers/net/`](../../kernel/drivers/net/) | The NIC drivers |
| [`lib/net/src/stack/`](../../lib/net/src/stack/) | `conn.rs` (TCP), `tcp.rs`, `udp.rs`, `eth.rs`, `arp.rs`, `dhcp.rs` wire formats |
| [`lib/net/src/`](../../lib/net/src/) | `ipv4.rs`, `icmp.rs`, `dns.rs`, `dns_wire.rs`, `ntp.rs`, `tls.rs`, `x509.rs`, `der.rs`, `verify.rs`, `crypto.rs`, `gcm.rs`, `rsa.rs`, `ec.rs`, `bignum.rs`, `http/`, `http2.rs`, `sock.rs` |
| [`lib/libmiku/c/netdb.rs`](../../lib/libmiku/c/netdb.rs) | `getaddrinfo` |
| [`apps/src/`](../../apps/src/) | `ping`, `traceroute`, `wget`, `curl`, `host`, `ntpdate` |
| [`etc/ssl/certs/ca-bundle.pem`](../../etc/ssl/certs/ca-bundle.pem) | The trust store |

Design note: [`dev/net-architecture.md`](../../dev/net-architecture.md). The `Connection` emits segments rather than sending them, which is what lets host tests put two connections face to face with a clock that is a variable.

---

## NIC drivers

`net::init` takes the first network adapter on the PCI bus (`pci::find_nic`) and matches its IDs:

| Driver | PCI IDs | Interrupt | Notes |
|:--|:--|:--|:--|
| e1000 (`e1000.rs`) | 8086:100E (82540EM, QEMU's default), 8086:100F (82545EM), 8086:10D3 (82574L, QEMU's `e1000e`), 8086:1502 (82579LM), 8086:153A (I217) | MSI when the function has it (the 82574L does; the 82540EM does not) | Interrupt throttling at about 130 us (at most ~7600 interrupts a second); ICR read-to-clear in the handler |
| virtio-net (`virtio_net.rs`) | 1AF4:1000 (legacy/transitional) | MSI-X when the device has vectors (an entry for the receive queue) | Legacy port-I/O transport; the vring layout is sized from the device's queue size. The modern-only ID 1AF4:1041 is not matched |
| rtl8168 (`rtl8168.rs`) | 10EC:8168 (RTL8111/8118/8168) | MSI | A port of Linux's `r8169` bring-up for the 8168G/H generations (PHY through GPHY OCP, `RXDV_GATED_EN`, the ERI window); written for the RTL8118AS on a Gigabyte B550M K |
| rtl8139 (`rtl8139.rs`) | 10EC:8139, and 10EC:8169 is routed to it as well | none (polled) | Stops after a few packets in QEMU; it did before the 2026-10-27 heap change too |

One adapter is used; a second is ignored. A driver implements `NetworkDriver` (`kernel/net/mod.rs`): `send`, `recv(handler)`, `has_packet`, `link_up`, `get_mac`, `diag`, `enable_irq`. The rtl8139 and rtl8168 need receive memory below 4 GiB (`DmaBox::new_below`, see [memory.md](memory.md)).

---

## Interrupts and polling

[`kernel/net/irq.rs`](../../kernel/net/irq.rs). **Only MSI or MSI-X** (through `drivers/bus/msi.rs`, as NVMe and the GPU use). Legacy INTx is not done: routing a PCI pin to an IO-APIC input needs the ACPI `_PRT`, which needs an AML interpreter this kernel does not have.

- **The handler** acknowledges the card's own cause register where the card wants that (e1000e's ICR, rtl8168's ISR) through an MMIO base handed over at set-up, marks a frame pending, and wakes `irq::RX`. It takes no lock: `NET` is a spin lock thread code holds with interrupts on.
- **netd** waits on `irq::RX` between pumps. With an interrupting card its backstop is 20 ms (short enough for the retransmission timers that run when waiters wake); without one it is one tick (4 ms), so a card with no MSI is pumped 250 times a second.
- **A pump that cannot drain says so** (fixed 2026-10-28): a pump that finds another one running sets `AGAIN` and the running one goes round once more; a full batch (`RX_BATCH = 64`) goes round again; `NET` held re-arms netd (`irq::rearm`) instead of spinning on a lock the caller may hold. Before this, a SYN-ACK arriving while the connecting thread held `NET` waited netd's 20 ms backstop.
- `[net] <driver> is polled every tick` in the boot log means no MSI was set up.
- **`netpoll`** on the boot line keeps every card polled.

---

## The receive path: rx::pump and the demux

[`kernel/net/rx.rs`](../../kernel/net/rx.rs). **Exactly one place drains the card**: `rx::pump()`. Any thread may call it at any time; a reentrancy guard makes concurrent calls cheap. Each frame is parsed once and routed:

| Frame | Goes to |
|:--|:--|
| ARP | Handled here: table update, and a reply when it asks for our address |
| ICMP echo request | Answered here |
| Other ICMP | Every registered ICMP waiter (ping, traceroute, raw sockets), up to 8 |
| UDP | The queue of the bound port (DHCP, DNS, NTP, user sockets) |
| TCP on a known 4-tuple | That connection's segment queue |
| TCP SYN to a listening port | That listener's backlog |
| TCP on nothing | A RST back (RFC 793 3.4), unless it was a RST |

| Table | Limit | Queue |
|:--|:--|:--|
| TCP connections | 1024 | 256 KiB of payload plus 64 KiB of margin, at most 1024 segments |
| Listeners | 64 | 64 SYNs |
| UDP ports | 256 | 256 datagrams |
| ICMP waiters | 8 | |

The tables are vectors that grow to these limits and reuse free slots. A full queue drops the newest arrival (the sender's retransmission recovers it).

- **Wakes**: a queued TCP segment wakes its connection's key (`sched::wait::key::net_tcp`), a SYN its listener's (`net_listen`), a datagram its port's (`net_udp`), an ICMP message its waiter's (`net_icmp`), and `poll` waiters are woken too ([scheduler.md](scheduler.md#wait-queues-and-lost-wakes)). Readers pump the card themselves when they wake, so a reader never waits for netd to be scheduled.
- **Locking**: `pump` holds `NET` only while draining the driver or sending a reply, never while a registry lock is held, so consumers that send with `NET` locked cannot deadlock against dispatch.
- **Drop counters** (`net status`, `net diag`): bad checksum, fragment, wrong address.

---

## Ethernet and ARP

- Ethernet II frames, EtherTypes IPv4 (0x0800) and ARP (0x0806); 14-byte header; no VLANs.
- **ARP table**: 16 entries (`ARP_TABLE_SIZE`), each valid for 60 s (`ENTRY_TTL_MS`). Every ARP request or reply seen updates it.
- **Resolution** (`resolve_arp`): a cached entry, or up to 5 requests, each waited on for 2 s (500 ticks) while pumping, then failure. Ctrl-C at the console aborts it.
- No gratuitous ARP, no ARP probing of the leased address.

---

## IPv4 and routing

`lib/net/src/ipv4.rs`, sent from `kernel/net/mod.rs`.

- **Routing** is one rule: a destination in our subnet (`is_same_subnet` with the mask) is ARPed directly, anything else goes to the gateway's MAC. `255.255.255.255` goes to the broadcast MAC. There is no routing table, no second interface and **no loopback interface**: `127.0.0.1` is not on the local subnet and is sent to the gateway like any other address.
- **TTL**: 64 by default (`DEFAULT_TTL`); `IP_TTL` per socket (what `traceroute` sets).
- **Checksums** are verified on every packet; a bad one is dropped and counted.
- **Fragments are dropped** (counted): there is no reassembly, and no fragmentation on send. TCP keeps segments under 1460 bytes of payload (we send at most 1400, `MAX_SEG`); UDP payloads are at most 1472 bytes.
- **IPv6 is not implemented** anywhere: not in the kernel, `std` refuses it by name, and `getaddrinfo` answers `EAI_FAMILY` for `AF_INET6`.

---

## ICMP

- **Echo requests** to our address are answered by the demux (so the machine can be pinged with no program running).
- **Echo replies, time exceeded and destination unreachable** go to the registered ICMP waiters.
- **Raw ICMP sockets** (`socket(AF_INET, SOCK_RAW, IPPROTO_ICMP)`) are unprivileged, like Linux's ping sockets: the kernel writes the IP header, so the source address cannot be forged; the program writes the ICMP message. Raw sockets with any other protocol are refused (`EPROTONOSUPPORT`).
- No ICMP is sent for a datagram to an unbound port.

---

## UDP

- `bind(port)` claims a port; a socket that sends first is bound to an ephemeral port automatically. **`bind` to port 0 is refused** (`EINVAL`).
- `connect` on a datagram socket pins a default peer for `send`/`recv`.
- `sendto`/`recvfrom` carry an IPv4 address and port.
- A datagram to a port nobody bound is dropped silently.
- A zero checksum (sender chose not to compute one) is accepted; a wrong one is dropped.
- Broadcast to `255.255.255.255` is allowed (DHCP uses it).

**Ephemeral ports** (`net::ephemeral_port`, TCP and UDP): 49152-65535, handed out in turn from a random start (RFC 6056 3.3.2); a caller skips one in use. It used to be 49152 plus the tick count, so sockets opened within one tick shared a port and a third of HTTP GETs hung on a server's TIME_WAIT (fixed 2026-10-28).

---

## TCP

### Where it lives

The protocol is `mikunet::stack::conn::Connection` ([`lib/net/src/stack/conn.rs`](../../lib/net/src/stack/conn.rs)): a state machine that takes segments and a time (`on_segment`, `on_tick`) and emits segments. The kernel's `TcpSocket` ([`kernel/net/tcp.rs`](../../kernel/net/tcp.rs)) provides the blocking API, the Ethernet and IP framing, the ephemeral port (drawn against the demux so two processes never get the same one), and the initial sequence number from `kcore::random` (RDRAND where the CPU has it; the crate's own generator is deterministic for tests).

### States

`Closed`, `SynSent`, `SynReceived`, `Established`, `FinWait1`, `FinWait2`, `CloseWait`, `LastAck` (`TcpState`); listening is a `TcpListener`. **There is no `TimeWait` and no `Closing` state**: a FIN received in `FinWait1`/`FinWait2` goes straight to `Closed`.

### Behaviour

| Feature | Detail |
|:--|:--|
| MSS | We advertise 1460 (`OUR_MSS`) and send at most 1400 bytes a segment; a peer that advertises none is assumed to take 536 |
| Buffers | 256 KiB receive (`RX_CAP`) and 256 KiB send (`TX_CAP`) |
| Window scaling (RFC 7323) | Our SYN and SYN-ACK offer a shift (3, for 256 KiB); the peer's shift applies when both sides sent one; a SYN's own window is never scaled |
| SACK (RFC 2018) | SACK-permitted on the SYN. As a receiver, every ACK sent while out-of-order data is held carries up to 3 blocks, most recent first. As a sender, SACKed runs are not resent in recovery or after an RTO |
| Out-of-order data | Held by bytes, sorted and merged, bounded by the receive buffer's free space |
| Congestion control | NewReno (RFC 5681, 6582): initial window 10 segments (RFC 6928), slow start, congestion avoidance, fast retransmit and fast recovery on three duplicate ACKs (a partial ACK resends the next hole), RTO halves `ssthresh` and drops `cwnd` to one segment. In flight: `min(cwnd, peer window)` |
| Retransmission timer | RFC 6298: 1 s before the first measurement, 250 ms floor, 60 s ceiling; Karn's rule (no sample from a resent segment) |
| Limits | 5 retransmissions of one segment (`MAX_RETRIES`) end the connection; 3 SYNs (`MAX_SYN_RETRIES`) end a `connect` |
| Keepalive | `SO_KEEPALIVE`, `TCP_KEEPIDLE`, `TCP_KEEPINTVL`, `TCP_KEEPCNT`; off by default, with Linux's numbers ready: 2 h idle, 9 probes 75 s apart |
| ACKs | Every segment is acknowledged (no delayed ACK) |
| Sending | Writes are segmented and sent as they come (no Nagle; there is no `TCP_NODELAY` option) |
| Timestamps | Not implemented (no PAWS/RTTM) |
| Options | The options of every segment reach the connection (SACK needs them) |

### Waiting and timeouts

Every TCP wait is a `wait_unless` on the connection's key with the network backstop (20 ms with an interrupting card, a tick without), so a segment between the check and the sleep is never lost ([scheduler.md](scheduler.md)). When a waiter wakes it pumps the card and runs the connection's timers.

| Wait | Limit |
|:--|:--|
| `connect` handshake | 3 SYNs; the socket layer gives up after 30 s (`OP_TIMEOUT_TICKS`) |
| `accept` | 30 s, then `EAGAIN` |
| Blocking `recv` | 30 s with no data, then `EAGAIN` (0 means only that the peer closed) |
| `close` | Up to 2 s for the peer to finish its half; 500 ms once our own FIN is acknowledged |

Non-blocking sockets (`O_NONBLOCK` via `fcntl` or the socket flags): `connect` answers `EINPROGRESS`, the socket turns writable when the handshake ends, and a failure waits in `SO_ERROR`. `recv` answers `EAGAIN` when nothing is queued.

Fixed 2026-10-28: `TcpSocket::close` waited one more backstop after the peer's FIN had closed the connection (20 ms per close).

---

## Sockets

### Families and types

| `socket(domain, type, protocol)` | |
|:--|:--|
| `AF_INET` (2), `SOCK_STREAM` (1), 0 or `IPPROTO_TCP` | TCP |
| `AF_INET`, `SOCK_DGRAM` (2), 0 or `IPPROTO_UDP` | UDP |
| `AF_INET`, `SOCK_RAW` (3), `IPPROTO_ICMP` | ICMP |
| `AF_UNIX` (1), `SOCK_STREAM` | Local stream socket (`SOCK_DGRAM` is `EPROTONOSUPPORT`) |
| `AF_MAYLAND` (42) | A connection to the display server, connected on creation, at most 4 per process ([mayland.md](mayland.md)) |

Anything else is `EAFNOSUPPORT` or `EPROTONOSUPPORT`.

### The table

- **256 sockets on the machine** (`MAX_SOCKETS`), at most 160 per user. Socket descriptors live in their own range starting at 4096 (`SOCK_FD_BASE`), so `read`, `write`, `close`, `poll`, `fcntl` and `dup` route them by number alone ([vfs.md](vfs.md#file-descriptors)).
- Sockets belong to a process: they are closed when it exits (`close_all_for_pid`), and close-on-exec ones at `execve`.
- The table lock is never held across a blocking call: an operation takes the socket out of its slot (marked busy), blocks, and puts it back.

### System calls

| No. | Call | Notes |
|:--|:--|:--|
| 56 | `socket(domain, type, protocol)` | |
| 57 | `connect(fd, addr, len)` | `sockaddr_in`, or `sockaddr_un` (path up to 108 bytes) |
| 58, 59 | `send(fd, buf, len, flags)`, `recv(...)` | Flags are ignored |
| 62 | `bind(fd, addr, len)` | Port 0 refused; an `AF_UNIX` bind creates a socket node in the VFS at that path (`EADDRINUSE` if the name exists) |
| 63 | `listen(fd, backlog)` | Backlog argument ignored (64 per listener) |
| 64 | `accept(fd, addr, len)` | Writes the peer's address |
| 65, 66 | `sendto`, `recvfrom` | |
| 77 | `socketpair(fds)` | `AF_UNIX` stream pairs only |
| 80 | `setsockopt(fd, level, name, value)` | The value is passed by register |
| 114 | `getsockopt(fd, level, name)` | The value is the return |
| 83 | `peercred(fd, out)` | `AF_UNIX`: `{pid, start_tick, uid, gid, euid, egid}` (40 bytes) of the peer **as it was when the connection was made** |
| 84, 85 | `sendfd(sockfd, fd)`, `recvfd(sockfd)` | Pass one open descriptor over a connected `AF_UNIX` socket; at most 8 in flight (`MAX_INFLIGHT_FDS`) |

`read`/`write`/`close` work on sockets too. There are **no** `shutdown`, `getsockname` or `getpeername` system calls (libc answers `EOPNOTSUPP`), no `sendmsg`/`recvmsg` (descriptor passing is `sendfd`/`recvfd`, which libc's `SCM_RIGHTS` path is not), and no `SO_REUSEADDR`. The full ABI is in [MikuOS_ABI.md](../MikuOS_ABI.md).

### Socket options

| Level | Option | set | get |
|:--|:--|:--|:--|
| `SOL_IP` (0) | `IP_TTL` (2) | yes | yes |
| `SOL_SOCKET` (1) | `SO_KEEPALIVE` (9) | yes (TCP) | yes |
| `IPPROTO_TCP` (6) | `TCP_KEEPIDLE` (4), `TCP_KEEPINTVL` (5) seconds, `TCP_KEEPCNT` (6) | yes | yes |
| `SOL_SOCKET` | `SO_ERROR` (4) | | yes (reads and clears the pending error) |
| `SOL_SOCKET` | `SO_TYPE` (3), `SO_ACCEPTCONN` (30) | | yes |

Anything else: `setsockopt` answers `EINVAL`, `getsockopt` `ENOPROTOOPT`.

### AF_UNIX

- Stream only; a bound name is a socket node in the VFS, and a connection finds the listener by that node (two names for one node reach one listener; a renamed listener is still the same one). The node is unlinked when the socket goes.
- Each direction buffers 64 KiB (`UNIX_BUF_MAX`).
- `peercred` is the basis of authentication for local services: `mikudod` never trusts a uid in a request, it asks the kernel ([system.md](system.md)). Compare both pid and start tick, since pids are reused.
- `connect` and `accept` waits are lost-wake free (`wait_unless`).

### poll and select

`poll` (76, milliseconds) and `poll_ns` (123, nanoseconds) watch up to 256 descriptors of any kind, sockets included; each socket's readiness comes from `socket::poll_fd`. libc's `select` is built on `poll` (readable = `POLLIN`, hangup or error; writable = `POLLOUT` or error; the exception set is never marked). **There is no `epoll`**; `mio`, `polling`, `smol` and `tokio` use `poll` here ([sdk.md](sdk.md)).

### From Rust std

`std::net` supports IPv4 `TcpStream`, `TcpListener` and `UdpSocket`, non-blocking mode, `try_clone`, `take_error`. Refused by name: IPv6, binding port 0, `shutdown`, `peek`, read/write timeouts, linger, `set_keepalive` through std, and `socket_addr` of a socket made from a raw descriptor (there is no `getsockname`). `lookup_host` goes through `getaddrinfo`.

---

## DHCP and netd

**netd** is a mikuD service (`netd_thread`, priority 1, restarted on failure):

1. With no card, or with the card handed to `/dev/net0` (`net=raw`), it goes dormant.
2. It waits up to 3 s for link (`link_up`), then tries DHCP anyway.
3. DHCP (`kernel/net/dhcp.rs`, wire format in `lib/net/src/stack/dhcp.rs`): DISCOVER (up to 4), OFFER, REQUEST, ACK, from port 68 to 67, asking for subnet mask, router, DNS server and lease time. Three attempts, 2 s apart.
4. On a lease it sets IP, gateway, mask and DNS (`set_ip`, `set_dns`) and logs `[netd] DHCP lease: ip=... gw=... dns=... lease=...s`.
5. It renews at half the lease (RFC 2131 T1; one hour assumed when the server gives none). With no lease it retries in 5 minutes.
6. Between renewals it pumps the card on every wake, which keeps ARP answered and TCP queued when no thread is in a socket call.

**There is no configuration file.** Static settings are console commands: `net ip <ip> <gw> <mask>` and `net dns <ip>`; `dhcp` runs DHCP now. netd starts only after the boot line's `run=` commands finish, so a boot-line test that needs the network runs `dhcp` first ([testing.md](testing.md)).

`/proc/net` publishes what programs need: `link:`, `ip:`, `dns:`, `mac:`, `owner:` (`kernel` or `/dev/net0`), and the raw counters when the card is handed out ([diagnostics.md](diagnostics.md)).

---

## DNS

Two resolvers, one wire format (`lib/net/src/dns_wire.rs`, A queries, CNAME chains followed in the answer, truncated answers refused):

| | `getaddrinfo` (libc_miku) | `mikunet::dns` |
|:--|:--|:--|
| Used by | `std` (`lookup_host`, `TcpStream::connect("name:port")`), tokio, C programs | `wget`, `curl`, `host`, `ntpdate` and other mikunet programs |
| Order | 1. address literal; 2. `localhost` and `*.localhost` (RFC 6761); 3. `/etc/hosts`; 4. DNS | DNS (with a 16-entry cache per program) |
| Server | The `dns:` line of `/proc/net` (from the DHCP lease); `EAI_AGAIN` while `ip:` is still 0.0.0.0 | Same (`system_resolver`) |
| Attempts | 3 x 2 s | 4 x 3 s |

IPv4 only: `AF_INET6` is `EAI_FAMILY`, `AF_UNSPEC` gets IPv4 answers. Service names: port numbers plus a handful of built-in names (no `/etc/services`). No `/etc/resolv.conf`. The `/etc/hosts` path is untested on the machine, because `/etc` is read-only in the test image. `host <name> [server]` queries directly.

---

## NTP

`lib/net/src/ntp.rs`; the program `ntpdate [server] [-q]`. The default server is `216.239.35.0` (time.google.com, an address so it works before DNS). `-q` only reports; setting the clock is `clock_settime(CLOCK_REALTIME)`, which needs `CAP_SYS_TIME`, so run it as `mikudo ntpdate`. The RTC sets the clock at boot; NTP refines it. See [scheduler.md](scheduler.md#time-and-clocks).

---

## TLS and certificates

TLS runs **in ring 3**, in `mikunet` ([`lib/net/src/tls.rs`](../../lib/net/src/tls.rs)), since 2026-08-25.

**Versions and cipher suites offered** (client only, no server side):

| | Suites |
|:--|:--|
| TLS 1.3 | `TLS_AES_128_GCM_SHA256` |
| TLS 1.2 | `ECDHE-RSA-AES128-SHA256` (0xC027), `ECDHE-RSA-AES128-SHA` (0xC013), `RSA-AES128-SHA256` (0x003C), `RSA-AES128-SHA` (0x002F) |

- Key exchange: ECDHE on P-256 (secp256r1) only.
- Signatures accepted: RSA PKCS#1 v1.5 (SHA-1/256/384/512), RSA-PSS (SHA-256/384/512), ECDSA (SHA-1/256/384/512); PKCS#1 and PSS are not confused with each other.
- ALPN offers `h2` and `http/1.1`.
- All randomness (client random, ECDHE keys, padding) comes from the platform entropy source; a failure aborts the handshake rather than falling back.

**Certificate validation** ([`lib/net/src/verify.rs`](../../lib/net/src/verify.rs)), in order:

1. the leaf names the host asked for (subjectAltName, wildcards; the CN only when there is no SAN);
2. every certificate is inside its validity window;
3. each signature verifies under its issuer's key;
4. every issuer is a CA allowed to sign certificates (basic constraints, path length);
5. the top of the chain is signed by a key from the **trust store**, not one that arrived with the connection.

**The trust store** is a file: `/etc/ssl/certs/ca-bundle.pem`, the Mozilla CA list (121 roots), staged from the repository's `etc/` into the read-only boot image. A `Trust` is constructed and handed to every connection (`TlsStream::connect(host, ip, port, &Trust)`), so there is no global that might be uninitialised. **No store means no connection**: a missing or empty file gives an empty store, an empty store anchors nothing, and the ordinary path refuses the chain. `--insecure` (on `wget` and `curl`) is the only way past and warns on every connection. To trust a server on the bench, add its root to the file.

If the clock was never set, validity dates are not checked and that is logged (`tls: clock not set - certificate validity dates not checked`). There is no revocation checking (no OCSP, no CRL).

---

## HTTP

`lib/net/src/http/` and `http2.rs`: HTTP/1.1 (header parser and chunked decoder in `wire.rs`, host-tested) and HTTP/2 when ALPN selects `h2`; up to five redirects (301, 302, 303, 307, 308); an idle timeout. Used by `wget` and `curl`.

---

## Raw frames: /dev/net0 and net=raw

For a network stack that lives in a program. `net=raw` on the boot line hands the card to `/dev/net0` ([`kernel/net/raw.rs`](../../kernel/net/raw.rs)): the in-kernel stack and netd stand down, `rx::pump` still drains the card once but delivers whole frames to the device instead of the demux, and a write sends one whole frame. 64 frames are held for the reader (`BACKLOG`); older ones are dropped. Frames up to 2048 bytes. Opening it needs root and `CAP_NET_RAW`. `/proc/net` counts reads, writes, drops and frames refused for an undersized buffer.

---

## Commands and programs

**Console commands** (kernel shell; from ring 3 through `mikudo`):

| Command | What |
|:--|:--|
| `net` / `net status` | Driver, link, IP, gateway, mask, DNS, MAC, frames sent and received, drop counters |
| `net ip <ip> <gw> <mask>` | Set the address by hand |
| `net dns [<ip>]` | Show or set the DNS server |
| `net arp` | The ARP table |
| `net pci` | Network devices on the bus |
| `net diag` | The driver's own diagnostics |
| `net send ...` | Send a test datagram |
| `net poll` | Pump the card once |
| `net test` | The network selftest |
| `dhcp` | Run DHCP now and print the lease |

**Programs in `/bin`** ([`apps/src/`](../../apps/src/)):

| Program | Syntax | Example |
|:--|:--|:--|
| `ping` | `ping <host|address> [count]` | `ping 10.0.2.2 4` |
| `traceroute` | `traceroute <host|address> [max-hops]` | `traceroute example.com 20` |
| `host` | `host <name> [server]` | `host zed.dev 1.1.1.1` |
| `wget` | `wget <url> [-O <file>] [--insecure] [-v]` | `wget https://example.com/ -O /tmp/index.html` |
| `curl` | `curl <url> [-X GET|POST] [-d <data>] [-o <file>] [-I] [--insecure] [-v]` | `curl -I https://example.com/` |
| `ntpdate` | `ntpdate [server] [-q]` | `mikudo ntpdate` |

`ping` and `traceroute` use raw ICMP sockets and need no privilege. The full program reference is in [commands.md](commands.md).

---

## QEMU setups

| Setup | Card | How it is driven |
|:--|:--|:--|
| `-netdev user,id=n0 -device virtio-net-pci,netdev=n0` | virtio-net with MSI-X vectors | interrupts (the recommended setup, as in README) |
| `-nic user,model=virtio-net-pci` | virtio-net **without** MSI-X vectors (no BAR1) | polled every tick |
| QEMU default / `-nic user` | e1000 82540EM, no MSI | polled every tick; the selftest suites' default |
| `-nic user,model=e1000e` | 82574L with MSI | interrupts |

- **User networking (slirp)**: the guest is `10.0.2.15`, the gateway and host `10.0.2.2`, DNS `10.0.2.3`.
- **The e1000's 500 ms connect is QEMU's, not the stack's.** Under slirp a TCP connect through the e1000 model costs a flat ~500 ms between our SYN leaving and the SYN-ACK arriving: the e1000's transmit runs slirp on a vCPU thread, and QEMU's main loop polls the new host socket only when its own timeout ends. virtio-net on the same slirp answers in about 0.1 ms. **Measure on virtio-net built with `-device`.**
- **Packet capture**: `-object filter-dump,id=f1,netdev=n0,file=/tmp/x.pcap` and open the file in Wireshark; it shows whose side a gap is on.
- `stdbench <dir> --net 10.0.2.2:8000` against a host `python3 -m http.server 8000` prints connect, close, a small GET and a 64 MiB download separately; `stdfs --get` downloads over HTTP through `std::net`. See [testing.md](testing.md) and [building.md](building.md).

---

## Tests

### On the machine: `run=net_test`

`net test` in the console, or `run=net_test` on the GRUB line. [`kernel/net/selftest.rs`](../../kernel/net/selftest.rs) checks only what needs the machine, against QEMU's user network:

| Case | Checks |
|:--|:--|
| `net/live` | A card is there and its link carries a frame |
| `net/dhcp-lease` | A real server answers DISCOVER and a lease is applied |
| `net/arp-gateway` | The gateway answers ARP |
| `net/ping` | An echo to the gateway comes back (and its round trip) |
| `net/drop-counters` | The demux's drop counters are sane |

The download cases that once needed a host HTTP(S) server on `10.0.2.2` are no longer in this suite; transfers are measured by `stdfs --get` and `stdbench --net` (above). The baseline was "net 4" on 2026-10-27.

### On the build host

```
cd lib/net && cargo test
```

152 test functions in `lib/net` (as of this writing; 43 when TLS moved to ring 3): SHA-1/256/384/512, HMAC and AES vectors; Montgomery arithmetic, including a real 2048-bit RSA exponentiation; all 121 roots in `ca-bundle.pem` parsed and self-verified (RSA-2048/4096, P-256/384/521); PKCS#1 against PSS cross-rejection; ECDH agreement and off-curve rejection; whole chains accepted and refused; hostname rules; ARP, DHCP, DNS, HTTP and TCP wire formats; and `tests/tcp_conn.rs`, which puts two `Connection`s face to face over a lossy wire (window-scale negotiation both ways, a 4 MiB transfer losing one segment in ten arriving whole with one resend per loss, `cwnd` growth and halving, a window delivered backwards). `lib/net/.cargo/config.toml` lets the crate escape the repository's `x86_64-unknown-none` target. Run it before booting after any change to `lib/net`.

**Target-side programs**: `stddns` (getaddrinfo), `stdremote`, `stdtokio`, `stdasync`, `stdpoll` exercise sockets through `std`; the TCP cases need a host `http.server` on port 8000 ([testing.md](testing.md)).

---

## Measured numbers

QEMU/KVM, slirp. Sources: [`dev/net-architecture.md`](../../dev/net-architecture.md), [`dev/kernel-core.md`](../../dev/kernel-core.md).

**32 MiB over HTTP through `std::net` (`stdfs --get`), 2026-10-23:**

| NIC | Before interrupts and windows | After | How driven |
|:--|:--|:--|:--|
| e1000 (82540EM) | 22-35 MB/s | 37-85 MB/s | polled (no MSI) |
| e1000e (82574L) | 28-49 MB/s | 21-52 MB/s | MSI (30-52 polled: QEMU's e1000e is the ceiling) |
| virtio-net | 135-148 MB/s | 233-281 MB/s | MSI-X |

**virtio-net with MSI-X, against a host `http.server`, 2026-10-28:**

| | Before | After |
|:--|:--|:--|
| TCP `connect` | 22 ms (7.4 ms polled) | 0.13 ms |
| TCP `close` | 20 ms | 0.12 ms |
| HTTP GET of a small file, one connection each | 11-42 ms | 0.84 ms |
| 64 MiB download | 211-369 MB/s | 281 MB/s |

**Ping to the gateway** (`net_test`): 25-139 us on e1000e with MSI; 1.2-4.2 ms polled (the reply waited for the next tick).

**Host test**: 4 MiB over a wire that loses one segment in ten takes 3296 segments for 2996 and 1188 ms at a 1 ms round trip.

The windows matter on a real network, which only real hardware shows; none of these numbers were taken on the card machine.

---

## Limits and known gaps

- **IPv4 only**; no IPv6.
- **One interface**, no loopback, no routing table, no VLANs, no bridging, no firewall or NAT.
- **No IP fragmentation or reassembly**: fragments are dropped.
- **TCP**: no `TIME_WAIT` or `Closing` states, no delayed ACKs, no Nagle, no timestamps (PAWS/RTTM), no `TCP_NODELAY`, no `SO_REUSEADDR`, no urgent data.
- A blocking TCP `recv`, `connect` and `accept` give up after 30 s; `recv` then answers `EAGAIN`. (Until 2026-11-03 `recv` returned 0 there, indistinguishable from end of file.)
- **No `shutdown`, `getsockname`, `getpeername`, `sendmsg`/`recvmsg`**; no `bind` to port 0; `listen`'s backlog argument is ignored.
- **No `epoll`**; `poll` takes at most 256 descriptors.
- **No ICMP port-unreachable**; no gratuitous ARP.
- **Legacy INTx interrupts are not supported**; a card without MSI is polled 250 times a second.
- **One NIC**; virtio-net modern-only devices (1AF4:1041) are not matched; 10EC:8169 goes to the rtl8139 driver; the rtl8139 stops after a few packets in QEMU.
- **TLS**: client only; one TLS 1.3 suite; P-256 only for ECDHE; no revocation checking; no session resumption.
- **DNS**: A records over UDP only; no `/etc/resolv.conf`; `/etc/hosts` untested on the machine.
- 256 sockets on the machine, 160 per user.
- Not verified on the real card machine; all measurements above are QEMU.

---

## Troubleshooting

| Symptom | Look at |
|:--|:--|
| No network at all | Boot log `[net] found: vendor=... device=...` and `[net] init: ...`; `net status`; an unsupported adapter logs `unsupported network adapter` |
| No address | `[netd] DHCP ...` lines; `dhcp` to retry; `net ip` for a static address |
| Everything about 4 ms slower than expected | The card is polled: `[net] ... is polled every tick` (no MSI). In QEMU use `-netdev ... -device virtio-net-pci,...` |
| A connect takes ~500 ms in QEMU | The e1000 device model; switch to virtio-net |
| A 20 ms or 100 ms delay | A missed wake ending at a backstop; take a pcap with `filter-dump` to see which side waits |
| `getaddrinfo` returns `EAI_AGAIN` | No lease yet (`ip:` in `/proc/net` is 0.0.0.0) |
| A TLS connection is refused | The chain does not anchor in `/etc/ssl/certs/ca-bundle.pem`, the name does not match, or a certificate expired; `-v` on `wget`/`curl` shows why. `--insecure` only for testing |
| `recv` fails with `EAGAIN` on a blocking socket | Nothing arrived for 30 s (the receive timeout above); the connection may still be up |
| Frames lost with `net=raw` | `/proc/net` `raw:` counters: `dropped` (reader too slow) vs `undersized` (reader's buffer too small) |
| Drop counters rising | `net status`: checksum errors, fragments (the peer sends fragmented packets), wrong address |

Related: [scheduler.md](scheduler.md) (wait queues), [memory.md](memory.md) (DMA memory), [sdk.md](sdk.md) (std and libc), [system.md](system.md) (mikuD, mikudo), [diagnostics.md](diagnostics.md) (`/proc/net`), [testing.md](testing.md).
