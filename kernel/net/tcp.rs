extern crate alloc;
use alloc::collections::VecDeque;
use core::sync::atomic::Ordering;
use super::CTRL_C;
use super::eth::{EthFrame, ETHERTYPE_IP};
use super::{ipv4, rx};

pub const FLAG_FIN: u8 = 0x01;
pub const FLAG_SYN: u8 = 0x02;
pub const FLAG_RST: u8 = 0x04;
pub const FLAG_PSH: u8 = 0x08;
pub const FLAG_ACK: u8 = 0x10;

const MAX_RETRIES: usize = 5;
const RX_BUF: usize = 32768;

/// Max payload per segment we will ever send; the peer's SYN MSS can only
/// lower this. 1400 keeps the frame comfortably under a 1500 MTU
const MAX_SEG: usize = 1400;
/// MSS we advertise in our own SYN / SYN-ACK
const OUR_MSS: u16 = 1460;
/// Send window: how many segments may be in flight unacknowledged
const SND_WND_SEGS: usize = 8;

// RFC 6298 RTO bounds (tick = 4 ms at TIMER_HZ_DEFAULT 250). We start
// with a 1 s RTO before any RTT has been measured, allow it to shrink to
// 250 ms once SRTT settles, and cap it at 60 s
const INIT_RTO_TICKS: u32 = 250;   // 1 s
const MIN_RTO_TICKS:  u32 = 62;    // ~250 ms (Linux-style floor, < RFC 1 s but practical)
const MAX_RTO_TICKS:  u32 = 15000; // 60 s

#[inline]
fn wrapping_gt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

#[inline]
fn wrapping_ge(a: u32, b: u32) -> bool {
    a == b || wrapping_gt(a, b)
}

fn random_isn() -> u32 {
    let tsc: u64;
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem));
        tsc = ((hi as u64) << 32) | lo as u64;
    }
    let up = crate::fs::procfs::uptime_ticks() as u64;
    let v = tsc
        .wrapping_mul(6364136223846793005)
        .wrapping_add(up)
        .wrapping_mul(0x9E3779B97F4A7C15);
    ((v >> 17) ^ (v >> 33)) as u32
}

#[derive(Clone, Copy, PartialEq, Debug)]
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
    pub data_offset: u8,
}

impl TcpSegment {
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < 20 {
            return None;
        }
        let data_offset = (buf[12] >> 4) * 4;
        if data_offset < 20 {
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
}

/// Build a TCP segment with an option block. 'opts' length must be a
/// multiple of 4 (pad with NOPs); pass &[] for a plain header
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
    if opts.len() % 4 != 0 || opts.len() > 40 {
        return 0;
    }
    let hdr_len = 20 + opts.len();
    let tcp_len = hdr_len + payload.len();
    if out.len() < tcp_len {
        return 0;
    }
    out[0] = (src_port >> 8) as u8;
    out[1] = src_port as u8;
    out[2] = (dst_port >> 8) as u8;
    out[3] = dst_port as u8;
    out[4] = (seq >> 24) as u8;
    out[5] = (seq >> 16) as u8;
    out[6] = (seq >> 8) as u8;
    out[7] = seq as u8;
    out[8] = (ack >> 24) as u8;
    out[9] = (ack >> 16) as u8;
    out[10] = (ack >> 8) as u8;
    out[11] = ack as u8;
    out[12] = ((hdr_len / 4) as u8) << 4;
    out[13] = flags;
    out[14] = (window >> 8) as u8;
    out[15] = window as u8;
    out[16] = 0;
    out[17] = 0;
    out[18] = 0;
    out[19] = 0;

    out[20..20 + opts.len()].copy_from_slice(opts);
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

pub fn checksum(src: &[u8; 4], dst: &[u8; 4], tcp_data: &[u8]) -> u16 {
    let len = tcp_data.len() as u16;
    let mut pseudo = [0u8; 12];
    pseudo[0..4].copy_from_slice(src);
    pseudo[4..8].copy_from_slice(dst);
    pseudo[8] = 0;
    pseudo[9] = ipv4::PROTO_TCP;
    pseudo[10] = (len >> 8) as u8;
    pseudo[11] = len as u8;

    let mut sum = 0u32;
    for chunk in [pseudo.as_slice(), tcp_data] {
        let mut i = 0;
        while i + 1 < chunk.len() {
            sum += u16::from_be_bytes([chunk[i], chunk[i + 1]]) as u32;
            i += 2;
        }
        if i < chunk.len() {
            sum += (chunk[i] as u32) << 8;
        }
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    let r = !(sum as u16);
    if r == 0 { 0xFFFF } else { r }
}

/// One unacknowledged segment in the send window. Data is not copied:
/// (off, len) index into the caller's buffer, which outlives the send call
struct InFlight {
    off: usize,
    len: usize,
    seq: u32, // sequence number of the first byte
    sent_ts: u64,
    resent: bool,
}

pub struct TcpSocket {
    pub state: TcpState,
    pub local_port: u16,
    pub remote_port: u16,
    pub remote_ip: [u8; 4],
    pub remote_mac: [u8; 6],
    pub local_ip: [u8; 4],
    pub local_mac: [u8; 6],
    pub seq: u32,
    pub ack: u32,
    pub rx_buf: [u8; RX_BUF],
    pub rx_len: usize,
    pub peer_closed: bool,
    pub peer_acked: u32,
    /// Peer's advertised receive window (from the last ACK)
    pub snd_wnd: u16,
    /// Peer's MSS from its SYN (536 if it sent no option)
    pub peer_mss: u16,
    /// Handle of this connection's queue in the RX demux
    pub rx_handle: Option<usize>,

    // RFC 6298 round-trip-time / retransmission-timeout state.
    // srtt/rttvar are 0 until the first RTT sample lands
    pub srtt_ticks:   u32,
    pub rttvar_ticks: u32,
    pub rto_ticks:    u32,
}

impl TcpSocket {
    pub fn new() -> Self {
        Self {
            state: TcpState::Closed,
            local_port: 0,
            remote_port: 0,
            remote_ip: [0; 4],
            remote_mac: [0; 6],
            local_ip: [0; 4],
            local_mac: [0; 6],
            seq: random_isn(),
            ack: 0,
            rx_buf: [0; RX_BUF],
            rx_len: 0,
            peer_closed: false,
            peer_acked: 0,
            snd_wnd: 8192,
            peer_mss: 536,
            rx_handle: None,
            srtt_ticks: 0,
            rttvar_ticks: 0,
            rto_ticks: INIT_RTO_TICKS,
        }
    }

    /// Update SRTT, RTTVAR and RTO from a fresh round-trip-time sample
    /// per RFC 6298 §2 with the standard alpha=1/8, beta=1/4 weights
    fn update_rtt(&mut self, r_ticks: u32) {
        let r = r_ticks.max(1);
        if self.srtt_ticks == 0 {
            // First measurement: SRTT = R, RTTVAR = R/2
            self.srtt_ticks = r;
            self.rttvar_ticks = r / 2;
        } else {
            // RTTVAR = 3/4 RTTVAR + 1/4 |SRTT - R|
            // SRTT   = 7/8 SRTT   + 1/8 R
            let diff = if r > self.srtt_ticks { r - self.srtt_ticks } else { self.srtt_ticks - r };
            self.rttvar_ticks = (self.rttvar_ticks - (self.rttvar_ticks >> 2))
                .saturating_add(diff >> 2);
            self.srtt_ticks = (self.srtt_ticks - (self.srtt_ticks >> 3))
                .saturating_add(r >> 3);
        }
        // RTO = SRTT + max(G, 4 * RTTVAR), G = 1 tick (clock granularity)
        let k_rttvar = self.rttvar_ticks.saturating_mul(4).max(1);
        let rto = self.srtt_ticks.saturating_add(k_rttvar);
        self.rto_ticks = rto.clamp(MIN_RTO_TICKS, MAX_RTO_TICKS);
    }

    /// Transmit one segment with an explicit sequence number (used both for
    /// fresh sends via self.seq and for retransmits of older data)
    pub(crate) fn send_segment_at(&self, seq: u32, flags: u8, opts: &[u8], payload: &[u8]) {
        let mut tcp_buf = [0u8; 1480];
        let window = (RX_BUF - self.rx_len).min(65535) as u16;
        let tcp_len = build_opts(
            self.local_port, self.remote_port,
            seq, self.ack, flags, window,
            opts, payload, &self.local_ip, &self.remote_ip,
            &mut tcp_buf,
        );
        if tcp_len == 0 { return; }

        let mut ip_buf = [0u8; 1500];
        let ip_len = ipv4::build(
            &self.local_ip, &self.remote_ip,
            ipv4::PROTO_TCP, &tcp_buf[..tcp_len],
            &mut ip_buf,
        );
        if ip_len == 0 { return; }

        let mut eth_buf = [0u8; 1520];
        let eth_len = EthFrame::build(
            &self.remote_mac, &self.local_mac,
            ETHERTYPE_IP, &ip_buf[..ip_len],
            &mut eth_buf,
        );
        if eth_len == 0 { return; }

        let mut state = super::NET.lock();
        if let Some(drv) = state.driver.as_mut() {
            drv.send(&eth_buf[..eth_len]);
        }
        state.tx_count += 1;
    }

    fn send_segment(&self, flags: u8, payload: &[u8]) {
        self.send_segment_at(self.seq, flags, &[], payload);
    }

    pub fn recv_one_into(&mut self, buf: &mut [u8], buf_len: &mut usize) {
        self.recv_one();
        if self.rx_len > 0 {
            let space = buf.len() - *buf_len;
            let take = self.rx_len.min(space);
            if take > 0 {
                buf[*buf_len..*buf_len + take].copy_from_slice(&self.rx_buf[..take]);
                *buf_len += take;
                self.rx_buf.copy_within(take..self.rx_len, 0);
                self.rx_len -= take;
            }
        }
    }

    /// Pump the demux and drain this connection's queue through the state
    /// machine. Returns true if at least one segment was processed
    pub fn recv_one(&mut self) -> bool {
        rx::pump();
        let handle = match self.rx_handle {
            Some(h) => h,
            None => return false,
        };

        let mut got = false;
        while let Some(pkt) = rx::tcp_take(handle) {
            got = true;

            if pkt.seg.flags & FLAG_RST != 0 {
                self.state = TcpState::Closed;
                self.peer_closed = true;
                continue;
            }
            if pkt.seg.flags & FLAG_SYN != 0 {
                self.peer_mss = pkt.syn_mss;
            }
            self.dispatch_segment(&pkt.seg, &pkt.payload);
        }
        got
    }

    fn dispatch_segment(&mut self, seg: &TcpSegment, data: &[u8]) {
        // Every ACK refreshes the peer's advertised window
        if seg.flags & FLAG_ACK != 0 {
            self.snd_wnd = seg.window;
        }

        match self.state {
            TcpState::SynSent => {
                if seg.flags & (FLAG_SYN | FLAG_ACK) == (FLAG_SYN | FLAG_ACK) {
                    self.ack = seg.seq.wrapping_add(1);
                    self.seq = seg.ack;
                    self.peer_acked = seg.ack;
                    self.state = TcpState::Established;
                    self.send_segment(FLAG_ACK, &[]);
                }
            }
            TcpState::SynReceived => {
                // Server side: waiting for the ACK that completes the
                // three-way handshake (our SYN-ACK consumed one seq)
                if seg.flags & FLAG_ACK != 0 && seg.ack == self.seq {
                    self.peer_acked = seg.ack;
                    self.state = TcpState::Established;
                    // The handshake ACK may already carry data
                    if !data.is_empty() || seg.flags & FLAG_FIN != 0 {
                        self.dispatch_segment(seg, data);
                    }
                }
            }
            TcpState::Established | TcpState::FinWait1 | TcpState::FinWait2 => {
                if seg.flags & FLAG_ACK != 0 && wrapping_ge(seg.ack, self.peer_acked) {
                    self.peer_acked = seg.ack;
                }

                let mut send_ack = false;

                if !data.is_empty() {
                    if seg.seq == self.ack {
                        let space = RX_BUF - self.rx_len;
                        let copy = data.len().min(space);
                        if copy > 0 {
                            self.rx_buf[self.rx_len..self.rx_len + copy].copy_from_slice(&data[..copy]);
                            self.rx_len += copy;
                            self.ack = self.ack.wrapping_add(copy as u32);
                        }
                        send_ack = true;
                    } else {
                        // Out of order or duplicate: dup-ACK so the peer's
                        // fast retransmit kicks in
                        send_ack = true;
                    }
                }

                if seg.flags & FLAG_FIN != 0 {
                    if seg.seq == self.ack {
                        self.ack = self.ack.wrapping_add(1);
                        self.peer_closed = true;
                        send_ack = true;
                        if self.state == TcpState::Established {
                            self.state = TcpState::CloseWait;
                        } else {
                            self.state = TcpState::Closed;
                        }
                    } else {
                        send_ack = true;
                    }
                }

                if send_ack {
                    self.send_segment(FLAG_ACK, &[]);
                }
            }
            TcpState::CloseWait => {
                if seg.flags & FLAG_ACK != 0 && wrapping_ge(seg.ack, self.peer_acked) {
                    self.peer_acked = seg.ack;
                }
            }
            TcpState::LastAck => {
                if seg.flags & FLAG_ACK != 0 {
                    self.state = TcpState::Closed;
                }
            }
            _ => {}
        }
    }

    pub fn connect(remote_ip: [u8; 4], remote_port: u16) -> Option<Self> {
        let local_ip = super::get_ip();
        let local_mac = super::get_mac();

        let remote_mac = super::resolve_arp(&remote_ip, &local_ip, &local_mac)?;

        // Ephemeral port; walk forward past any tuple already in use
        let mut local_port = 49152 + (crate::fs::procfs::uptime_ticks() as u16 & 0x3FFF);
        while rx::tcp_tuple_in_use(local_port, remote_ip, remote_port) {
            local_port = local_port.wrapping_add(1).max(49152);
        }

        let mut sock = TcpSocket::new();
        sock.local_ip = local_ip;
        sock.local_mac = local_mac;
        sock.remote_ip = remote_ip;
        sock.remote_mac = remote_mac;
        sock.remote_port = remote_port;
        sock.local_port = local_port;

        // Register with the demux BEFORE the SYN goes out, or the SYN-ACK
        // can race us and get dropped
        sock.rx_handle = rx::register_tcp(local_port, remote_ip, remote_port);
        sock.rx_handle?;

        sock.send_segment_at(sock.seq, FLAG_SYN, &mss_opts(), &[]);
        sock.seq = sock.seq.wrapping_add(1);
        sock.state = TcpState::SynSent;

        let start = crate::fs::procfs::uptime_ticks();
        let mut syn_retries = 0u32;
        let mut syn_dl = start.wrapping_add(INIT_RTO_TICKS as u64);
        loop {
            if CTRL_C.load(Ordering::SeqCst) { return None; }
            sock.recv_one();
            if sock.state == TcpState::Established {
                return Some(sock);
            }
            if sock.state == TcpState::Closed {
                return None;
            }
            let now = crate::fs::procfs::uptime_ticks();
            if now >= syn_dl && syn_retries < 3 {
                syn_retries += 1;
                sock.send_segment_at(sock.seq.wrapping_sub(1), FLAG_SYN, &mss_opts(), &[]);
                syn_dl = now.wrapping_add((INIT_RTO_TICKS as u64) << syn_retries);
            }
            if now.wrapping_sub(start) >= 2500 {
                return None; // 10 s connect timeout
            }
            crate::sched::yield_now();
        }
    }

    /// Send all of 'data' with a sliding window: up to SND_WND_SEGS segments
    /// (bounded by the peer's advertised window) may be in flight at once.
    /// Retransmission is go-back-oldest with RFC 6298 backoff plus a
    /// 3-dup-ACK fast retransmit
    pub fn send(&mut self, data: &[u8]) -> bool {
        if self.state != TcpState::Established {
            return false;
        }

        let mss = (self.peer_mss as usize).clamp(536, MAX_SEG);
        let base_seq = self.seq;
        let mut inflight: VecDeque<InFlight> = VecDeque::new();
        let mut next_off = 0usize;
        let mut last_ack_seen = self.peer_acked;
        let mut dup_acks = 0u32;
        let mut oldest_retries = 0usize;

        loop {
            if CTRL_C.load(Ordering::SeqCst) { return false; }
            if self.state == TcpState::Closed { return false; }

            // Fill the window
            while next_off < data.len() && inflight.len() < SND_WND_SEGS {
                let bytes_out = self.seq.wrapping_sub(self.peer_acked) as usize;
                let wnd = self.snd_wnd as usize;
                // Always allow one segment when nothing is in flight, which
                // doubles as a zero-window probe
                if !inflight.is_empty() && bytes_out + mss > wnd {
                    break;
                }
                let len = mss.min(data.len() - next_off);
                let seq = self.seq;
                self.send_segment_at(seq, FLAG_PSH | FLAG_ACK, &[], &data[next_off..next_off + len]);
                inflight.push_back(InFlight {
                    off: next_off,
                    len,
                    seq,
                    sent_ts: crate::fs::procfs::uptime_ticks(),
                    resent: false,
                });
                self.seq = seq.wrapping_add(len as u32);
                next_off += len;
            }

            if inflight.is_empty() && next_off >= data.len() {
                return true;
            }

            let got = self.recv_one();

            // Release everything the peer has cumulatively acknowledged
            let mut progressed = false;
            while let Some(front) = inflight.front() {
                let end_seq = front.seq.wrapping_add(front.len as u32);
                if wrapping_ge(self.peer_acked, end_seq) {
                    // Karn's algorithm: skip the RTT sample if this segment
                    // was retransmitted (the ACK is ambiguous)
                    if !front.resent {
                        let now = crate::fs::procfs::uptime_ticks();
                        let r = now.wrapping_sub(front.sent_ts).min(u32::MAX as u64) as u32;
                        self.update_rtt(r);
                    }
                    inflight.pop_front();
                    progressed = true;
                } else {
                    break;
                }
            }
            if progressed {
                oldest_retries = 0;
                dup_acks = 0;
                last_ack_seen = self.peer_acked;
            } else if got && self.peer_acked == last_ack_seen && !inflight.is_empty() {
                dup_acks += 1;
            }

            // Fast retransmit after 3 duplicate ACKs
            if dup_acks >= 3 {
                if let Some(front) = inflight.front_mut() {
                    crate::log!("tcp: fast retransmit seq={}", front.seq);
                    let (seq, off, len) = (front.seq, front.off, front.len);
                    front.resent = true;
                    front.sent_ts = crate::fs::procfs::uptime_ticks();
                    self.send_segment_at(seq, FLAG_PSH | FLAG_ACK, &[], &data[off..off + len]);
                }
                dup_acks = 0;
            }

            // RTO expiry on the oldest in-flight segment
            if let Some(front) = inflight.front_mut() {
                let now = crate::fs::procfs::uptime_ticks();
                if now.wrapping_sub(front.sent_ts) >= self.rto_ticks as u64 {
                    oldest_retries += 1;
                    if oldest_retries > MAX_RETRIES {
                        crate::log_err!("tcp: max retransmits reached, closing");
                        self.state = TcpState::Closed;
                        // Roll seq back so the caller sees a consistent count
                        self.seq = base_seq.wrapping_add(next_off as u32);
                        return false;
                    }
                    crate::log!("tcp: retransmit #{} seq={} rto={}t",
                        oldest_retries, front.seq, self.rto_ticks);
                    let (seq, off, len) = (front.seq, front.off, front.len);
                    front.resent = true;
                    front.sent_ts = now;
                    self.send_segment_at(seq, FLAG_PSH | FLAG_ACK, &[], &data[off..off + len]);
                    // RFC 6298 §5.5: exponential backoff on each retransmit
                    self.rto_ticks = self.rto_ticks.saturating_mul(2).min(MAX_RTO_TICKS);
                }
            }

            crate::sched::yield_now();
        }
    }

    pub fn recv_wait(&mut self, _timeout_iters: usize) -> &[u8] {
        let max_ticks = 1000u64;
        let start = crate::fs::procfs::uptime_ticks();
        loop {
            if CTRL_C.load(Ordering::SeqCst) { break; }
            self.recv_one();
            if self.rx_len > 0 || self.peer_closed {
                break;
            }
            if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= max_ticks {
                break;
            }
            crate::sched::yield_now();
        }
        &self.rx_buf[..self.rx_len]
    }

    pub fn recv_all(&mut self, _timeout_iters: usize) -> &[u8] {
        let max_ticks = 1000u64;
        let mut start = crate::fs::procfs::uptime_ticks();
        loop {
            if CTRL_C.load(Ordering::SeqCst) { break; }
            let prev = self.rx_len;
            self.recv_one();
            if self.peer_closed { break; }
            if self.rx_len != prev {
                start = crate::fs::procfs::uptime_ticks();
            } else if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= max_ticks {
                break;
            }
            crate::sched::yield_now();
        }
        &self.rx_buf[..self.rx_len]
    }

    pub fn close(&mut self) {
        if self.state == TcpState::Established {
            self.send_segment(FLAG_FIN | FLAG_ACK, &[]);
            self.seq = self.seq.wrapping_add(1);
            self.state = TcpState::FinWait1;
            let start = crate::fs::procfs::uptime_ticks();
            loop {
                self.recv_one();
                if self.state == TcpState::Closed { break; }
                if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= 500 { break; }
                crate::sched::yield_now();
            }
        } else if self.state == TcpState::CloseWait {
            self.send_segment(FLAG_FIN | FLAG_ACK, &[]);
            self.seq = self.seq.wrapping_add(1);
            self.state = TcpState::LastAck;
            let start = crate::fs::procfs::uptime_ticks();
            loop {
                self.recv_one();
                if self.state == TcpState::Closed { break; }
                if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= 500 { break; }
                crate::sched::yield_now();
            }
        }
        self.state = TcpState::Closed;
        if let Some(h) = self.rx_handle.take() {
            rx::unregister_tcp(h);
        }
    }

    pub fn is_connected(&self) -> bool {
        self.state == TcpState::Established || self.state == TcpState::CloseWait
    }
}

impl Drop for TcpSocket {
    fn drop(&mut self) {
        if let Some(h) = self.rx_handle.take() {
            rx::unregister_tcp(h);
        }
    }
}
