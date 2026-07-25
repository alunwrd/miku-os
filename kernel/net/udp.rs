use super::ipv4;

pub const MAX_UDP_PAYLOAD: usize = 1472;

#[derive(Clone, Copy, Debug)]
pub struct UdpHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub length: u16,
}

impl UdpHeader {
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < 8 {
            return None;
        }
        Some(Self {
            src_port: u16::from_be_bytes([buf[0], buf[1]]),
            dst_port: u16::from_be_bytes([buf[2], buf[3]]),
            length: u16::from_be_bytes([buf[4], buf[5]]),
        })
    }

    pub fn payload<'a>(&self, buf: &'a [u8]) -> &'a [u8] {
        let len = (self.length as usize).saturating_sub(8);
        let len = len.min(buf.len().saturating_sub(8));
        &buf[8..8 + len]
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
    let udp_len = (8 + payload.len()) as u16;
    if out.len() < 8 + payload.len() {
        return 0;
    }

    out[0] = (src_port >> 8) as u8;
    out[1] = src_port as u8;
    out[2] = (dst_port >> 8) as u8;
    out[3] = dst_port as u8;
    out[4] = (udp_len >> 8) as u8;
    out[5] = udp_len as u8;
    out[6] = 0;
    out[7] = 0;
    out[8..8 + payload.len()].copy_from_slice(payload);

    let csum = ipv4::udp_checksum(src_ip, dst_ip, &out[..8 + payload.len()]);
    out[6] = (csum >> 8) as u8;
    out[7] = csum as u8;

    8 + payload.len()
}

pub struct UdpSocket {
    pub port: u16,
    pub rx_buf: [u8; 1500],
    pub rx_len: usize,
    pub rx_src_ip: [u8; 4],
    pub rx_src_port: u16,
    pub has_data: bool,
}

impl UdpSocket {
    pub const fn new(port: u16) -> Self {
        Self {
            port,
            rx_buf: [0u8; 1500],
            rx_len: 0,
            rx_src_ip: [0; 4],
            rx_src_port: 0,
            has_data: false,
        }
    }

    pub fn on_recv(&mut self, src_ip: &[u8; 4], src_port: u16, data: &[u8]) {
        let len = data.len().min(1500);
        self.rx_buf[..len].copy_from_slice(&data[..len]);
        self.rx_len = len;
        self.rx_src_ip = *src_ip;
        self.rx_src_port = src_port;
        self.has_data = true;
    }

    pub fn recv(&mut self) -> Option<(&[u8], [u8; 4], u16)> {
        if self.has_data {
            self.has_data = false;
            Some((
                &self.rx_buf[..self.rx_len],
                self.rx_src_ip,
                self.rx_src_port,
            ))
        } else {
            None
        }
    }
}
