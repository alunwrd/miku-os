// TCP, as the kernel sees it: a blocking socket over a state machine that is
// not the kernel's
//
// The protocol lives in `mikunet::stack::conn` and is checked by
// `cd lib/net && cargo test`: sequence wraparound, retransmission, three
// duplicate acknowledgements, a FIN on the last data segment.
//
// Here is what belongs to a kernel:
//
//   * the blocking API. `read` and `send` are syscalls, so somebody has to
//     wait, and the scheduler puts a task to sleep.
//   * ethernet and IP framing. `Connection` emits a TCP segment; which card it
//     leaves by, and which MAC stands for the next hop, is routing: kernel
//     state, arbitrated between every process on the machine.
//   * the ephemeral port, drawn against the demux so two processes cannot be
//     handed the same one.
//
// The initial sequence number comes from `kcore::random` (RDRAND where the
// processor has it). The crate cannot draw its own: built without the
// `userspace` feature its entropy is a deterministic generator for repeatable
// tests, and a predictable ISN lets anybody write into a connection.

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use super::eth::{EthFrame, ETHERTYPE_IP};
use super::{ipv4, rx, CTRL_C};

use mikunet::stack::conn::Connection;

/// The wire format, re-exported so `rx`, the selftest and other readers of a
/// segment can say `tcp::`
pub use mikunet::stack::tcp::*;

/// How long `recv_wait` waits for the first byte before giving up (ticks)
const RECV_TIMEOUT_TICKS: u64 = 1000; // 4 s
/// How long `connect` waits for the handshake
const CONNECT_TIMEOUT_TICKS: u64 = 2500; // 10 s
/// How long `close` waits for the peer to finish its half
const CLOSE_TIMEOUT_TICKS: u64 = 500; // 2 s
/// And how long after our own FIN is acknowledged: a server keeping a half-open
/// connection is no reason to hold the caller for the full timeout
const HALF_CLOSED_TICKS: u64 = 125; // 500 ms

pub struct TcpSocket {
    conn: Connection,
    pub local_mac: [u8; 6],
    pub remote_mac: [u8; 6],
    /// Handle of this connection's queue in the RX demux
    pub rx_handle: Option<usize>,
    /// Bytes taken out of the connection and not yet handed to the caller.
    /// `recv_wait` and `recv_all` return a slice, so the data has to be
    /// somewhere contiguous that outlives the call
    held: Vec<u8>,
    /// When the SYN went out, for a handshake advanced a step at a time
    connect_started: u64,
}

/// Where a client handshake stands after one step
#[derive(Clone, Copy, PartialEq)]
pub enum Handshake {
    Done,
    Pending,
    /// Refused, reset, or out of time
    Failed,
}

impl TcpSocket {
    fn wrap(conn: Connection, local_mac: [u8; 6], remote_mac: [u8; 6]) -> Self {
        Self { conn, local_mac, remote_mac, rx_handle: None, held: Vec::new(), connect_started: 0 }
    }

    pub fn local_port(&self) -> u16 {
        self.conn.local_port
    }
    pub fn remote_ip(&self) -> [u8; 4] {
        self.conn.remote_ip
    }
    pub fn remote_port(&self) -> u16 {
        self.conn.remote_port
    }
    pub fn state(&self) -> TcpState {
        self.conn.state
    }
    pub fn peer_closed(&self) -> bool {
        self.conn.peer_closed
    }
    pub fn is_connected(&self) -> bool {
        self.conn.is_connected()
    }
    pub fn keepalive(&self) -> mikunet::stack::conn::Keepalive {
        self.conn.keepalive()
    }
    pub fn set_keepalive(&mut self, k: mikunet::stack::conn::Keepalive) {
        self.conn.set_keepalive(k, super::now_ms());
    }
    /// Bytes ready to be read without waiting
    pub fn rx_available(&self) -> usize {
        self.held.len() + self.conn.rx_available()
    }

    /// Wrap one TCP segment in IP and ethernet and hand it to the driver
    fn ship(&self, segment: &[u8]) -> bool {
        let mut ip_buf = [0u8; 1500];
        let ip_len = ipv4::build(
            &self.conn.local_ip,
            &self.conn.remote_ip,
            ipv4::PROTO_TCP,
            segment,
            &mut ip_buf,
        );
        if ip_len == 0 {
            return false;
        }
        let mut eth_buf = [0u8; 1520];
        let eth_len = EthFrame::build(
            &self.remote_mac,
            &self.local_mac,
            ETHERTYPE_IP,
            &ip_buf[..ip_len],
            &mut eth_buf,
        );
        if eth_len == 0 {
            return false;
        }
        super::tx_frame(&eth_buf[..eth_len])
    }

    /// Everything the state machine decided to say, onto the wire.
    ///
    /// A segment the driver will not take is dropped rather than requeued, as
    /// if lost on the wire, and the retransmission timer handles it
    fn flush(&mut self) {
        while let Some(seg) = self.conn.take_segment() {
            if !self.ship(&seg) {
                break;
            }
        }
    }

    /// Drain this connection's demux queue into the state machine, let the
    /// clock run, and send whatever came of it. Returns true if a segment
    /// arrived
    pub fn recv_one(&mut self) -> bool {
        rx::pump();
        let now = super::now_ms();
        let mut got = false;

        if let Some(handle) = self.rx_handle {
            while let Some(pkt) = rx::tcp_take(handle) {
                got = true;
                self.conn.on_segment(&pkt.seg, &pkt.payload, &pkt.opts, now);
            }
        }
        self.conn.on_tick(now);
        self.flush();

        // Move what arrived out of the connection, so `rx_available` and the
        // slice-returning reads see one buffer rather than two
        let ready = self.conn.rx_available();
        if ready > 0 {
            let at = self.held.len();
            self.held.resize(at + ready, 0);
            let n = self.conn.read(&mut self.held[at..]);
            self.held.truncate(at + n);
        }
        got
    }

    /// A blocking client connect: the SYN, then the handshake a tick at a time
    pub fn connect(remote_ip: [u8; 4], remote_port: u16) -> Option<Self> {
        let mut sock = Self::begin_connect(remote_ip, remote_port)?;
        loop {
            if CTRL_C.load(Ordering::SeqCst) {
                return None;
            }
            match sock.handshake_step() {
                Handshake::Done => return Some(sock),
                Handshake::Failed => return None,
                Handshake::Pending => {}
            }
            sock.wait();
        }
    }

    /// Sends the SYN and returns at once, for a connect that must not block.
    ///
    /// The one wait left is ARP: the first packet to an unknown host has to
    /// learn its MAC before a SYN can be addressed. A LAN round trip, over in a
    /// millisecond, so not worth a second pending state
    pub fn begin_connect(remote_ip: [u8; 4], remote_port: u16) -> Option<Self> {
        let local_ip = super::get_ip();
        let local_mac = super::get_mac();
        let remote_mac = super::resolve_arp(&remote_ip, &local_ip, &local_mac)?;

        // Ephemeral port, walked forward past any tuple already in use. The
        // demux is the only thing that knows what every process holds
        let mut local_port = super::ephemeral_port();
        while rx::tcp_tuple_in_use(local_port, remote_ip, remote_port) {
            local_port = super::ephemeral_port();
        }

        let isn = crate::kcore::random::random_u64() as u32;
        let conn = Connection::connect(
            local_ip, local_port, remote_ip, remote_port, isn, super::now_ms(),
        );
        let mut sock = Self::wrap(conn, local_mac, remote_mac);

        // Registers with the demux before the SYN goes out, or the SYN-ACK can
        // race it and be dropped
        sock.rx_handle = rx::register_tcp(local_port, remote_ip, remote_port);
        sock.rx_handle?;
        sock.connect_started = crate::fs::procfs::uptime_ticks();
        sock.flush();
        Some(sock)
    }

    /// Advances a client handshake by whatever has arrived, without waiting.
    /// The retransmission timer runs inside `recv_one`, so calling this often
    /// enough recovers a lost SYN
    pub fn handshake_step(&mut self) -> Handshake {
        self.recv_one();
        match self.conn.state {
            TcpState::Established => Handshake::Done,
            TcpState::Closed => Handshake::Failed,
            _ if crate::fs::procfs::uptime_ticks().wrapping_sub(self.connect_started)
                >= CONNECT_TIMEOUT_TICKS => Handshake::Failed,
            _ => Handshake::Pending,
        }
    }

    /// The server half: a SYN reached a listener's backlog and we answer it
    pub fn accept_syn(local_port: u16, syn: &rx::SynRequest) -> Option<Self> {
        let local_ip = super::get_ip();
        let local_mac = super::get_mac();

        let isn = crate::kcore::random::random_u64() as u32;
        let conn = Connection::accept(
            local_ip,
            local_port,
            syn.src_ip,
            syn.src_port,
            isn,
            syn.seq,
            syn.window,
            syn.opts,
            super::now_ms(),
        );
        let mut sock = Self::wrap(conn, local_mac, syn.src_mac);

        // Register the 4-tuple before the SYN-ACK leaves, so the client's ACK
        // is queued to this socket instead of being dropped
        sock.rx_handle = rx::register_tcp(local_port, syn.src_ip, syn.src_port);
        sock.rx_handle?;
        sock.flush();
        Some(sock)
    }

    pub fn send(&mut self, data: &[u8]) -> bool {
        if !self.conn.is_connected() {
            return false;
        }
        let mut off = 0usize;
        let start = crate::fs::procfs::uptime_ticks();
        loop {
            if CTRL_C.load(Ordering::SeqCst) {
                return false;
            }
            off += self.conn.write(&data[off..]);
            self.recv_one();

            if off >= data.len() && self.conn.tx_pending() == 0 {
                return true;
            }
            if self.conn.state == TcpState::Closed {
                return false;
            }
            // The state machine gives up on its own after MAX_RETRIES; this is
            // only a backstop for a transmit ring that never drains
            if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= 7500 {
                crate::log_err!("tcp: send stalled, giving up");
                return false;
            }
            self.wait();
        }
    }

    /// Off the processor until a segment arrives for this connection, or the
    /// backstop: a tick while the card is polled, longer once it interrupts
    /// (`net::irq::backstop_ns`). The demux wakes this connection's key as it
    /// queues a segment; what a wake finds is `recv_one`'s to take
    pub fn wait(&self) {
        use crate::sched::wait::{key, keyed};
        let deadline = crate::kcore::time::now_ns().saturating_add(super::irq::backstop_ns());
        match self.rx_handle {
            Some(h) => {
                keyed(key::net_tcp(h)).wait_unless(deadline, || rx::tcp_pending(h));
            }
            None => crate::sched::sleep(1),
        }
    }

    /// Copy what is ready into `buf` at `*buf_len`, without waiting
    pub fn recv_one_into(&mut self, buf: &mut [u8], buf_len: &mut usize) {
        self.recv_one();
        let space = buf.len() - *buf_len;
        let take = self.held.len().min(space);
        if take > 0 {
            buf[*buf_len..*buf_len + take].copy_from_slice(&self.held[..take]);
            *buf_len += take;
            self.held.drain(..take);
        }
    }

    /// Wait for the first bytes to arrive, then return everything held
    pub fn recv_wait(&mut self, _timeout_iters: usize) -> &[u8] {
        let start = crate::fs::procfs::uptime_ticks();
        loop {
            if CTRL_C.load(Ordering::SeqCst) {
                break;
            }
            self.recv_one();
            if !self.held.is_empty() || self.conn.peer_closed {
                break;
            }
            if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= RECV_TIMEOUT_TICKS {
                break;
            }
            self.wait();
        }
        &self.held
    }

    /// Read until the peer closes or goes quiet for `RECV_TIMEOUT_TICKS`
    pub fn recv_all(&mut self, _timeout_iters: usize) -> &[u8] {
        let mut last_progress = crate::fs::procfs::uptime_ticks();
        loop {
            if CTRL_C.load(Ordering::SeqCst) {
                break;
            }
            let before = self.held.len();
            self.recv_one();
            if self.conn.peer_closed {
                break;
            }
            if self.held.len() != before {
                last_progress = crate::fs::procfs::uptime_ticks();
            } else if crate::fs::procfs::uptime_ticks().wrapping_sub(last_progress)
                >= RECV_TIMEOUT_TICKS
            {
                break;
            }
            self.wait();
        }
        &self.held
    }

    /// Stream everything into `out` until the peer closes or falls silent
    pub fn recv_to_vec(&mut self, out: &mut Vec<u8>, idle_ticks: u64) {
        let mut last_progress = crate::fs::procfs::uptime_ticks();
        loop {
            if CTRL_C.load(Ordering::SeqCst) {
                break;
            }
            self.recv_one();
            if !self.held.is_empty() {
                out.extend_from_slice(&self.held);
                self.held.clear();
                last_progress = crate::fs::procfs::uptime_ticks();
            }
            if self.conn.peer_closed {
                crate::trace_println!("[tcp] recv done, peer closed after {} bytes", out.len());
                break;
            }
            if self.conn.state == TcpState::Closed {
                crate::log_err!("tcp: connection died after {} bytes", out.len());
                break;
            }
            if crate::fs::procfs::uptime_ticks().wrapping_sub(last_progress) >= idle_ticks {
                crate::log_err!("tcp: recv idle timeout after {} bytes", out.len());
                break;
            }
            self.wait();
        }
    }

    pub fn close(&mut self) {
        let start = crate::fs::procfs::uptime_ticks();
        self.conn.close(super::now_ms());
        self.flush();

        let mut half_closed_at: Option<u64> = None;
        while self.conn.state != TcpState::Closed {
            self.recv_one();
            // The peer's FIN that `recv_one` just took closed it: nothing is
            // left to wake a wait, which would run its whole backstop
            if self.conn.state == TcpState::Closed {
                break;
            }
            let now = crate::fs::procfs::uptime_ticks();
            if self.conn.state == TcpState::FinWait2 {
                let since = *half_closed_at.get_or_insert(now);
                if now.wrapping_sub(since) >= HALF_CLOSED_TICKS {
                    break;
                }
            }
            if now.wrapping_sub(start) >= CLOSE_TIMEOUT_TICKS {
                break;
            }
            self.wait();
        }

        if let Some(h) = self.rx_handle.take() {
            rx::unregister_tcp(h);
        }
    }
}

impl Drop for TcpSocket {
    fn drop(&mut self) {
        if let Some(h) = self.rx_handle.take() {
            rx::unregister_tcp(h);
        }
    }
}
