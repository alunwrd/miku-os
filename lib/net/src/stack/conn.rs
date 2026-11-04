// One TCP connection, as a state machine with no network access
//
// Two entry points, `on_segment` and `on_tick`, so netd can wait on the card
// and all clients in one `poll`.
//
// The state machine emits segments as `Vec<u8>` in an outbox. A test can face
// two of these with no card, no driver, and a clock that is a number.
//
// Field names follow RFC 793: `snd_una` is the oldest unacknowledged byte sent,
// `snd_nxt` the next to send, `rcv_nxt` the next expected.

extern crate alloc;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::stack::tcp::{
    self, seq_ge, seq_gt, seq_lt, SynOptions, TcpSegment, TcpState, FLAG_ACK, FLAG_FIN, FLAG_PSH,
    FLAG_RST, FLAG_SYN,
};

/// Received data held before the window closes, in and out of order together.
/// See `dev/net-architecture.md`
pub const RX_CAP: usize = 256 * 1024;
/// Application data queued and not yet acknowledged
pub const TX_CAP: usize = 256 * 1024;

/// Window shift our SYN offers (RFC 7323): the smallest that fits `RX_CAP` in
/// sixteen bits
pub const OUR_WSCALE: u8 = {
    let mut s = 0u8;
    while (RX_CAP >> s) > u16::MAX as usize {
        s += 1;
    }
    s
};

/// The congestion window a connection starts with, in segments (RFC 6928)
pub const INIT_CWND_SEGS: usize = 10;

/// SACK blocks per ACK. Four fit with no other options; Linux sends three with
/// room for its timestamp
const MAX_SACK_BLOCKS: usize = 3;

/// Retransmissions of one segment before the connection is dead
pub const MAX_RETRIES: usize = 5;
/// SYNs sent before `connect` fails
pub const MAX_SYN_RETRIES: u32 = 3;

// RFC 6298 bounds: one second before any measured round trip, a 250 ms floor
// once SRTT settles (as Linux), a minute at the top
pub const INIT_RTO_MS: u32 = 1_000;
pub const MIN_RTO_MS: u32 = 250;
pub const MAX_RTO_MS: u32 = 60_000;

/// The smallest MSS a peer may be assumed to accept when it advertises none
const DEFAULT_PEER_MSS: u16 = 536;

/// One unacknowledged run of bytes. The data stays in `snd_buf`; this records
/// where it was and when it went
#[derive(Clone, Copy)]
struct InFlight {
    seq: u32,
    len: usize,
    sent_ms: u64,
    /// Karn's algorithm: an ACK of a segment sent twice gives no round-trip
    /// sample
    resent: bool,
    /// Covered by a SACK block. Not acknowledged, since the peer may renege,
    /// but not resent in recovery
    sacked: bool,
    /// Sent again in the current recovery already
    rexmitted: bool,
}

/// `SO_KEEPALIVE` and the three `TCP_KEEP*` numbers.
///
/// A connection silent for `idle_ms` gets an empty ACK one byte behind
/// `snd_nxt`, which a live peer must answer (RFC 1122 4.2.3.6). `count`
/// unanswered probes `interval_ms` apart end the connection
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Keepalive {
    pub enabled: bool,
    pub idle_ms: u64,
    pub interval_ms: u64,
    pub count: u32,
}

impl Keepalive {
    /// Off, with Linux's defaults: two hours of silence, then nine probes 75
    /// seconds apart
    pub const DEFAULT: Keepalive =
        Keepalive { enabled: false, idle_ms: 7_200_000, interval_ms: 75_000, count: 9 };
}

impl Default for Keepalive {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Why a connection ended, when it ended badly
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Failure {
    /// The peer refused or tore down the connection
    Reset,
    /// The handshake got no answer
    SynTimeout,
    /// The same segment went out `MAX_RETRIES` times unanswered
    RetransmitLimit,
    /// Every keepalive probe went unanswered
    KeepaliveTimeout,
}

pub struct Connection {
    pub state: TcpState,
    pub local_ip: [u8; 4],
    pub remote_ip: [u8; 4],
    pub local_port: u16,
    pub remote_port: u16,

    /// Oldest byte sent and not acknowledged; also the sequence number of
    /// `snd_buf[0]`
    snd_una: u32,
    /// Next sequence number we will put on the wire
    snd_nxt: u32,
    /// Next sequence number we expect from the peer
    rcv_nxt: u32,
    /// Highest sequence number ever sent. A timeout rewinds `snd_nxt` below
    /// this, and an ACK of earlier bytes still counts
    snd_max: u32,
    /// The peer's receive window, from its last ACK, already scaled
    snd_wnd: u32,
    /// The peer's MSS from its SYN
    peer_mss: u16,
    /// Window shifts, 0 unless both SYNs carried one. The peer's applies to
    /// windows it sends, ours to windows we send
    snd_wscale: u8,
    rcv_wscale: u8,
    /// Both SYNs said SACK-permitted
    sack_ok: bool,

    /// Congestion control: NewReno (RFC 5681, 6582), in bytes
    cwnd: usize,
    ssthresh: usize,
    in_recovery: bool,
    /// `snd_max` when recovery began; an ACK reaching it ends recovery
    recover: u32,
    /// Highest sequence number covered by a SACK block; runs below it are holes
    high_sacked: u32,

    /// Application bytes, not all yet acknowledged
    snd_buf: VecDeque<u8>,
    inflight: VecDeque<InFlight>,
    dup_acks: u32,
    oldest_retries: usize,
    last_ack_seen: u32,
    /// A FIN is queued behind whatever is still in `snd_buf`
    fin_queued: bool,
    /// And sent, so `snd_nxt` cannot be rewound past it
    fin_sent: bool,

    /// Received in order, not yet read
    rx: VecDeque<u8>,
    /// Runs past the gap, held until it closes: sorted by distance from
    /// `rcv_nxt`, merged where they touch, bounded by the window
    ooo: Vec<(u32, Vec<u8>)>,
    ooo_bytes: usize,
    /// Start of the latest out-of-order segment, whose run is the first SACK
    /// block (RFC 2018 4)
    last_ooo_seq: u32,
    pub peer_closed: bool,

    srtt_ms: u32,
    rttvar_ms: u32,
    rto_ms: u32,

    syn_retries: u32,
    /// When the next SYN or retransmission is due
    rexmit_at: u64,

    out: VecDeque<Vec<u8>>,
    pub failure: Option<Failure>,

    keepalive: Keepalive,
    /// When the peer last sent anything; idle time starts here
    last_heard_ms: u64,
    /// Probes sent since then
    probes: u32,
}

impl Connection {
    fn blank(
        local_ip: [u8; 4],
        local_port: u16,
        remote_ip: [u8; 4],
        remote_port: u16,
        isn: u32,
    ) -> Self {
        Self {
            state: TcpState::Closed,
            local_ip,
            remote_ip,
            local_port,
            remote_port,
            snd_una: isn,
            snd_nxt: isn,
            rcv_nxt: 0,
            snd_max: isn,
            snd_wnd: 8192,
            peer_mss: DEFAULT_PEER_MSS,
            snd_wscale: 0,
            rcv_wscale: 0,
            sack_ok: false,
            cwnd: INIT_CWND_SEGS * DEFAULT_PEER_MSS as usize,
            ssthresh: usize::MAX,
            in_recovery: false,
            recover: isn,
            high_sacked: isn,
            snd_buf: VecDeque::new(),
            inflight: VecDeque::new(),
            dup_acks: 0,
            oldest_retries: 0,
            last_ack_seen: isn,
            fin_queued: false,
            fin_sent: false,
            rx: VecDeque::new(),
            ooo: Vec::new(),
            ooo_bytes: 0,
            last_ooo_seq: 0,
            peer_closed: false,
            srtt_ms: 0,
            rttvar_ms: 0,
            rto_ms: INIT_RTO_MS,
            syn_retries: 0,
            rexmit_at: 0,
            out: VecDeque::new(),
            failure: None,
            keepalive: Keepalive::DEFAULT,
            last_heard_ms: 0,
            probes: 0,
        }
    }

    /// Opens a connection, emitting the SYN. The initial sequence number is a
    /// parameter: it must come from real entropy, and a test chooses it to
    /// exercise the wrap
    pub fn connect(
        local_ip: [u8; 4],
        local_port: u16,
        remote_ip: [u8; 4],
        remote_port: u16,
        isn: u32,
        now_ms: u64,
    ) -> Self {
        let mut c = Self::blank(local_ip, local_port, remote_ip, remote_port, isn);
        c.state = TcpState::SynSent;
        c.emit_syn();
        c.snd_nxt = isn.wrapping_add(1);
        c.snd_max = c.snd_nxt;
        c.rexmit_at = now_ms + INIT_RTO_MS as u64;
        c.last_heard_ms = now_ms;
        c
    }

    /// Answers an arrived SYN, emitting the SYN-ACK. Takes the SYN's parsed
    /// fields, as a listener's backlog holds them.
    ///
    /// `peer` is what the SYN's options said. A window shift and SACK are
    /// answered only when offered (RFC 7323 1.3, RFC 2018 2)
    #[allow(clippy::too_many_arguments)]
    pub fn accept(
        local_ip: [u8; 4],
        local_port: u16,
        remote_ip: [u8; 4],
        remote_port: u16,
        isn: u32,
        peer_seq: u32,
        peer_window: u16,
        peer: SynOptions,
        now_ms: u64,
    ) -> Self {
        let mut c = Self::blank(local_ip, local_port, remote_ip, remote_port, isn);
        c.rcv_nxt = peer_seq.wrapping_add(1);
        // A SYN's own window is never scaled
        c.snd_wnd = peer_window as u32;
        c.peer_mss = peer.mss.unwrap_or(DEFAULT_PEER_MSS);
        if let Some(shift) = peer.wscale {
            c.snd_wscale = shift;
            c.rcv_wscale = OUR_WSCALE;
        }
        c.sack_ok = peer.sack_permitted;
        c.state = TcpState::SynReceived;
        c.emit_syn();
        c.snd_nxt = isn.wrapping_add(1);
        c.snd_max = c.snd_nxt;
        c.rexmit_at = now_ms + INIT_RTO_MS as u64;
        c.last_heard_ms = now_ms;
        c
    }

    // what the application does

    /// Queues bytes to send. Returns how many were taken, fewer when the send
    /// buffer is full
    pub fn write(&mut self, data: &[u8]) -> usize {
        if self.fin_queued || !self.can_send() {
            return 0;
        }
        let room = TX_CAP.saturating_sub(self.snd_buf.len());
        let take = data.len().min(room);
        self.snd_buf.extend(&data[..take]);
        take
    }

    /// Takes received bytes, returning how many were copied
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let n = buf.len().min(self.rx.len());
        for (i, b) in self.rx.drain(..n).enumerate() {
            buf[i] = b;
        }
        n
    }

    pub fn rx_available(&self) -> usize {
        self.rx.len()
    }

    /// Bytes written, not yet acknowledged
    pub fn tx_pending(&self) -> usize {
        self.snd_buf.len()
    }

    /// Begins an orderly close. The FIN follows whatever is queued
    pub fn close(&mut self, now_ms: u64) {
        match self.state {
            TcpState::Established | TcpState::CloseWait => {
                self.fin_queued = true;
                self.pump(now_ms);
            }
            TcpState::SynSent => {
                self.state = TcpState::Closed;
            }
            _ => {}
        }
    }

    /// Tears the connection down, telling the peer
    pub fn reset(&mut self) {
        if self.state != TcpState::Closed {
            self.emit_at(self.snd_nxt, FLAG_RST | FLAG_ACK, &[], &[]);
        }
        self.state = TcpState::Closed;
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.state, TcpState::Established | TcpState::CloseWait)
    }

    fn can_send(&self) -> bool {
        matches!(self.state, TcpState::Established | TcpState::CloseWait)
    }

    /// The next segment to put on the wire, already built and checksummed
    pub fn take_segment(&mut self) -> Option<Vec<u8>> {
        self.out.pop_front()
    }

    pub fn has_output(&self) -> bool {
        !self.out.is_empty()
    }

    pub fn keepalive(&self) -> Keepalive {
        self.keepalive
    }

    /// Takes effect now: idle time counts from the last thing heard or this
    /// call, whichever is later, and sent probes are forgotten, as Linux does
    /// on `setsockopt`
    pub fn set_keepalive(&mut self, k: Keepalive, now_ms: u64) {
        self.keepalive = k;
        self.last_heard_ms = self.last_heard_ms.max(now_ms);
        self.probes = 0;
    }

    /// Current retransmission timeout, for a caller deciding how long to sleep
    pub fn rto_ms(&self) -> u32 {
        self.rto_ms
    }

    /// Window shifts in force (the peer's, ours); both 0 without scaling
    pub fn window_scales(&self) -> (u8, u8) {
        (self.snd_wscale, self.rcv_wscale)
    }

    pub fn sack_permitted(&self) -> bool {
        self.sack_ok
    }

    /// The congestion window, in bytes
    pub fn cwnd(&self) -> usize {
        self.cwnd
    }

    pub fn in_recovery(&self) -> bool {
        self.in_recovery
    }

    // the wire

    /// Room ahead of `rcv_nxt`. Unread data counts against it; out-of-order
    /// data does not, since it lies inside the window (RFC 793 3.7)
    fn rcv_space(&self) -> usize {
        RX_CAP.saturating_sub(self.rx.len())
    }

    /// Window field for a segment: in units of our shift, except in a SYN
    fn window_field(&self, syn: bool) -> u16 {
        let space = self.rcv_space();
        let shift = if syn { 0 } else { self.rcv_wscale };
        (space >> shift).min(u16::MAX as usize) as u16
    }

    fn mss(&self) -> usize {
        (self.peer_mss as usize).clamp(DEFAULT_PEER_MSS as usize, tcp::MAX_SEG)
    }

    /// Our SYN or SYN-ACK: the client offers a shift and SACK, the server
    /// answers what was offered
    fn emit_syn(&mut self) {
        let isn = self.snd_una;
        let (opts, n) = match self.state {
            TcpState::SynReceived => {
                tcp::syn_opts((self.rcv_wscale > 0).then_some(OUR_WSCALE), self.sack_ok)
            }
            _ => tcp::syn_opts(Some(OUR_WSCALE), true),
        };
        let flags = if self.state == TcpState::SynReceived { FLAG_SYN | FLAG_ACK } else { FLAG_SYN };
        self.emit_at(isn, flags, &opts[..n], &[]);
    }

    fn emit_at(&mut self, seq: u32, flags: u8, opts: &[u8], payload: &[u8]) {
        let mut buf = [0u8; 1500];
        let window = self.window_field(flags & FLAG_SYN != 0);
        let n = tcp::build_opts(
            self.local_port,
            self.remote_port,
            seq,
            self.rcv_nxt,
            flags,
            window,
            opts,
            payload,
            &self.local_ip,
            &self.remote_ip,
            &mut buf,
        );
        if n > 0 {
            self.out.push_back(Vec::from(&buf[..n]));
        }
    }

    /// A bare ACK, with SACK blocks for whatever is held out of order
    fn emit_ack(&mut self) {
        let mut blocks = [(0u32, 0u32); MAX_SACK_BLOCKS];
        let n = if self.sack_ok { self.sack_blocks(&mut blocks) } else { 0 };
        let (opts, len) = tcp::sack_opts(&blocks[..n]);
        self.emit_at(self.snd_nxt, FLAG_ACK, &opts[..len], &[]);
    }

    /// Out-of-order runs as SACK blocks: the latest arrival's first, then the
    /// rest from the lowest (RFC 2018 4)
    fn sack_blocks(&self, out: &mut [(u32, u32); MAX_SACK_BLOCKS]) -> usize {
        let mut n = 0;
        let latest = self.ooo.iter().position(|(s, d)| {
            seq_ge(self.last_ooo_seq, *s) && seq_lt(self.last_ooo_seq, s.wrapping_add(d.len() as u32))
        });
        if let Some(i) = latest {
            let (s, d) = &self.ooo[i];
            out[0] = (*s, s.wrapping_add(d.len() as u32));
            n = 1;
        }
        for (i, (s, d)) in self.ooo.iter().enumerate() {
            if n >= out.len() {
                break;
            }
            if Some(i) != latest {
                out[n] = (*s, s.wrapping_add(d.len() as u32));
                n += 1;
            }
        }
        n
    }

    // events

    /// One segment for this connection. The caller has checked that it belongs
    /// here and its checksum
    pub fn on_segment(&mut self, seg: &TcpSegment, data: &[u8], opts: &[u8], now_ms: u64) {
        // Any segment at all is proof of life, the answer to a probe included
        self.last_heard_ms = self.last_heard_ms.max(now_ms);
        self.probes = 0;

        if seg.has(FLAG_RST) {
            self.state = TcpState::Closed;
            self.peer_closed = true;
            if self.failure.is_none() {
                self.failure = Some(Failure::Reset);
            }
            return;
        }

        // A SYN-ACK's window is not scaled; every later one is
        if seg.has(FLAG_ACK) && !seg.has(FLAG_SYN) {
            self.snd_wnd = (seg.window as u32) << self.snd_wscale;
        }

        match self.state {
            TcpState::SynSent => self.on_syn_ack(seg, opts, now_ms),
            TcpState::SynReceived => {
                // Waiting for the handshake's final ACK; our SYN-ACK consumed
                // one sequence number
                if seg.has(FLAG_ACK) && seg.ack == self.snd_nxt {
                    self.snd_una = seg.ack;
                    self.state = TcpState::Established;
                    self.inflight.clear();
                    self.oldest_retries = 0;
                    self.start_congestion_control();
                    // That ACK may already be carrying data or a FIN
                    if !data.is_empty() || seg.has(FLAG_FIN) {
                        self.on_established(seg, data, opts, now_ms);
                    }
                }
            }
            TcpState::Established
            | TcpState::FinWait1
            | TcpState::FinWait2
            | TcpState::CloseWait => self.on_established(seg, data, opts, now_ms),
            TcpState::LastAck => {
                if seg.has(FLAG_ACK) {
                    self.state = TcpState::Closed;
                }
            }
            TcpState::Closed => {}
        }

        self.pump(now_ms);
    }

    fn on_syn_ack(&mut self, seg: &TcpSegment, opts: &[u8], now_ms: u64) {
        if !(seg.has(FLAG_SYN) && seg.has(FLAG_ACK)) {
            return;
        }
        if seg.ack != self.snd_nxt {
            // Answers a SYN we did not send. RFC 793 says to reset it; ignoring
            // it would wait out the connect timeout
            return;
        }
        self.rcv_nxt = seg.seq.wrapping_add(1);
        self.snd_una = seg.ack;
        let peer = tcp::parse_syn_options(opts);
        self.peer_mss = peer.mss.unwrap_or(DEFAULT_PEER_MSS);
        // Scaling requires a shift in both SYNs; ours always has one
        if let Some(shift) = peer.wscale {
            self.snd_wscale = shift;
            self.rcv_wscale = OUR_WSCALE;
        }
        self.sack_ok = peer.sack_permitted;
        self.snd_wnd = seg.window as u32;
        self.state = TcpState::Established;
        self.last_ack_seen = self.snd_una;
        self.oldest_retries = 0;
        self.start_congestion_control();

        // The SYN's round trip is the first RTT sample
        if self.syn_retries == 0 {
            let sent = self.rexmit_at.saturating_sub(INIT_RTO_MS as u64);
            self.update_rtt(now_ms.saturating_sub(sent) as u32);
        }
        self.emit_ack();
    }

    fn on_established(&mut self, seg: &TcpSegment, data: &[u8], opts: &[u8], now_ms: u64) {
        if seg.has(FLAG_ACK) {
            let pure = data.is_empty() && !seg.has(FLAG_FIN) && !seg.has(FLAG_SYN);
            self.on_ack(seg.ack, opts, pure, now_ms);
        }

        let mut send_ack = false;

        if !data.is_empty() {
            let end = seg.seq.wrapping_add(data.len() as u32);
            if !seq_gt(seg.seq, self.rcv_nxt) && seq_gt(end, self.rcv_nxt) {
                // At the gap, or a retransmission straddling it: the part past
                // `rcv_nxt` is new
                let skip = self.rcv_nxt.wrapping_sub(seg.seq) as usize;
                self.accept_data(&data[skip..]);
                // This segment may have been the gap
                self.drain_out_of_order();
            } else if seq_gt(seg.seq, self.rcv_nxt) {
                // Arrived early. Held, with a duplicate ACK so the peer's fast
                // retransmit fills the gap
                self.hold_out_of_order(seg.seq, data);
            }
            // Wholly behind the ack: a duplicate, answered by a plain ACK
            send_ack = true;
        } else if !seg.has(FLAG_FIN) && seq_gt(self.rcv_nxt, seg.seq) {
            // Empty and behind what we have: a keepalive probe. RFC 793 answers
            // every unacceptable segment with an ACK. Ours carry `snd_nxt`,
            // which the peer accepts, so two ACKs cannot answer each other
            send_ack = true;
        }

        if seg.has(FLAG_FIN) {
            // A FIN occupies the sequence number after the segment's data, so
            // its number is seg.seq + data.len(). Servers commonly set it on
            // the last data segment
            let fin_seq = seg.seq.wrapping_add(data.len() as u32);
            if fin_seq == self.rcv_nxt {
                self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                self.peer_closed = true;
                self.state = match self.state {
                    TcpState::Established => TcpState::CloseWait,
                    // Our FIN and theirs crossed, or ours was already answered
                    _ => TcpState::Closed,
                };
            }
            send_ack = true;
        }

        if send_ack {
            self.emit_ack();
        }
    }

    /// Handles an ACK. `pure` means nothing else in it; only those count as
    /// duplicates (RFC 5681 2)
    fn on_ack(&mut self, ack: u32, opts: &[u8], pure: bool, now_ms: u64) {
        if self.sack_ok {
            self.take_sack(opts);
        }
        if !seq_ge(ack, self.snd_una) || seq_gt(ack, self.snd_max) {
            // Acknowledges something unsent or already released
            return;
        }

        let advanced = ack != self.snd_una;
        if !advanced {
            if pure && !self.inflight.is_empty() {
                self.on_dup_ack(now_ms);
            }
            return;
        }

        let acked = ack.wrapping_sub(self.snd_una) as usize;
        self.snd_una = ack;
        // A timeout rewound `snd_nxt`; this acknowledges bytes sent before that
        if seq_gt(ack, self.snd_nxt) {
            self.snd_nxt = ack;
        }

        // `snd_buf[0]` is the byte at `snd_una`, so the buffer moves by exactly
        // what the ACK covered. A peer may acknowledge the middle of a segment
        let drop = acked.min(self.snd_buf.len());
        self.snd_buf.drain(..drop);

        // Releases every in-flight run now covered and trims the one partly
        // covered
        while let Some(front) = self.inflight.front().copied() {
            let end = front.seq.wrapping_add(front.len as u32);
            if seq_ge(self.snd_una, end) {
                if !front.resent {
                    self.update_rtt(now_ms.saturating_sub(front.sent_ms) as u32);
                }
                self.inflight.pop_front();
            } else if seq_gt(self.snd_una, front.seq) {
                let gone = self.snd_una.wrapping_sub(front.seq) as usize;
                if let Some(f) = self.inflight.front_mut() {
                    f.seq = self.snd_una;
                    f.len -= gone.min(f.len);
                }
                break;
            } else {
                break;
            }
        }

        self.oldest_retries = 0;
        self.dup_acks = 0;
        self.last_ack_seen = self.snd_una;
        // RFC 6298 5.3: a fresh ACK restarts the timer
        self.rexmit_at = now_ms + self.rto_ms as u64;

        let mss = self.mss();
        if self.in_recovery {
            if seq_ge(ack, self.recover) {
                // Everything outstanding at the loss is in. RFC 6582 3.2 step
                // 3, the first choice
                self.in_recovery = false;
                self.cwnd = self.ssthresh;
            } else {
                // Partial ACK: the next hole is known. Resend at once and
                // deflate by what left the network (RFC 6582 3.2 step 4)
                self.retransmit_front(now_ms);
                self.cwnd = self.cwnd.saturating_sub(acked) + mss;
            }
        } else if self.cwnd < self.ssthresh {
            // Slow start: at most a segment per ACK (RFC 5681 3.1)
            self.cwnd += acked.min(mss);
        } else {
            // Congestion avoidance: about a segment per round trip
            self.cwnd += (mss * mss / self.cwnd.max(1)).max(1);
        }

        // Our FIN consumed one sequence number. Once acknowledged, only the
        // peer's FIN is outstanding
        if self.state == TcpState::FinWait1 && seq_ge(self.snd_una, self.snd_max) {
            self.state = TcpState::FinWait2;
        }
    }

    /// The same acknowledgement again, with nothing else in it
    fn on_dup_ack(&mut self, now_ms: u64) {
        self.dup_acks += 1;
        let mss = self.mss();
        if self.in_recovery {
            // Each one is a segment that left the network, so room for another
            // (RFC 5681 3.2 step 4). With SACK the room goes to the next hole
            // first
            self.cwnd += mss;
            if self.sack_ok {
                self.retransmit_next_hole(now_ms);
            }
        } else if self.dup_acks == 3 && seq_gt(self.snd_una, self.recover.wrapping_sub(1)) {
            // Fast retransmit, and recovery until everything now outstanding is
            // acknowledged. Only past the last recovery's end (RFC 6582 4.1),
            // so one loss does not halve the window twice
            let flight = self.snd_max.wrapping_sub(self.snd_una) as usize;
            self.ssthresh = (flight / 2).max(2 * mss);
            self.recover = self.snd_max;
            self.in_recovery = true;
            for f in self.inflight.iter_mut() {
                f.rexmitted = false;
            }
            self.retransmit_front(now_ms);
            self.cwnd = self.ssthresh + 3 * mss;
        }
    }

    /// SACK blocks in an ACK: their in-flight runs are held by the peer. A
    /// block below `snd_una` or above what was sent is ignored (RFC 2018 4)
    fn take_sack(&mut self, opts: &[u8]) {
        let mut blocks = [(0u32, 0u32); 4];
        let n = tcp::parse_sack(opts, &mut blocks);
        for &(l, r) in &blocks[..n] {
            if !seq_gt(r, l) || !seq_ge(l, self.snd_una) || seq_gt(r, self.snd_max) {
                continue;
            }
            for f in self.inflight.iter_mut() {
                let end = f.seq.wrapping_add(f.len as u32);
                if seq_ge(f.seq, l) && !seq_gt(end, r) {
                    f.sacked = true;
                }
            }
            if seq_gt(r, self.high_sacked) {
                self.high_sacked = r;
            }
        }
    }

    /// Time passed: retransmissions, the SYN's retries, and data the window has
    /// made room for
    pub fn on_tick(&mut self, now_ms: u64) {
        match self.state {
            TcpState::SynSent => {
                if now_ms >= self.rexmit_at {
                    if self.syn_retries >= MAX_SYN_RETRIES {
                        self.state = TcpState::Closed;
                        self.failure = Some(Failure::SynTimeout);
                        return;
                    }
                    self.syn_retries += 1;
                    self.emit_syn();
                    // Doubling from the initial value, so three retries span 2
                    // + 4 + 8 seconds
                    self.rexmit_at =
                        now_ms + ((INIT_RTO_MS as u64) << self.syn_retries.min(6));
                }
                return;
            }
            TcpState::SynReceived => {
                if now_ms >= self.rexmit_at {
                    if self.syn_retries >= MAX_SYN_RETRIES {
                        self.state = TcpState::Closed;
                        self.failure = Some(Failure::SynTimeout);
                        return;
                    }
                    self.syn_retries += 1;
                    self.emit_syn();
                    self.rexmit_at =
                        now_ms + ((INIT_RTO_MS as u64) << self.syn_retries.min(6));
                }
                return;
            }
            TcpState::Closed => return,
            _ => {}
        }

        if let Some(front) = self.inflight.front().copied() {
            if now_ms.wrapping_sub(front.sent_ms) >= self.rto_ms as u64 {
                self.oldest_retries += 1;
                if self.oldest_retries > MAX_RETRIES {
                    self.fail(Failure::RetransmitLimit);
                    return;
                }
                self.on_timeout(now_ms);
                // RFC 6298 5.5: back off on each retransmission
                self.rto_ms = self.rto_ms.saturating_mul(2).min(MAX_RTO_MS);
            }
        }

        self.pump(now_ms);
        self.keepalive_tick(now_ms);
    }

    /// Asks a silent peer if it is still there, only while nothing of ours is
    /// outstanding; data in flight has its own timer
    fn keepalive_tick(&mut self, now_ms: u64) {
        let k = self.keepalive;
        if !k.enabled || !self.can_send() || !self.inflight.is_empty() || self.fin_queued {
            return;
        }
        let due = self
            .last_heard_ms
            .saturating_add(k.idle_ms)
            .saturating_add(k.interval_ms.saturating_mul(self.probes as u64));
        if now_ms < due {
            return;
        }
        if self.probes >= k.count {
            self.fail(Failure::KeepaliveTimeout);
            return;
        }
        self.probes += 1;
        // One byte behind what the peer has acknowledged: outside its window,
        // so not data, and a live TCP must answer it
        self.emit_at(self.snd_nxt.wrapping_sub(1), FLAG_ACK, &[], &[]);
    }

    /// The connection ended with nothing said on the wire. Reported as the peer
    /// having gone, so a reader sees the end and a poller wakes
    fn fail(&mut self, why: Failure) {
        self.state = TcpState::Closed;
        self.peer_closed = true;
        if self.failure.is_none() {
            self.failure = Some(why);
        }
    }

    /// Sends what the smaller of the peer's and congestion windows allows, and
    /// the FIN once nothing is left
    fn pump(&mut self, now_ms: u64) {
        if !self.can_send() && self.state != TcpState::FinWait1 {
            return;
        }
        let mss = self.mss();

        loop {
            let sent = self.snd_nxt.wrapping_sub(self.snd_una) as usize;
            let unsent = self.snd_buf.len().saturating_sub(sent);
            if unsent == 0 {
                break;
            }
            let len = unsent.min(mss);
            // One segment is always allowed with nothing in flight, which also
            // serves as the zero-window probe
            let flight = self.snd_max.wrapping_sub(self.snd_una) as usize;
            let room = (self.snd_wnd as usize).min(self.cwnd);
            if !self.inflight.is_empty() && (flight + len > room || sent + len > self.snd_wnd as usize) {
                break;
            }

            let seq = self.snd_nxt;
            let mut payload = Vec::with_capacity(len);
            for i in 0..len {
                payload.push(self.snd_buf[sent + i]);
            }
            self.emit_at(seq, FLAG_PSH | FLAG_ACK, &[], &payload);
            self.inflight.push_back(InFlight {
                seq, len, sent_ms: now_ms, resent: false, sacked: false, rexmitted: false,
            });
            if self.inflight.len() == 1 {
                self.rexmit_at = now_ms + self.rto_ms as u64;
            }
            self.snd_nxt = seq.wrapping_add(len as u32);
            if seq_gt(self.snd_nxt, self.snd_max) {
                self.snd_max = self.snd_nxt;
            }
        }

        // The FIN goes last, after every byte written before it
        if self.fin_queued {
            let sent = self.snd_nxt.wrapping_sub(self.snd_una) as usize;
            if sent >= self.snd_buf.len() {
                self.fin_queued = false;
                self.fin_sent = true;
                self.emit_at(self.snd_nxt, FLAG_FIN | FLAG_ACK, &[], &[]);
                self.snd_nxt = self.snd_nxt.wrapping_add(1);
                self.snd_max = self.snd_nxt;
                self.state = match self.state {
                    TcpState::CloseWait => TcpState::LastAck,
                    _ => TcpState::FinWait1,
                };
            }
        }
    }

    /// Send in-flight run `i` again, from `snd_buf`
    fn retransmit(&mut self, i: usize, now_ms: u64) {
        let Some(run) = self.inflight.get(i).copied() else { return };
        let off = run.seq.wrapping_sub(self.snd_una) as usize;
        if off + run.len > self.snd_buf.len() {
            return;
        }
        let mut payload = Vec::with_capacity(run.len);
        for k in 0..run.len {
            payload.push(self.snd_buf[off + k]);
        }
        self.emit_at(run.seq, FLAG_PSH | FLAG_ACK, &[], &payload);
        if let Some(f) = self.inflight.get_mut(i) {
            f.resent = true;
            f.rexmitted = true;
            f.sent_ms = now_ms;
        }
        if i == 0 {
            self.rexmit_at = now_ms + self.rto_ms as u64;
        }
    }

    fn retransmit_front(&mut self, now_ms: u64) {
        self.retransmit(0, now_ms);
    }

    /// In SACK recovery: the lowest run the peer lacks, below the highest it
    /// has, and not yet resent in this recovery
    fn retransmit_next_hole(&mut self, now_ms: u64) {
        let high = self.high_sacked;
        let hole = self.inflight.iter().position(|f| {
            !f.sacked && !f.rexmitted && seq_lt(f.seq.wrapping_add(f.len as u32), high.wrapping_add(1))
        });
        if let Some(i) = hole {
            self.retransmit(i, now_ms);
        }
    }

    /// Retransmission timeout: everything outstanding is taken as lost (RFC
    /// 5681 3.1, 6582 4.2). The window restarts at one segment, SACK data is
    /// forgotten (RFC 2018 8), and sending resumes from the hole. Not past a
    /// sent FIN
    fn on_timeout(&mut self, now_ms: u64) {
        let mss = self.mss();
        let flight = self.snd_max.wrapping_sub(self.snd_una) as usize;
        self.ssthresh = (flight / 2).max(2 * mss);
        self.cwnd = mss;
        self.in_recovery = false;
        self.dup_acks = 0;
        self.recover = self.snd_max;
        self.high_sacked = self.snd_una;
        if !self.fin_sent {
            if let Some(front) = self.inflight.front().copied() {
                self.inflight.truncate(1);
                self.snd_nxt = front.seq.wrapping_add(front.len as u32);
            }
        }
        for f in self.inflight.iter_mut() {
            f.sacked = false;
        }
        self.retransmit_front(now_ms);
    }

    /// Congestion control's starting point, once the peer's MSS is known
    fn start_congestion_control(&mut self) {
        self.cwnd = INIT_CWND_SEGS * self.mss();
        self.ssthresh = usize::MAX;
        self.recover = self.snd_una;
        self.high_sacked = self.snd_una;
    }

    // receive bookkeeping

    fn accept_data(&mut self, data: &[u8]) -> usize {
        let space = RX_CAP.saturating_sub(self.rx.len());
        let copy = data.len().min(space);
        if copy > 0 {
            self.rx.extend(&data[..copy]);
            self.rcv_nxt = self.rcv_nxt.wrapping_add(copy as u32);
        }
        copy
    }

    /// Keeps a run past the gap. Sorted by distance from `rcv_nxt` and merged
    /// where they touch, so the SACK blocks are the runs; anything past the
    /// window's right edge is dropped
    fn hold_out_of_order(&mut self, seq: u32, data: &[u8]) {
        let base = self.rcv_nxt;
        let off = |s: u32| s.wrapping_sub(base) as usize;
        let space = self.rcv_space();
        let start = off(seq);
        if start == 0 || start >= space || data.is_empty() {
            return;
        }
        let data = &data[..data.len().min(space - start)];
        let end = start + data.len();
        self.last_ooo_seq = seq;

        // The first run that reaches `start` or lies past it
        let i = self.ooo.iter().position(|(s, d)| off(*s) + d.len() >= start).unwrap_or(self.ooo.len());
        if i < self.ooo.len() && off(self.ooo[i].0) <= start {
            // Begins inside run `i` or at its end: the run grows by what is
            // past its end
            let run_end = off(self.ooo[i].0) + self.ooo[i].1.len();
            if end > run_end {
                self.ooo[i].1.extend_from_slice(&data[run_end - start..]);
            }
        } else {
            self.ooo.insert(i, (seq, Vec::from(data)));
        }
        // Run `i` may now reach later ones
        while i + 1 < self.ooo.len() {
            let i_end = off(self.ooo[i].0) + self.ooo[i].1.len();
            let next_start = off(self.ooo[i + 1].0);
            if next_start > i_end {
                break;
            }
            let (_, next) = self.ooo.remove(i + 1);
            let next_end = next_start + next.len();
            if next_end > i_end {
                self.ooo[i].1.extend_from_slice(&next[i_end - next_start..]);
            }
        }
        self.ooo_bytes = self.ooo.iter().map(|(_, d)| d.len()).sum();
    }

    /// Whatever the closed gap was holding back
    fn drain_out_of_order(&mut self) {
        while let Some((s, _)) = self.ooo.first() {
            if seq_gt(*s, self.rcv_nxt) {
                break;
            }
            let (seq, data) = self.ooo.remove(0);
            let skip = self.rcv_nxt.wrapping_sub(seq) as usize;
            if skip < data.len() {
                self.accept_data(&data[skip..]);
            }
        }
        self.ooo_bytes = self.ooo.iter().map(|(_, d)| d.len()).sum();
    }

    /// RFC 6298 §2, with the usual alpha = 1/8 and beta = 1/4
    fn update_rtt(&mut self, sample_ms: u32) {
        let r = sample_ms.max(1);
        if self.srtt_ms == 0 {
            self.srtt_ms = r;
            self.rttvar_ms = r / 2;
        } else {
            let diff = self.srtt_ms.abs_diff(r);
            self.rttvar_ms =
                (self.rttvar_ms - (self.rttvar_ms >> 2)).saturating_add(diff >> 2);
            self.srtt_ms = (self.srtt_ms - (self.srtt_ms >> 3)).saturating_add(r >> 3);
        }
        let k_rttvar = self.rttvar_ms.saturating_mul(4).max(1);
        self.rto_ms = self
            .srtt_ms
            .saturating_add(k_rttvar)
            .clamp(MIN_RTO_MS, MAX_RTO_MS);
    }

    // for tests and for netd's own accounting

    pub fn snd_una(&self) -> u32 {
        self.snd_una
    }
    pub fn snd_nxt(&self) -> u32 {
        self.snd_nxt
    }
    pub fn rcv_nxt(&self) -> u32 {
        self.rcv_nxt
    }
    pub fn inflight_segments(&self) -> usize {
        self.inflight.len()
    }
    /// Runs held out of order
    pub fn held_out_of_order(&self) -> usize {
        self.ooo.len()
    }

    /// Bytes held out of order
    pub fn held_out_of_order_bytes(&self) -> usize {
        self.ooo_bytes
    }
}
