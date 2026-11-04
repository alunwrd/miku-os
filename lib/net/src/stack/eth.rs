// Ethernet framing

pub const ETHERTYPE_IP: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;
pub const BROADCAST_MAC: [u8; 6] = [0xFF; 6];

/// Header size, also the smallest frame that parses
pub const HEADER_LEN: usize = 14;

#[derive(Clone, Copy, Debug)]
pub struct EthFrame<'a> {
    pub dst: [u8; 6],
    pub src: [u8; 6],
    pub ethertype: u16,
    pub payload: &'a [u8],
}

impl<'a> EthFrame<'a> {
    pub fn parse(buf: &'a [u8]) -> Option<Self> {
        if buf.len() < HEADER_LEN {
            return None;
        }

        let mut dst = [0u8; 6];
        let mut src = [0u8; 6];
        dst.copy_from_slice(&buf[0..6]);
        src.copy_from_slice(&buf[6..12]);

        Some(Self {
            dst,
            src,
            ethertype: u16::from_be_bytes([buf[12], buf[13]]),
            payload: &buf[HEADER_LEN..],
        })
    }

    pub fn build(
        dst: &[u8; 6],
        src: &[u8; 6],
        ethertype: u16,
        payload: &[u8],
        out: &mut [u8],
    ) -> usize {
        if out.len() < HEADER_LEN + payload.len() {
            return 0;
        }

        out[0..6].copy_from_slice(dst);
        out[6..12].copy_from_slice(src);
        out[12] = (ethertype >> 8) as u8;
        out[13] = ethertype as u8;
        out[HEADER_LEN..HEADER_LEN + payload.len()].copy_from_slice(payload);

        HEADER_LEN + payload.len()
    }
}

/// Whether a frame with this destination is for us, including broadcast and
/// multicast. Promiscuous mode, a hub, or a mirrored switch port delivers other
/// machines' traffic
pub fn addressed_to(dst: &[u8; 6], me: &[u8; 6]) -> bool {
    dst == me || *dst == BROADCAST_MAC || dst[0] & 0x01 != 0
}
