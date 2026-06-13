## Network Stack

### Network Card Drivers

| Driver | Chip |
|---|---|
| **Intel E1000** | 82540EM, 82545EM, 82574L, 82579LM, I217 |
| **Realtek RTL8139** | RTL8139 |
| **Realtek RTL8168** | RTL8168, RTL8169 |
| **VirtIO Net** | QEMU/KVM virtual network card |

### Protocols

| Layer | Protocols |
|---|---|
| **L2** | Ethernet, ARP (with cache table, header validation) |
| **L3** | IPv4, ICMP |
| **L4** | UDP, TCP (listener + client, state machine, retransmits) |

### Socket API

Userspace programs use BSD-style socket syscalls: `sys_socket` (56),
`sys_connect` (57), `sys_send` (58), `sys_recv` (59) - see the
[Syscall Interface](#syscall-interface) table.
| **Application** | DHCP, DNS, NTP, HTTP/1.1, HTTP/2 (HPACK), Ping, Traceroute |
| **Security** | TLS 1.2 / 1.3 (ECDHE + RSA + AES-GCM, constant-time) |

### TLS 1.2 / 1.3 Implementation

- ECDH: P-256 ECDHE key exchange (`tls_ecdh.rs`), constant-time Montgomery-style scalar multiplication (always-double-always-add + `cmov`)
- RSA: ASN.1/DER certificate parsing, PKCS#1 v1.5 padding (`tls_rsa.rs`), RDRAND-sourced padding bytes
- BigNum: custom big number implementation for RSA 2048-bit (`tls_bignum.rs`)
- AES-GCM: authenticated symmetric encryption (`tls_gcm.rs`)
- SHA-256, HMAC, HKDF: hashing and key derivation (`tls_crypto.rs`)
- Handshake: ClientHello -> ServerHello -> Certificate -> [ECDHE] -> Finished (client + server Finished verify_data checked)
- HTTP/2: RFC 7540 framing and RFC 7541 HPACK with correct Appendix B Huffman table (`http2.rs`)

### Security Hardening

| Concern | Mitigation |
|---|---|
| **RNG** | RDRAND-based CSPRNG for ClientHello random, CBC IV, ECDH private key, RSA padding (`random::random_u64`) |
| **Timing (Lucky13)** | Constant-time MAC compare via OR-accumulator byte diff |
| **Padding oracle** | Full RFC 5246 padding check - all pad bytes verified, not just the last |
| **ECDH timing leak** | `fe_cmov` / `jac_cmov` XOR-mask constant-time field / point select |
| **Server impersonation** | TLS 1.2 server Finished `verify_data = PRF(master, "server finished", hs_hash)` checked constant-time |
| **PKCS#1 padding** | RDRAND-sourced non-zero padding bytes (rejection loop) |
| **ARP spoofing** | hw_type / proto_type / hlen / plen checks before accepting ARP-IPv4 entries |

---