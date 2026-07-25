pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;

const ICMP_ECHO_REQUEST: u8 = 8;
const ICMP_ECHO_REPLY: u8 = 0;

#[derive(Clone, Copy, Debug)]
pub struct Ipv4Header {
    pub src: [u8; 4],
    pub dst: [u8; 4],
    pub proto: u8,
    pub ttl: u8,
    pub ihl: u8,
    pub total_len: u16,
}

impl Ipv4Header {
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < 20 {
            return None;
        }
        let version = buf[0] >> 4;
        if version != 4 {
            return None;
        }
        let ihl = (buf[0] & 0x0F) * 4;
        if ihl < 20 || ihl as usize > buf.len() {
            return None;
        }
        let total_len = u16::from_be_bytes([buf[2], buf[3]]);
        let ttl = buf[8];
        let proto = buf[9];
        let mut src = [0u8; 4];
        let mut dst = [0u8; 4];
        src.copy_from_slice(&buf[12..16]);
        dst.copy_from_slice(&buf[16..20]);
        Some(Self {
            src,
            dst,
            proto,
            ttl,
            ihl,
            total_len,
        })
    }

    pub fn payload<'a>(&self, buf: &'a [u8]) -> &'a [u8] {
        let start = self.ihl as usize;
        let end = (self.total_len as usize).min(buf.len());
        if end < start { return &[]; }
        &buf[start..end]
    }
}

pub fn build(src: &[u8; 4], dst: &[u8; 4], proto: u8, payload: &[u8], out: &mut [u8]) -> usize {
    if out.len() < 20 + payload.len() {
        return 0;
    }
    let total = (20 + payload.len()) as u16;
    out[0] = 0x45;
    out[1] = 0;
    out[2] = (total >> 8) as u8;
    out[3] = total as u8;
    out[4] = 0;
    out[5] = 0;
    out[6] = 0x40;
    out[7] = 0;
    out[8] = 64;
    out[9] = proto;
    out[10] = 0;
    out[11] = 0;
    out[12..16].copy_from_slice(src);
    out[16..20].copy_from_slice(dst);

    let csum = checksum(&out[..20]);
    out[10] = (csum >> 8) as u8;
    out[11] = csum as u8;

    out[20..20 + payload.len()].copy_from_slice(payload);
    20 + payload.len()
}

pub fn checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

pub fn udp_checksum(src: &[u8; 4], dst: &[u8; 4], udp_data: &[u8]) -> u16 {
    let len = udp_data.len() as u16;
    let mut pseudo = [0u8; 12];
    pseudo[0..4].copy_from_slice(src);
    pseudo[4..8].copy_from_slice(dst);
    pseudo[8] = 0;
    pseudo[9] = PROTO_UDP;
    pseudo[10] = (len >> 8) as u8;
    pseudo[11] = len as u8;

    let mut sum = 0u32;
    let mut calc = |data: &[u8]| {
        let mut i = 0;
        while i + 1 < data.len() {
            sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
            i += 2;
        }
        if i < data.len() {
            sum += (data[i] as u32) << 8;
        }
    };
    calc(&pseudo);
    calc(udp_data);
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    let result = !(sum as u16);
    if result == 0 {
        0xFFFF
    } else {
        result
    }
}

pub fn handle_icmp(
    ip: &Ipv4Header,
    our_ip: &[u8; 4],
    icmp_payload: &[u8],
    reply_ip: &mut [u8; 1500],
) -> usize {
    if &ip.dst != our_ip {
        return 0;
    }
    if icmp_payload.len() < 8 {
        return 0;
    }
    if icmp_payload[0] != ICMP_ECHO_REQUEST {
        return 0;
    }

    let id = u16::from_be_bytes([icmp_payload[4], icmp_payload[5]]);
    let seq = u16::from_be_bytes([icmp_payload[6], icmp_payload[7]]);
    let data = &icmp_payload[8..];

    let mut icmp_reply = [0u8; 1500];
    let icmp_len = 8 + data.len();
    icmp_reply[0] = ICMP_ECHO_REPLY;
    icmp_reply[1] = 0;
    icmp_reply[2] = 0;
    icmp_reply[3] = 0;
    icmp_reply[4] = (id >> 8) as u8;
    icmp_reply[5] = id as u8;
    icmp_reply[6] = (seq >> 8) as u8;
    icmp_reply[7] = seq as u8;
    icmp_reply[8..8 + data.len()].copy_from_slice(data);

    let csum = checksum(&icmp_reply[..icmp_len]);
    icmp_reply[2] = (csum >> 8) as u8;
    icmp_reply[3] = csum as u8;

    build(
        &ip.dst,
        &ip.src,
        PROTO_ICMP,
        &icmp_reply[..icmp_len],
        reply_ip,
    )
}
