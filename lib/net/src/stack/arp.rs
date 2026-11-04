// ARP table and frames. The clock is a parameter in milliseconds, so expiry is
// testable without waiting

use crate::stack::eth::{EthFrame, BROADCAST_MAC, ETHERTYPE_ARP};

const ARP_REQUEST: u16 = 1;
const ARP_REPLY: u16 = 2;
const HW_ETHERNET: u16 = 1;
const PROTO_IPV4: u16 = 0x0800;

/// Wire format: 28 bytes for Ethernet and IPv4
pub const PACKET_LEN: usize = 28;

pub const ARP_TABLE_SIZE: usize = 16;

/// How long a learned mapping is trusted. Bounds how long a gateway that
/// changes its MAC stays unreachable
pub const ENTRY_TTL_MS: u64 = 60_000;

#[derive(Clone, Copy)]
pub struct ArpEntry {
    pub ip: [u8; 4],
    pub mac: [u8; 6],
    pub valid: bool,
    /// When this mapping was last confirmed, in the caller's milliseconds
    pub learned: u64,
}

impl ArpEntry {
    pub const fn empty() -> Self {
        Self { ip: [0; 4], mac: [0; 6], valid: false, learned: 0 }
    }
}

pub struct ArpTable {
    pub entries: [ArpEntry; ARP_TABLE_SIZE],
    next: usize,
}

impl Default for ArpTable {
    fn default() -> Self {
        Self::new()
    }
}

impl ArpTable {
    pub const fn new() -> Self {
        Self { entries: [ArpEntry::empty(); ARP_TABLE_SIZE], next: 0 }
    }

    pub fn lookup(&self, ip: &[u8; 4], now_ms: u64) -> Option<[u8; 6]> {
        for e in &self.entries {
            if e.valid && &e.ip == ip {
                if now_ms.wrapping_sub(e.learned) >= ENTRY_TTL_MS {
                    return None; // stale: ask again
                }
                return Some(e.mac);
            }
        }
        None
    }

    /// Whether a mapping exists for this address, expired or not. Tells a
    /// refresh from someone else's ARP
    pub fn knows(&self, ip: &[u8; 4]) -> bool {
        self.entries.iter().any(|e| e.valid && &e.ip == ip)
    }

    pub fn insert(&mut self, ip: [u8; 4], mac: [u8; 6], now_ms: u64) {
        // 0.0.0.0 is what a host with no address puts in its probes
        if ip == [0, 0, 0, 0] {
            return;
        }
        for e in self.entries.iter_mut() {
            if e.valid && e.ip == ip {
                e.mac = mac;
                e.learned = now_ms;
                return;
            }
        }
        // A free or expired slot before evicting a live one
        let idx = self
            .entries
            .iter()
            .position(|e| !e.valid || now_ms.wrapping_sub(e.learned) >= ENTRY_TTL_MS)
            .unwrap_or_else(|| {
                let i = self.next % ARP_TABLE_SIZE;
                self.next = (self.next + 1) % ARP_TABLE_SIZE;
                i
            });
        self.entries[idx] = ArpEntry { ip, mac, valid: true, learned: now_ms };
    }
}

/// sender IP, sender MAC, opcode
pub fn parse(buf: &[u8]) -> Option<([u8; 4], [u8; 6], u16)> {
    if buf.len() < PACKET_LEN {
        return None;
    }
    let hw_type = u16::from_be_bytes([buf[0], buf[1]]);
    let proto_type = u16::from_be_bytes([buf[2], buf[3]]);
    if hw_type != HW_ETHERNET || proto_type != PROTO_IPV4 {
        return None;
    }
    // Six bytes of hardware address and four of protocol address, or not an ARP
    // this reads
    if buf[4] != 6 || buf[5] != 4 {
        return None;
    }
    let op = u16::from_be_bytes([buf[6], buf[7]]);
    let mut sender_mac = [0u8; 6];
    let mut sender_ip = [0u8; 4];
    sender_mac.copy_from_slice(&buf[8..14]);
    sender_ip.copy_from_slice(&buf[14..18]);
    Some((sender_ip, sender_mac, op))
}

/// The address this ARP packet is asking about
pub fn target_ip(buf: &[u8]) -> Option<[u8; 4]> {
    if buf.len() < PACKET_LEN {
        return None;
    }
    let mut ip = [0u8; 4];
    ip.copy_from_slice(&buf[24..28]);
    Some(ip)
}

fn build(op: u16, our_mac: &[u8; 6], our_ip: &[u8; 4], target_mac: &[u8; 6], tip: &[u8; 4])
    -> [u8; PACKET_LEN]
{
    let mut out = [0u8; PACKET_LEN];
    out[0..2].copy_from_slice(&HW_ETHERNET.to_be_bytes());
    out[2..4].copy_from_slice(&PROTO_IPV4.to_be_bytes());
    out[4] = 6;
    out[5] = 4;
    out[6..8].copy_from_slice(&op.to_be_bytes());
    out[8..14].copy_from_slice(our_mac);
    out[14..18].copy_from_slice(our_ip);
    out[18..24].copy_from_slice(target_mac);
    out[24..28].copy_from_slice(tip);
    out
}

pub fn build_reply(
    our_mac: &[u8; 6],
    our_ip: &[u8; 4],
    target_mac: &[u8; 6],
    target_ip: &[u8; 4],
) -> [u8; PACKET_LEN] {
    build(ARP_REPLY, our_mac, our_ip, target_mac, target_ip)
}

pub fn build_request(our_mac: &[u8; 6], our_ip: &[u8; 4], target_ip: &[u8; 4])
    -> [u8; PACKET_LEN]
{
    build(ARP_REQUEST, our_mac, our_ip, &[0u8; 6], target_ip)
}

/// A broadcast "who has this address" frame, ready for the wire
pub fn request_frame(
    our_mac: &[u8; 6],
    our_ip: &[u8; 4],
    target_ip: &[u8; 4],
    out: &mut [u8],
) -> usize {
    let payload = build_request(our_mac, our_ip, target_ip);
    EthFrame::build(&BROADCAST_MAC, our_mac, ETHERTYPE_ARP, &payload, out)
}

/// Takes in one ARP frame: learns from it if it concerns us, answers a question
/// addressed to us. Returns the reply length in `out`, or 0
pub fn handle(
    frame: &EthFrame,
    our_mac: &[u8; 6],
    our_ip: &[u8; 4],
    table: &mut ArpTable,
    now_ms: u64,
    out: &mut [u8],
) -> usize {
    let (sender_ip, sender_mac, op) = match parse(frame.payload) {
        Some(v) => v,
        None => return 0,
    };
    let tip = target_ip(frame.payload).unwrap_or([0; 4]);
    let for_us = &tip == our_ip;

    // Learns only from ARP concerning us: a reply we asked for, a request to
    // us, or a refresh of a held mapping. Broadcast chatter between other
    // machines would evict the gateway from the 16-entry table
    if for_us || op == ARP_REPLY || table.knows(&sender_ip) {
        table.insert(sender_ip, sender_mac, now_ms);
    }

    if op == ARP_REQUEST && for_us {
        let reply = build_reply(our_mac, our_ip, &sender_mac, &sender_ip);
        return EthFrame::build(&sender_mac, our_mac, ETHERTYPE_ARP, &reply, out);
    }

    0
}
