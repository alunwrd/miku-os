// TCP server side over the central RX demux
//
// bind() registers the port with rx::LISTENERS so incoming SYNs land on this
// listener's backlog queue no matter who is pumping. accept() pops one SYN,
// registers the new connection's 4-tuple (so the handshake ACK and all later
// segments are routed to the new socket), answers SYN-ACK and waits for the ACK
// through the state machine's SynReceived state.
//
// `Connection::on_tick` retransmits an unanswered SYN-ACK on the same doubling
// schedule the client half uses, and gives up after the same number of tries.

use core::sync::atomic::Ordering;
use super::CTRL_C;
use super::rx;
use super::tcp::{TcpSocket, TcpState};

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
        self.socket.remote_ip()
    }
    pub fn peer_port(&self) -> u16 {
        self.socket.remote_port()
    }
    pub fn is_connected(&self) -> bool {
        self.socket.is_connected()
    }

    /// Hand the inner socket over (used by the userspace accept syscall,
    /// which stores plain TcpSockets in the fd table)
    #[allow(dead_code)]
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

    /// Is a connection waiting, without completing a handshake?
    ///
    /// What `poll` asks. Does not pump: a listener's backlog is filled by the
    /// RX demux, and the caller has already pumped
    pub fn pending(&self) -> bool {
        self.handle.is_some_and(rx::listener_pending)
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
            // Until a SYN is queued for this port (`rx::handle_tcp` wakes its
            // key), or the backstop
            let Some(h) = self.handle else { return None };
            let deadline = crate::kcore::time::now_ns().saturating_add(crate::net::irq::backstop_ns());
            crate::sched::wait::keyed(crate::sched::wait::key::net_listen(h))
                .wait_unless(deadline, || rx::listener_pending(h));
        }
    }

    fn complete_handshake(&self, syn: rx::SynRequest) -> Option<TcpConn> {
        let mut sock = TcpSocket::accept_syn(self.port, &syn)?;

        let start = crate::fs::procfs::uptime_ticks();
        loop {
            if CTRL_C.load(Ordering::SeqCst) { return None; }
            sock.recv_one();

            if sock.state() == TcpState::Established
                || sock.state() == TcpState::CloseWait
            {
                crate::log!("tcp_listener: accepted {}.{}.{}.{}:{} -> port {}",
                    syn.src_ip[0], syn.src_ip[1], syn.src_ip[2], syn.src_ip[3],
                    syn.src_port, self.port);
                return Some(TcpConn { socket: sock });
            }
            if sock.state() == TcpState::Closed { return None; }
            if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= 1250 {
                crate::log_err!("tcp_listener: handshake timeout (no ACK received)");
                return None; // socket drop unregisters the tuple
            }
            sock.wait();
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
