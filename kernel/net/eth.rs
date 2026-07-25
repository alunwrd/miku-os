pub const ETHERTYPE_IP: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;
pub const BROADCAST_MAC: [u8; 6] = [0xFF; 6];

#[derive(Clone, Copy, Debug)]
pub struct EthFrame<'a> {
    pub dst: [u8; 6],
    pub src: [u8; 6],
    pub ethertype: u16,
    pub payload: &'a [u8],
}

impl<'a> EthFrame<'a> {
    pub fn parse(buf: &'a [u8]) -> Option<Self> {
        if buf.len() < 14 {
            return None;
        }

        let mut dst = [0u8; 6];
        let mut src = [0u8; 6];

        dst.copy_from_slice(&buf[0..6]);
        src.copy_from_slice(&buf[6..12]);

        let ethertype = u16::from_be_bytes([buf[12], buf[13]]);

        Some(Self {
            dst,
            src,
            ethertype,
            payload: &buf[14..],
        })
    }

    pub fn build(
        dst: &[u8; 6],
        src: &[u8; 6],
        ethertype: u16,
        payload: &[u8],
        out: &mut [u8],
    ) -> usize {
        if out.len() < 14 + payload.len() {
            return 0;
        }

        out[0..6].copy_from_slice(dst);
        out[6..12].copy_from_slice(src);
        out[12] = (ethertype >> 8) as u8;
        out[13] = ethertype as u8;
        out[14..14 + payload.len()].copy_from_slice(payload);

        14 + payload.len()
    }
}

pub fn mac_eq(a: &[u8; 6], b: &[u8; 6]) -> bool {
    a == b
}

pub fn mac_fmt(mac: &[u8; 6], out: &mut [u8; 17]) {
    const HEX: &[u8] = b"0123456789abcdef";
    for i in 0..6 {
        out[i * 3] = HEX[(mac[i] >> 4) as usize];
        out[i * 3 + 1] = HEX[(mac[i] & 0xF) as usize];
        if i < 5 {
            out[i * 3 + 2] = b':';
        }
    }
}

