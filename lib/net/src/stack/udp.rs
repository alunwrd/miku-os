// UDP header and checksum

use crate::ipv4;

/// 1500 MTU less the 20-byte IPv4 header and the 8-byte UDP header
pub const MAX_UDP_PAYLOAD: usize = 1472;

pub const HEADER_LEN: usize = 8;

#[derive(Clone, Copy, Debug)]
pub struct UdpHeader {
    pub src_port: u16,
    pub dst_port: u16,
    /// Header plus payload as the sender wrote it; `payload` also clamps to the
    /// buffer
    pub length: u16,
}

impl UdpHeader {
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < HEADER_LEN {
            return None;
        }
        Some(Self {
            src_port: u16::from_be_bytes([buf[0], buf[1]]),
            dst_port: u16::from_be_bytes([buf[2], buf[3]]),
            length: u16::from_be_bytes([buf[4], buf[5]]),
        })
    }

    /// Datagram body, clamped by both the claimed length and the bytes that
    /// arrived
    pub fn payload<'a>(&self, buf: &'a [u8]) -> &'a [u8] {
        let len = (self.length as usize).saturating_sub(HEADER_LEN);
        let len = len.min(buf.len().saturating_sub(HEADER_LEN));
        &buf[HEADER_LEN..HEADER_LEN + len]
    }
}

pub fn build(
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
    src_ip: &[u8; 4],
    dst_ip: &[u8; 4],
    out: &mut [u8],
) -> usize {
    let total = HEADER_LEN + payload.len();
    if out.len() < total {
        return 0;
    }
    let udp_len = total as u16;

    out[0] = (src_port >> 8) as u8;
    out[1] = src_port as u8;
    out[2] = (dst_port >> 8) as u8;
    out[3] = dst_port as u8;
    out[4] = (udp_len >> 8) as u8;
    out[5] = udp_len as u8;
    out[6] = 0;
    out[7] = 0;
    out[HEADER_LEN..total].copy_from_slice(payload);

    let csum = ipv4::udp_checksum(src_ip, dst_ip, &out[..total]);
    out[6] = (csum >> 8) as u8;
    out[7] = csum as u8;

    total
}
