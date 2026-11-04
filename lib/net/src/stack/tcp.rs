// TCP on the wire: sequence arithmetic, headers, options. The connection state
// machine is in `conn`.
//
// TCP numbers bytes in 32 bits and wraps, so "greater than" is not `>`. A wrong
// comparison discards live data as an old duplicate.

use crate::ipv4;

pub const FLAG_FIN: u8 = 0x01;
pub const FLAG_SYN: u8 = 0x02;
pub const FLAG_RST: u8 = 0x04;
pub const FLAG_PSH: u8 = 0x08;
pub const FLAG_ACK: u8 = 0x10;

/// The smallest TCP header, with no options
pub const HEADER_LEN: usize = 20;
/// The largest, with the option area full
pub const MAX_HEADER_LEN: usize = 60;

/// MSS we advertise in our own SYN / SYN-ACK
pub const OUR_MSS: u16 = 1460;

/// Most payload in one segment; the peer's MSS can only lower it. 1400 stays
/// under a 1500 MTU
pub const MAX_SEG: usize = 1400;

/// Whether a comes after b in the wrapping 32-bit sequence space. The
/// difference read as signed is positive when `a` is within half the space
/// ahead of `b`
#[inline]
pub fn seq_gt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

#[inline]
pub fn seq_ge(a: u32, b: u32) -> bool {
    a == b || seq_gt(a, b)
}

#[inline]
pub fn seq_lt(a: u32, b: u32) -> bool {
    seq_gt(b, a)
}

#[inline]
pub fn seq_le(a: u32, b: u32) -> bool {
    seq_ge(b, a)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TcpState {
    Closed,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    LastAck,
}

#[derive(Clone, Copy, Debug)]
pub struct TcpSegment {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub window: u16,
    /// Header length in bytes, already multiplied out of the 32-bit-word field
    pub data_offset: u8,
}

impl TcpSegment {
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < HEADER_LEN {
            return None;
        }
        let data_offset = (buf[12] >> 4) * 4;
        if (data_offset as usize) < HEADER_LEN {
            return None;
        }
        // A header longer than the segment is refused, so `payload` and
        // `options` below get a valid range
        if data_offset as usize > buf.len() {
            return None;
        }
        Some(Self {
            src_port: u16::from_be_bytes([buf[0], buf[1]]),
            dst_port: u16::from_be_bytes([buf[2], buf[3]]),
            seq: u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]),
            ack: u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]),
            data_offset,
            flags: buf[13],
            window: u16::from_be_bytes([buf[14], buf[15]]),
        })
    }

    pub fn payload<'a>(&self, buf: &'a [u8]) -> &'a [u8] {
        let off = self.data_offset as usize;
        if off >= buf.len() {
            &[]
        } else {
            &buf[off..]
        }
    }

    /// The option area between the fixed header and the payload
    pub fn options<'a>(&self, buf: &'a [u8]) -> &'a [u8] {
        let off = (self.data_offset as usize).min(buf.len());
        &buf[HEADER_LEN.min(off)..off]
    }

    /// How much sequence space this segment occupies. SYN and FIN each take one
    /// number
    pub fn seq_len(&self, payload_len: usize) -> u32 {
        let mut n = payload_len as u32;
        if self.flags & FLAG_SYN != 0 {
            n += 1;
        }
        if self.flags & FLAG_FIN != 0 {
            n += 1;
        }
        n
    }

    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

/// The peer's advertised MSS, if sent. Walks the options, stopping at the
/// end-of-options marker and on an invalid length
pub fn parse_mss(opts: &[u8]) -> Option<u16> {
    let mut i = 0;
    while i < opts.len() {
        match opts[i] {
            0 => return None,        // end of option list
            1 => { i += 1; continue } // no-op padding
            _ => {}
        }
        if i + 1 >= opts.len() {
            return None;
        }
        let len = opts[i + 1] as usize;
        // Other options carry a length that includes the kind and length bytes,
        // so below 2 is invalid
        if len < 2 || i + len > opts.len() {
            return None;
        }
        if opts[i] == 2 && len == 4 {
            return Some(u16::from_be_bytes([opts[i + 2], opts[i + 3]]));
        }
        i += len;
    }
    None
}

/// Builds a TCP segment with an option block. `opts` length must be a multiple
/// of 4 (pad with no-ops); `&[]` for a plain header
#[allow(clippy::too_many_arguments)]
pub fn build_opts(
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    opts: &[u8],
    payload: &[u8],
    src_ip: &[u8; 4],
    dst_ip: &[u8; 4],
    out: &mut [u8],
) -> usize {
    if opts.len() % 4 != 0 || opts.len() > MAX_HEADER_LEN - HEADER_LEN {
        return 0;
    }
    let hdr_len = HEADER_LEN + opts.len();
    let tcp_len = hdr_len + payload.len();
    if out.len() < tcp_len {
        return 0;
    }
    out[0..2].copy_from_slice(&src_port.to_be_bytes());
    out[2..4].copy_from_slice(&dst_port.to_be_bytes());
    out[4..8].copy_from_slice(&seq.to_be_bytes());
    out[8..12].copy_from_slice(&ack.to_be_bytes());
    out[12] = ((hdr_len / 4) as u8) << 4;
    out[13] = flags;
    out[14..16].copy_from_slice(&window.to_be_bytes());
    out[16] = 0;
    out[17] = 0;
    out[18] = 0;
    out[19] = 0;

    out[HEADER_LEN..hdr_len].copy_from_slice(opts);
    if !payload.is_empty() {
        out[hdr_len..tcp_len].copy_from_slice(payload);
    }

    let csum = checksum(src_ip, dst_ip, &out[..tcp_len]);
    out[16] = (csum >> 8) as u8;
    out[17] = csum as u8;

    tcp_len
}

#[allow(clippy::too_many_arguments)]
pub fn build(
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    payload: &[u8],
    src_ip: &[u8; 4],
    dst_ip: &[u8; 4],
    out: &mut [u8],
) -> usize {
    build_opts(src_port, dst_port, seq, ack, flags, window, &[], payload, src_ip, dst_ip, out)
}

/// The 4-byte MSS option block advertised in our SYN / SYN-ACK
pub fn mss_opts() -> [u8; 4] {
    [2, 4, (OUR_MSS >> 8) as u8, OUR_MSS as u8]
}

/// Every option as (kind, body), stopping at end-of-list or an invalid length
fn for_each_option(opts: &[u8], mut f: impl FnMut(u8, &[u8])) {
    let mut i = 0;
    while i < opts.len() {
        match opts[i] {
            0 => return,
            1 => {
                i += 1;
                continue;
            }
            _ => {}
        }
        if i + 1 >= opts.len() {
            return;
        }
        let len = opts[i + 1] as usize;
        if len < 2 || i + len > opts.len() {
            return;
        }
        f(opts[i], &opts[i + 2..i + len]);
        i += len;
    }
}

/// What a SYN said about the connection it opens
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct SynOptions {
    pub mss: Option<u16>,
    /// Shift the sender applies to every window after the SYN (RFC 7323 2.2).
    /// At most 14
    pub wscale: Option<u8>,
    /// It will understand SACK blocks in our ACKs (RFC 2018)
    pub sack_permitted: bool,
}

pub fn parse_syn_options(opts: &[u8]) -> SynOptions {
    let mut out = SynOptions::default();
    for_each_option(opts, |kind, body| match (kind, body.len()) {
        (2, 2) => out.mss = Some(u16::from_be_bytes([body[0], body[1]])),
        (3, 1) => out.wscale = Some(body[0].min(14)),
        (4, 0) => out.sack_permitted = true,
        _ => {}
    });
    out
}

/// SACK blocks in `opts`, each `(left, right)`: first number held and the one
/// after the last. Returns how many went into `out`
pub fn parse_sack(opts: &[u8], out: &mut [(u32, u32); 4]) -> usize {
    let mut n = 0;
    for_each_option(opts, |kind, body| {
        if kind != 5 {
            return;
        }
        for b in body.chunks_exact(8) {
            if n < out.len() {
                let l = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
                let r = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
                out[n] = (l, r);
                n += 1;
            }
        }
    });
    n
}

/// A SYN's options: our MSS, SACK permitted, and a window shift, padded with
/// no-ops to four bytes. A SYN-ACK carries the last two only if the SYN did
pub fn syn_opts(wscale: Option<u8>, sack: bool) -> ([u8; 12], usize) {
    let mut o = [0u8; 12];
    o[..4].copy_from_slice(&mss_opts());
    let mut n = 4;
    if sack {
        o[n..n + 4].copy_from_slice(&[1, 1, 4, 2]);
        n += 4;
    }
    if let Some(s) = wscale {
        o[n..n + 4].copy_from_slice(&[1, 3, 3, s]);
        n += 4;
    }
    (o, n)
}

/// An ACK's SACK option for `blocks` (at most four), led by two no-ops
pub fn sack_opts(blocks: &[(u32, u32)]) -> ([u8; 36], usize) {
    let mut o = [0u8; 36];
    let k = blocks.len().min(4);
    if k == 0 {
        return (o, 0);
    }
    o[0] = 1;
    o[1] = 1;
    o[2] = 5;
    o[3] = (2 + 8 * k) as u8;
    for (i, &(l, r)) in blocks[..k].iter().enumerate() {
        o[4 + i * 8..8 + i * 8].copy_from_slice(&l.to_be_bytes());
        o[8 + i * 8..12 + i * 8].copy_from_slice(&r.to_be_bytes());
    }
    (o, 4 + 8 * k)
}

pub fn checksum(src: &[u8; 4], dst: &[u8; 4], tcp_data: &[u8]) -> u16 {
    ipv4::l4_checksum(src, dst, ipv4::PROTO_TCP, tcp_data)
}

pub fn verify(src: &[u8; 4], dst: &[u8; 4], tcp_data: &[u8]) -> bool {
    ipv4::verify_l4(src, dst, ipv4::PROTO_TCP, tcp_data)
}
