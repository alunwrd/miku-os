// IPv4 headers and the checksums the layers above need
//
// Outside the `stack` feature: a raw ICMP socket delivers whole IP packets, so
// `ping` and `traceroute` parse this header without a stack.
//
// `sum16` is the shared primitive. The header, TCP, UDP, and ICMP checksums are
// its complement over different bytes, and the verifiers compare it to 0xFFFF.

pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;

/// What a packet leaves with unless the sender asked for something else
pub const DEFAULT_TTL: u8 = 64;

#[derive(Clone, Copy, Debug)]
pub struct Ipv4Header {
    pub src: [u8; 4],
    pub dst: [u8; 4],
    pub proto: u8,
    pub ttl: u8,
    /// Header length in bytes, already multiplied out of the 32-bit-word field
    pub ihl: u8,
    pub total_len: u16,
    /// One piece of a larger datagram: "more fragments" set or a non-zero
    /// offset
    pub fragmented: bool,
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
        let frag = u16::from_be_bytes([buf[6], buf[7]]);
        // Bit 13 is MF ("more fragments"); the low 13 bits are the offset
        let fragmented = frag & 0x2000 != 0 || frag & 0x1FFF != 0;
        let ttl = buf[8];
        let proto = buf[9];
        let mut src = [0u8; 4];
        let mut dst = [0u8; 4];
        src.copy_from_slice(&buf[12..16]);
        dst.copy_from_slice(&buf[16..20]);
        Some(Self { src, dst, proto, ttl, ihl, total_len, fragmented })
    }

    pub fn payload<'a>(&self, buf: &'a [u8]) -> &'a [u8] {
        let start = self.ihl as usize;
        let end = (self.total_len as usize).min(buf.len());
        if end < start {
            return &[];
        }
        &buf[start..end]
    }
}

pub fn build(src: &[u8; 4], dst: &[u8; 4], proto: u8, payload: &[u8], out: &mut [u8]) -> usize {
    build_ttl(src, dst, proto, payload, DEFAULT_TTL, out)
}

/// Same header with an explicit hop limit, for traceroute
pub fn build_ttl(
    src: &[u8; 4],
    dst: &[u8; 4],
    proto: u8,
    payload: &[u8],
    ttl: u8,
    out: &mut [u8],
) -> usize {
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
    out[8] = ttl;
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

/// RFC 1071 one's-complement sum over several buffers, folded to 16 bits. A
/// correct header or segment summed with its own checksum field comes to
/// 0xFFFF, including when the sender folded a zero result to 0xFFFF
pub fn sum16(chunks: &[&[u8]]) -> u16 {
    let mut sum = 0u32;
    // An odd-length chunk's last byte is the high half of a word whose low half
    // starts the next chunk
    let mut carried: Option<u8> = None;
    for chunk in chunks {
        let mut i = 0;
        if let Some(hi) = carried.take() {
            if chunk.is_empty() {
                carried = Some(hi);
            } else {
                sum += u16::from_be_bytes([hi, chunk[0]]) as u32;
                i = 1;
            }
        }
        while i + 1 < chunk.len() {
            sum += u16::from_be_bytes([chunk[i], chunk[i + 1]]) as u32;
            i += 2;
        }
        if i < chunk.len() {
            carried = Some(chunk[i]);
        }
    }
    if let Some(hi) = carried {
        sum += (hi as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    sum as u16
}

/// The 12-byte TCP/UDP pseudo-header both L4 checksums are computed over
pub fn pseudo_header(src: &[u8; 4], dst: &[u8; 4], proto: u8, len: usize) -> [u8; 12] {
    let mut p = [0u8; 12];
    p[0..4].copy_from_slice(src);
    p[4..8].copy_from_slice(dst);
    p[8] = 0;
    p[9] = proto;
    p[10] = (len >> 8) as u8;
    p[11] = len as u8;
    p
}

/// Internet checksum over one run of bytes. ICMP and the IPv4 header use it
/// with no pseudo-header
pub fn checksum(data: &[u8]) -> u16 {
    !sum16(&[data])
}

/// True if an IPv4 header's own checksum field checks out
pub fn verify_header(hdr: &[u8]) -> bool {
    sum16(&[hdr]) == 0xFFFF
}

/// Checksum for a TCP or UDP segment with pseudo-header. A computed zero is
/// sent as 0xFFFF, as UDP requires (0 means not computed); in one's complement
/// the two are the same value
pub fn l4_checksum(src: &[u8; 4], dst: &[u8; 4], proto: u8, l4_data: &[u8]) -> u16 {
    let pseudo = pseudo_header(src, dst, proto, l4_data.len());
    let r = !sum16(&[&pseudo, l4_data]);
    if r == 0 {
        0xFFFF
    } else {
        r
    }
}

/// Whether a TCP or UDP checksum is correct. The caller skips UDP segments with
/// a zero checksum, which the sender did not compute
pub fn verify_l4(src: &[u8; 4], dst: &[u8; 4], proto: u8, l4_data: &[u8]) -> bool {
    let pseudo = pseudo_header(src, dst, proto, l4_data.len());
    sum16(&[&pseudo, l4_data]) == 0xFFFF
}

pub fn udp_checksum(src: &[u8; 4], dst: &[u8; 4], udp_data: &[u8]) -> u16 {
    l4_checksum(src, dst, PROTO_UDP, udp_data)
}
