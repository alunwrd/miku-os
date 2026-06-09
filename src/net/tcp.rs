use core::sync::atomic::Ordering;
use super::CTRL_C;
use super::eth::{EthFrame, ETHERTYPE_ARP, ETHERTYPE_IP};
use super::ipv4;

pub const FLAG_FIN: u8 = 0x01;
pub const FLAG_SYN: u8 = 0x02;
pub const FLAG_RST: u8 = 0x04;
pub const FLAG_PSH: u8 = 0x08;
pub const FLAG_ACK: u8 = 0x10;

const MAX_RETRIES: usize = 5;
const RX_BUF: usize = 32768;

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

fn random_isn() -> u32 {
    let tsc: u64;
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem));
        tsc = ((hi as u64) << 32) | lo as u64;
    }
    let up = crate::vfs::procfs::uptime_ticks() as u64;
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
    let tcp_len = 20 + payload.len();
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
    out[12] = 0x50;
    out[13] = flags;
    out[14] = (window >> 8) as u8;
    out[15] = window as u8;
    out[16] = 0;
    out[17] = 0;
    out[18] = 0;
    out[19] = 0;

    if !payload.is_empty() {
        out[20..tcp_len].copy_from_slice(payload);
    }

    let csum = checksum(src_ip, dst_ip, &out[..tcp_len]);
    out[16] = (csum >> 8) as u8;
    out[17] = csum as u8;

    tcp_len
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
    pub last_sent_payload: [u8; 1400],
    pub last_sent_len: usize,
    pub last_sent_flags: u8,
    pub retransmit_seq: u32,
    pub retransmit_dl: u64,
    pub retransmit_count: usize,
    pub peer_acked: u32,

    // RFC 6298 round-trip-time / retransmission-timeout state.
    // srtt/rttvar are 0 until the first RTT sample lands. send_ts records
    // when the currently-pending segment was first transmitted, so the
    // ACK path can compute R = now - send_ts. Karn's algorithm: if the
    // pending segment was retransmitted we skip the sample (the ACK is
    // ambiguous about which copy it acknowledges)
    pub srtt_ticks:      u32,
    pub rttvar_ticks:    u32,
    pub rto_ticks:       u32,
    pub send_ts:         u64,
    pub send_was_resend: bool,
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
            last_sent_payload: [0u8; 1400],
            last_sent_len: 0,
            last_sent_flags: 0,
            retransmit_seq: 0,
            retransmit_dl: 0,
            retransmit_count: 0,
            peer_acked: 0,
            srtt_ticks: 0,
            rttvar_ticks: 0,
            rto_ticks: INIT_RTO_TICKS,
            send_ts: 0,
            send_was_resend: false,
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

    fn send_segment(&self, flags: u8, payload: &[u8]) {
        let mut tcp_buf = [0u8; 1480];
        let window = (RX_BUF - self.rx_len).min(65535) as u16;
        let tcp_len = build(
            self.local_port, self.remote_port,
            self.seq, self.ack, flags, window,
            payload, &self.local_ip, &self.remote_ip,
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

    fn recv_one(&mut self) -> bool {
        let mut raw: [[u8; 1520]; 4] = [[0; 1520]; 4];
        let mut raw_lens = [0usize; 4];
        let mut raw_n = 0usize;

        {
            let mut state = super::NET.lock();
            if let Some(drv) = state.driver.as_mut() {
                drv.recv(&mut |buf| {
                    if raw_n < 4 {
                        let l = buf.len().min(1520);
                        raw[raw_n][..l].copy_from_slice(&buf[..l]);
                        raw_lens[raw_n] = l;
                        raw_n += 1;
                    }
                });
            }
            state.rx_count += raw_n as u64;
        }

        let mut got_tcp = false;

        for i in 0..raw_n {
            let buf = &raw[i][..raw_lens[i]];
            let frame = match EthFrame::parse(buf) {
                Some(f) => f,
                None => continue,
            };

            if frame.ethertype == ETHERTYPE_ARP {
                let mut state = super::NET.lock();
                let mc = state.mac;
                let ic = state.ip;
                let mut rep = [0u8; 64];
                let rlen = super::arp::handle(&frame, &mc, &ic, &mut state.arp, &mut rep);
                if rlen > 0 {
                    let rc = rep;
                    if let Some(drv) = state.driver.as_mut() {
                        drv.send(&rc[..rlen]);
                    }
                }
                continue;
            }

            if frame.ethertype != ETHERTYPE_IP { continue; }

            let ip = match ipv4::Ipv4Header::parse(frame.payload) {
                Some(h) => h,
                None => continue,
            };
            if ip.proto != ipv4::PROTO_TCP { continue; }
            if ip.src != self.remote_ip { continue; }

            let tcp_payload = ip.payload(frame.payload);
            let seg = match TcpSegment::parse(tcp_payload) {
                Some(s) => s,
                None => continue,
            };

            if seg.src_port != self.remote_port { continue; }
            if seg.dst_port != self.local_port { continue; }

            if seg.flags & FLAG_RST != 0 {
                self.state = TcpState::Closed;
                self.peer_closed = true;
                got_tcp = true;
                continue;
            }

            got_tcp = true;
            self.dispatch_segment(&seg, seg.payload(tcp_payload));
        }

        got_tcp
    }

    fn dispatch_segment(&mut self, seg: &TcpSegment, data: &[u8]) {
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
            TcpState::Established | TcpState::FinWait1 | TcpState::FinWait2 => {
                if seg.flags & FLAG_ACK != 0 {
                    if self.peer_acked == seg.ack || wrapping_gt(seg.ack, self.peer_acked) {
                        self.peer_acked = seg.ack;
                    }
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
                    } else if wrapping_gt(self.ack, seg.seq) {
                        send_ack = true;
                    } else {
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
                if seg.flags & FLAG_ACK != 0 {
                    if self.peer_acked == seg.ack || wrapping_gt(seg.ack, self.peer_acked) {
                        self.peer_acked = seg.ack;
                    }
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

        let local_port = 49152 + (crate::vfs::procfs::uptime_ticks() as u16 & 0x3FFF);

        let mut sock = TcpSocket::new();
        sock.local_ip = local_ip;
        sock.local_mac = local_mac;
        sock.remote_ip = remote_ip;
        sock.remote_mac = remote_mac;
        sock.remote_port = remote_port;
        sock.local_port = local_port;

        sock.send_segment(FLAG_SYN, &[]);
        sock.seq = sock.seq.wrapping_add(1);
        sock.state = TcpState::SynSent;

        let start = crate::vfs::procfs::uptime_ticks();
        loop {
            if CTRL_C.load(Ordering::SeqCst) { return None; }
            sock.recv_one();
            if sock.state == TcpState::Established {
                return Some(sock);
            }
            if sock.state == TcpState::Closed {
                return None;
            }
            if crate::vfs::procfs::uptime_ticks().wrapping_sub(start) >= 1000 {
                return None;
            }
            core::hint::spin_loop();
        }
    }

    pub fn send(&mut self, data: &[u8]) -> bool {
        if self.state != TcpState::Established {
            return false;
        }

        const CHUNK: usize = 1400;
        let mut offset = 0;

        while offset < data.len() {
            if CTRL_C.load(Ordering::SeqCst) { return false; }

            let end = (offset + CHUNK).min(data.len());
            let chunk = &data[offset..end];
            let flags = FLAG_PSH | FLAG_ACK;

            let sent_seq = self.seq;

            self.last_sent_len = chunk.len().min(1400);
            self.last_sent_payload[..self.last_sent_len].copy_from_slice(&chunk[..self.last_sent_len]);
            self.last_sent_flags = flags;
            self.retransmit_seq = sent_seq;
            // Record the first-transmission timestamp so the ACK path can
            // sample R = now - send_ts. Karn flag stays false until a
            // retransmit happens (then we skip the RTT sample)
            self.send_ts = crate::vfs::procfs::uptime_ticks();
            self.send_was_resend = false;
            self.retransmit_dl = self.send_ts.wrapping_add(self.rto_ticks as u64);
            self.retransmit_count = 0;

            self.send_segment(flags, chunk);
            self.seq = self.seq.wrapping_add(chunk.len() as u32);

            let expected_ack = self.seq;
            if !self.wait_for_ack(expected_ack) {
                return false;
            }

            offset = end;
        }
        true
    }

    fn wait_for_ack(&mut self, expected_ack: u32) -> bool {
        for _attempt in 0..MAX_RETRIES {
            loop {
                if CTRL_C.load(Ordering::SeqCst) { return false; }

                self.recv_one();

                if self.state == TcpState::Closed { return false; }

                if self.peer_acked == expected_ack || wrapping_gt(self.peer_acked, expected_ack) {
                    // Karn's algorithm: only sample RTT if the acknowledged
                    // segment was sent exactly once. Ambiguous ACKs after a
                    // retransmit would otherwise corrupt SRTT/RTTVAR
                    if !self.send_was_resend {
                        let now = crate::vfs::procfs::uptime_ticks();
                        let r = now.wrapping_sub(self.send_ts).min(u32::MAX as u64) as u32;
                        self.update_rtt(r);
                    }
                    return true;
                }

                if crate::vfs::procfs::uptime_ticks() >= self.retransmit_dl { break; }

                core::hint::spin_loop();
            }

            self.retransmit_count += 1;
            crate::log!("tcp: retransmit #{} seq={} rto={}t",
                self.retransmit_count, self.retransmit_seq, self.rto_ticks);

            self.seq = self.retransmit_seq;

            let mut tmp = [0u8; 1400];
            tmp[..self.last_sent_len].copy_from_slice(&self.last_sent_payload[..self.last_sent_len]);

            self.send_segment(self.last_sent_flags, &tmp[..self.last_sent_len]);
            self.seq = self.seq.wrapping_add(self.last_sent_len as u32);
            // RFC 6298 §5.5: exponential backoff on each retransmit
            self.rto_ticks = self.rto_ticks.saturating_mul(2).min(MAX_RTO_TICKS);
            self.send_was_resend = true;
            self.retransmit_dl = crate::vfs::procfs::uptime_ticks()
                .wrapping_add(self.rto_ticks as u64);
        }

        crate::log_err!("tcp: max retransmits reached, closing");
        self.state = TcpState::Closed;
        false
    }

    pub fn recv_wait(&mut self, _timeout_iters: usize) -> &[u8] {
        let max_ticks = 1000u64;
        let start = crate::vfs::procfs::uptime_ticks();
        loop {
            if CTRL_C.load(Ordering::SeqCst) { break; }
            self.recv_one();
            if self.rx_len > 0 || self.peer_closed {
                break;
            }
            if crate::vfs::procfs::uptime_ticks().wrapping_sub(start) >= max_ticks {
                break;
            }
            core::hint::spin_loop();
        }
        &self.rx_buf[..self.rx_len]
    }

    pub fn recv_all(&mut self, _timeout_iters: usize) -> &[u8] {
        let max_ticks = 1000u64;
        let mut start = crate::vfs::procfs::uptime_ticks();
        loop {
            if CTRL_C.load(Ordering::SeqCst) { break; }
            let prev = self.rx_len;
            self.recv_one();
            if self.peer_closed { break; }
            if self.rx_len != prev {
                start = crate::vfs::procfs::uptime_ticks();
            } else {
                if crate::vfs::procfs::uptime_ticks().wrapping_sub(start) >= max_ticks {
                    break;
                }
            }
            core::hint::spin_loop();
        }
        &self.rx_buf[..self.rx_len]
    }

    pub fn close(&mut self) {
        if self.state == TcpState::Established {
            self.send_segment(FLAG_FIN | FLAG_ACK, &[]);
            self.seq = self.seq.wrapping_add(1);
            self.state = TcpState::FinWait1;
            let start = crate::vfs::procfs::uptime_ticks();
            loop {
                self.recv_one();
                if self.state == TcpState::Closed { break; }
                if crate::vfs::procfs::uptime_ticks().wrapping_sub(start) >= 500 { break; }
                core::hint::spin_loop();
            }
        } else if self.state == TcpState::CloseWait {
            self.send_segment(FLAG_FIN | FLAG_ACK, &[]);
            self.seq = self.seq.wrapping_add(1);
            self.state = TcpState::LastAck;
            let start = crate::vfs::procfs::uptime_ticks();
            loop {
                self.recv_one();
                if self.state == TcpState::Closed { break; }
                if crate::vfs::procfs::uptime_ticks().wrapping_sub(start) >= 500 { break; }
                core::hint::spin_loop();
            }
        }
        self.state = TcpState::Closed;
    }

    pub fn is_connected(&self) -> bool {
        self.state == TcpState::Established || self.state == TcpState::CloseWait
    }
}
