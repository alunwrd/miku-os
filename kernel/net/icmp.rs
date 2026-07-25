use super::ipv4;

pub const ICMP_ECHO_REQUEST: u8 = 8;
pub const ICMP_ECHO_REPLY: u8 = 0;

pub fn build_echo_request(id: u16, seq: u16, payload: &[u8], out: &mut [u8]) -> usize {
    let icmp_len = 8 + payload.len();
    if out.len() < icmp_len {
        return 0;
    }
    out[0] = ICMP_ECHO_REQUEST;
    out[1] = 0;
    out[2] = 0;
    out[3] = 0;
    out[4] = (id >> 8) as u8;
    out[5] = id as u8;
    out[6] = (seq >> 8) as u8;
    out[7] = seq as u8;
    out[8..8 + payload.len()].copy_from_slice(payload);
    let csum = ipv4::checksum(&out[..icmp_len]);
    out[2] = (csum >> 8) as u8;
    out[3] = csum as u8;
    icmp_len
}

pub struct EchoReply {
    pub id: u16,
    pub seq: u16,
    pub ttl: u8,
    pub data_len: usize,
}

pub fn parse_echo_reply(ip_buf: &[u8]) -> Option<EchoReply> {
    let ip = ipv4::Ipv4Header::parse(ip_buf)?;
    if ip.proto != ipv4::PROTO_ICMP {
        return None;
    }
    let icmp = ip.payload(ip_buf);
    if icmp.len() < 8 {
        return None;
    }
    if icmp[0] != ICMP_ECHO_REPLY {
        return None;
    }
    let id = u16::from_be_bytes([icmp[4], icmp[5]]);
    let seq = u16::from_be_bytes([icmp[6], icmp[7]]);
    Some(EchoReply {
        id,
        seq,
        ttl: ip.ttl,
        data_len: icmp.len().saturating_sub(8),
    })
}
