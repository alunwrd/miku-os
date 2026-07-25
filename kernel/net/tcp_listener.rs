// TCP server side over the central RX demux
//
// bind() registers the port with rx::LISTENERS so incoming SYNs land on this
// listener's backlog queue no matter who is pumping. accept() pops one SYN,
// registers the new connection's 4-tuple (so the handshake ACK and all later
// segments are routed to the new socket), answers SYN-ACK and waits for the
// ACK through the ordinary TcpSocket state machine (SynReceived state).

use core::sync::atomic::Ordering;
use super::CTRL_C;
use super::rx;
use super::tcp::{mss_opts, TcpSocket, TcpState, FLAG_ACK, FLAG_SYN};

pub struct TcpConn {
    socket: TcpSocket,
}

impl TcpConn {
    pub fn send(&mut self, data: &[u8]) -> bool {
        self.socket.send(data)
    }

    pub fn recv_wait(&mut self, timeout_iters: usize) -> &[u8] {
        self.socket.recv_wait(timeout_iters)
    }

    pub fn recv_all(&mut self, timeout_iters: usize) -> &[u8] {
        self.socket.recv_all(timeout_iters)
    }

    pub fn close(&mut self) {
        self.socket.close();
    }

    pub fn peer_ip(&self) -> [u8; 4] {
        self.socket.remote_ip
    }
    pub fn peer_port(&self) -> u16 {
        self.socket.remote_port
    }
    pub fn is_connected(&self) -> bool {
        self.socket.is_connected()
    }

    /// Hand the inner socket over (used by the userspace accept syscall,
    /// which stores plain TcpSockets in the fd table)
    pub fn into_socket(self) -> TcpSocket {
        self.socket
    }
}

pub struct TcpListener {
    pub port: u16,
    handle: Option<usize>,
}

impl TcpListener {
    pub fn bind(port: u16) -> Option<Self> {
        let handle = rx::register_listener(port)?;
        Some(Self { port, handle: Some(handle) })
    }

    /// Non-blocking: check the backlog once, complete the handshake if a SYN
    /// is pending. Callers loop over this with their own timeout policy
    pub fn accept(&self) -> Option<TcpConn> {
        rx::pump();
        let syn = rx::listener_take(self.handle?)?;
        self.complete_handshake(syn)
    }

    /// Block until a connection arrives or the timeout (in ticks) expires
    pub fn accept_wait(&self, timeout_ticks: u64) -> Option<TcpConn> {
        let start = crate::fs::procfs::uptime_ticks();
        loop {
            if CTRL_C.load(Ordering::SeqCst) { return None; }
            if let Some(c) = self.accept() {
                return Some(c);
            }
            if timeout_ticks > 0
                && crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= timeout_ticks
            {
                return None;
            }
            crate::sched::yield_now();
        }
    }

    fn complete_handshake(&self, syn: rx::SynRequest) -> Option<TcpConn> {
        let local_ip  = super::get_ip();
        let local_mac = super::get_mac();

        let mut sock = TcpSocket::new();
        sock.local_ip    = local_ip;
        sock.local_mac   = local_mac;
        sock.local_port  = self.port;
        sock.remote_ip   = syn.src_ip;
        sock.remote_mac  = syn.src_mac;
        sock.remote_port = syn.src_port;
        sock.peer_mss    = syn.mss;
        sock.ack         = syn.seq.wrapping_add(1);

        // Register the 4-tuple before the SYN-ACK leaves so the client's ACK
        // is queued to this socket instead of being dropped
        sock.rx_handle = rx::register_tcp(self.port, syn.src_ip, syn.src_port);
        sock.rx_handle?;

        sock.send_segment_at(sock.seq, FLAG_SYN | FLAG_ACK, &mss_opts(), &[]);
        sock.seq = sock.seq.wrapping_add(1);
        sock.state = TcpState::SynReceived;

        let start = crate::fs::procfs::uptime_ticks();
        loop {
            if CTRL_C.load(Ordering::SeqCst) { return None; }
            sock.recv_one();
            if sock.state == TcpState::Established
                || sock.state == TcpState::CloseWait
            {
                crate::log!("tcp_listener: accepted {}.{}.{}.{}:{} -> port {}",
                    syn.src_ip[0], syn.src_ip[1], syn.src_ip[2], syn.src_ip[3],
                    syn.src_port, self.port);
                return Some(TcpConn { socket: sock });
            }
            if sock.state == TcpState::Closed { return None; }
            if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= 1250 {
                crate::log_err!("tcp_listener: handshake timeout (no ACK received)");
                return None; // socket drop unregisters the tuple
            }
            crate::sched::yield_now();
        }
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            rx::unregister_listener(h);
        }
    }
}
