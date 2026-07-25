use super::eth::{EthFrame, BROADCAST_MAC, ETHERTYPE_ARP};

const ARP_REQUEST: u16 = 1;
const ARP_REPLY: u16 = 2;
const HW_ETHERNET: u16 = 1;
const PROTO_IPV4: u16 = 0x0800;

pub const ARP_TABLE_SIZE: usize = 16;

#[derive(Clone, Copy)]
pub struct ArpEntry {
    pub ip: [u8; 4],
    pub mac: [u8; 6],
    pub valid: bool,
}

impl ArpEntry {
    pub const fn empty() -> Self {
        Self {
            ip: [0; 4],
            mac: [0; 6],
            valid: false,
        }
    }
}

pub struct ArpTable {
    pub entries: [ArpEntry; ARP_TABLE_SIZE],
    next: usize,
}

impl ArpTable {
    pub const fn new() -> Self {
        Self {
            entries: [ArpEntry::empty(); ARP_TABLE_SIZE],
            next: 0,
        }
    }

    pub fn lookup(&self, ip: &[u8; 4]) -> Option<[u8; 6]> {
        for e in &self.entries {
            if e.valid && &e.ip == ip {
                return Some(e.mac);
            }
        }
        None
    }

    pub fn insert(&mut self, ip: [u8; 4], mac: [u8; 6]) {
        for e in self.entries.iter_mut() {
            if e.valid && e.ip == ip {
                e.mac = mac;
                return;
            }
        }
        let idx = self.next % ARP_TABLE_SIZE;
        self.entries[idx] = ArpEntry {
            ip,
            mac,
            valid: true,
        };
        self.next = (self.next + 1) % ARP_TABLE_SIZE;
    }
}

pub fn parse(buf: &[u8]) -> Option<([u8; 4], [u8; 6], u16)> {
    if buf.len() < 28 {
        return None;
    }
    let hw_type = u16::from_be_bytes([buf[0], buf[1]]);
    let proto_type = u16::from_be_bytes([buf[2], buf[3]]);
    if hw_type != HW_ETHERNET || proto_type != PROTO_IPV4 {
        return None;
    }
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

pub fn build_reply(
    our_mac: &[u8; 6],
    our_ip: &[u8; 4],
    target_mac: &[u8; 6],
    target_ip: &[u8; 4],
    out: &mut [u8; 28],
) {
    out[0..2].copy_from_slice(&HW_ETHERNET.to_be_bytes());
    out[2..4].copy_from_slice(&PROTO_IPV4.to_be_bytes());
    out[4] = 6;
    out[5] = 4;
    out[6..8].copy_from_slice(&ARP_REPLY.to_be_bytes());
    out[8..14].copy_from_slice(our_mac);
    out[14..18].copy_from_slice(our_ip);
    out[18..24].copy_from_slice(target_mac);
    out[24..28].copy_from_slice(target_ip);
}

pub fn build_request(our_mac: &[u8; 6], our_ip: &[u8; 4], target_ip: &[u8; 4], out: &mut [u8; 28]) {
    out[0..2].copy_from_slice(&HW_ETHERNET.to_be_bytes());
    out[2..4].copy_from_slice(&PROTO_IPV4.to_be_bytes());
    out[4] = 6;
    out[5] = 4;
    out[6..8].copy_from_slice(&ARP_REQUEST.to_be_bytes());
    out[8..14].copy_from_slice(our_mac);
    out[14..18].copy_from_slice(our_ip);
    out[18..24].copy_from_slice(&[0u8; 6]);
    out[24..28].copy_from_slice(target_ip);
}

pub fn send_request(
    our_mac: &[u8; 6],
    our_ip: &[u8; 4],
    target_ip: &[u8; 4],
    tx_buf: &mut [u8; 64],
) -> usize {
    let mut arp_payload = [0u8; 28];
    build_request(our_mac, our_ip, target_ip, &mut arp_payload);
    EthFrame::build(&BROADCAST_MAC, our_mac, ETHERTYPE_ARP, &arp_payload, tx_buf)
}

pub fn handle(
    frame: &EthFrame,
    our_mac: &[u8; 6],
    our_ip: &[u8; 4],
    table: &mut ArpTable,
    reply_buf: &mut [u8; 64],
) -> usize {
    let (sender_ip, sender_mac, op) = match parse(frame.payload) {
        Some(v) => v,
        None => return 0,
    };

    table.insert(sender_ip, sender_mac);

    if op == ARP_REQUEST {
        let mut target_ip = [0u8; 4];
        if frame.payload.len() >= 28 {
            target_ip.copy_from_slice(&frame.payload[24..28]);
        }
        if &target_ip == our_ip {
            let mut arp_reply = [0u8; 28];
            build_reply(our_mac, our_ip, &sender_mac, &sender_ip, &mut arp_reply);
            return EthFrame::build(&sender_mac, our_mac, ETHERTYPE_ARP, &arp_reply, reply_buf);
        }
    }

    0 
}
