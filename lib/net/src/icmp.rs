// ICMP for programs
//
// A raw socket delivers whole IP packets, so this is a builder for outgoing
// messages and a parser for replies. Used by `ping` and `traceroute`.
//
// ICMP has no ports, so every ICMP consumer receives every message and must
// recognize its own by identifier. The identifier comes from the kernel's
// entropy, not the process id.

extern crate alloc;
use alloc::vec::Vec;

pub const ECHO_REPLY: u8 = 0;
pub const DEST_UNREACHABLE: u8 = 3;
pub const ECHO_REQUEST: u8 = 8;
pub const TIME_EXCEEDED: u8 = 11;

/// The internet checksum, RFC 1071
pub use crate::ipv4::checksum;

/// An echo request with `payload` after the header
pub fn echo_request(id: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&[ECHO_REQUEST, 0, 0, 0]);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(payload);
    let c = checksum(&out);
    out[2] = (c >> 8) as u8;
    out[3] = c as u8;
    out
}

/// Reply to an echo request, from the request's ICMP bytes. Identifier,
/// sequence, and body are unchanged. None for anything that is not an echo
/// request.
///
/// The body is capped at what an unfragmented reply can carry, since a card
/// with 2 KB receive buffers can deliver more than a reply frame holds
pub fn echo_reply(request: &[u8]) -> Option<Vec<u8>> {
    if request.len() < 8 || request[0] != ECHO_REQUEST {
        return None;
    }
    const MAX_ECHO_DATA: usize = 1500 - 20 - 8;
    let body = &request[8..request.len().min(8 + MAX_ECHO_DATA)];

    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&[ECHO_REPLY, 0, 0, 0]);
    // Identifier and sequence straight through, in the sender's byte order
    out.extend_from_slice(&request[4..8]);
    out.extend_from_slice(body);
    let c = checksum(&out);
    out[2] = (c >> 8) as u8;
    out[3] = c as u8;
    Some(out)
}

/// A message read from an IP packet delivered by a raw socket
pub struct Message {
    pub icmp_type: u8,
    pub code: u8,
    /// Remaining hop limit; `ping` subtracts it from the sender's starting
    /// value
    pub ttl: u8,
    pub src: [u8; 4],
    /// For an echo reply, from the header. For time-exceeded or unreachable,
    /// from the quoted original header
    pub id: u16,
    pub seq: u16,
}

/// One ICMP message from an IP packet, or None for anything else
pub fn parse(packet: &[u8]) -> Option<Message> {
    let ip = crate::ipv4::Ipv4Header::parse(packet)?;
    // Only ICMP should reach this socket; the check guards against reading a
    // TCP segment as an ICMP header
    if ip.proto != crate::ipv4::PROTO_ICMP {
        return None;
    }
    if packet.len() < ip.ihl as usize + 8 {
        return None;
    }

    let ttl = ip.ttl;
    let src = ip.src;

    let icmp = &packet[ip.ihl as usize..];
    let icmp_type = icmp[0];
    let code = icmp[1];

    let (id, seq) = match icmp_type {
        ECHO_REPLY | ECHO_REQUEST => (
            u16::from_be_bytes([icmp[4], icmp[5]]),
            u16::from_be_bytes([icmp[6], icmp[7]]),
        ),
        // Time-exceeded and unreachable messages quote the original IP header
        // and the next eight bytes, which for an echo request is the whole ICMP
        // header. traceroute uses them to recognize its probes
        TIME_EXCEEDED | DEST_UNREACHABLE => {
            let quoted = &icmp[8..];
            match crate::ipv4::Ipv4Header::parse(quoted) {
                Some(q) if quoted.len() >= q.ihl as usize + 8 => {
                    let h = &quoted[q.ihl as usize..];
                    (
                        u16::from_be_bytes([h[4], h[5]]),
                        u16::from_be_bytes([h[6], h[7]]),
                    )
                }
                _ => (0, 0),
            }
        }
        _ => (0, 0),
    };

    Some(Message { icmp_type, code, ttl, src, id, seq })
}

/// A short name for a message type, for a program that has to print it
pub fn describe(icmp_type: u8, code: u8) -> &'static str {
    match (icmp_type, code) {
        (ECHO_REPLY, _) => "echo reply",
        (DEST_UNREACHABLE, 0) => "network unreachable",
        (DEST_UNREACHABLE, 1) => "host unreachable",
        (DEST_UNREACHABLE, 2) => "protocol unreachable",
        (DEST_UNREACHABLE, 3) => "port unreachable",
        (DEST_UNREACHABLE, 4) => "fragmentation needed",
        (DEST_UNREACHABLE, _) => "unreachable",
        (TIME_EXCEEDED, 0) => "time exceeded in transit",
        (TIME_EXCEEDED, 1) => "fragment reassembly time exceeded",
        (TIME_EXCEEDED, _) => "time exceeded",
        _ => "icmp",
    }
}
