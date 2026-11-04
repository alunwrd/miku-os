// Transport for programs
//
// A thin layer over the socket syscalls: read what has arrived with a time
// limit, and report when the peer is done. TCP itself is in the kernel. Every
// wait is a `poll`.

extern crate alloc;

use miku::{MikuPollFd, POLLERR, POLLHUP, POLLIN, POLLNVAL};

/// Longest single `poll` before checking the deadline and signals
const SLICE_MS: i64 = 50;

fn wait_readable(fd: i64, timeout_ms: u64) -> (bool, bool) {
    let mut deadline = timeout_ms;
    loop {
        let slice = if deadline == 0 {
            0
        } else if deadline > SLICE_MS as u64 {
            SLICE_MS
        } else {
            deadline as i64
        };

        let mut pfd = MikuPollFd { fd: fd as i32, events: POLLIN, revents: 0 };
        let n = unsafe { miku::miku_poll(&mut pfd as *mut MikuPollFd, 1, slice) };
        if n < 0 {
            return (false, true);
        }
        if pfd.revents & (POLLERR | POLLNVAL) != 0 {
            return (false, true);
        }
        // Data before hangup: a FIN on the same segment as the last bytes sets
        // both
        if pfd.revents & POLLIN != 0 {
            return (true, false);
        }
        if pfd.revents & POLLHUP != 0 {
            return (false, true);
        }

        if deadline == 0 {
            return (false, false);
        }
        deadline = deadline.saturating_sub(slice as u64);
        if deadline == 0 {
            return (false, false);
        }
    }
}

pub struct Stream {
    fd: i64,
    /// The peer has closed its half. Read by the protocol code inside record
    /// loops
    pub peer_closed: bool,
}

impl Stream {
    pub fn connect(ip: [u8; 4], port: u16) -> Option<Self> {
        let fd = unsafe { miku::miku_socket() };
        if fd < 0 {
            return None;
        }
        if unsafe { miku::miku_connect(fd, ip.as_ptr(), port) } < 0 {
            unsafe { miku::miku_close(fd) };
            return None;
        }
        Some(Self { fd, peer_closed: false })
    }

    /// Writes all of `data` or returns an error
    pub fn send(&mut self, data: &[u8]) -> bool {
        let mut off = 0usize;
        while off < data.len() {
            let n = unsafe { miku::miku_send(self.fd, data[off..].as_ptr(), data.len() - off) };
            if n <= 0 {
                return false;
            }
            off += n as usize;
        }
        true
    }

    /// What has arrived, up to `buf.len()`, waiting at most `wait_ms`.
    /// `*out_len` of 0 means nothing came, which is neither an error nor the
    /// end of the stream; the caller checks its deadline and `peer_closed`
    pub fn recv_one_into(&mut self, buf: &mut [u8], out_len: &mut usize, wait_ms: u64) {
        *out_len = 0;
        if buf.is_empty() {
            return;
        }
        let (readable, closed) = wait_readable(self.fd, wait_ms);
        if !readable {
            if closed {
                self.peer_closed = true;
            }
            return;
        }
        let n = unsafe { miku::miku_recv(self.fd, buf.as_mut_ptr(), buf.len()) };
        if n > 0 {
            *out_len = n as usize;
        } else {
            // 0 is an orderly close; a negative value here means the connection
            // is gone, and for a reader both end the bytes
            self.peer_closed = true;
        }
    }

    /// Everything until the peer closes or `idle_ms` passes with nothing
    /// arriving. The limit is on silence, not the transfer. Plain-HTTP
    /// counterpart of `TlsStream::recv_to_vec`
    pub fn recv_to_vec(&mut self, out: &mut alloc::vec::Vec<u8>, idle_ms: u64) {
        const PROBE_MS: u64 = 250;
        let mut idle: u64 = 0;
        let mut buf = [0u8; 4096];
        loop {
            if crate::interrupted() {
                return;
            }
            let mut n = 0usize;
            self.recv_one_into(&mut buf, &mut n, PROBE_MS);
            if n > 0 {
                out.extend_from_slice(&buf[..n]);
                idle = 0;
                continue;
            }
            // Closed check after the read: a FIN on the same segment as the
            // last bytes must not lose the tail
            if self.peer_closed {
                return;
            }
            idle += PROBE_MS;
            if idle > idle_ms {
                return;
            }
        }
    }

    pub fn close(&mut self) {
        if self.fd >= 0 {
            unsafe { miku::miku_close(self.fd) };
            self.fd = -1;
        }
    }

    pub fn is_connected(&self) -> bool {
        self.fd >= 0 && !self.peer_closed
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.close();
    }
}

/// Datagram socket for DNS and NTP
pub struct Dgram {
    fd: i64,
}

impl Dgram {
    /// Unbound; the kernel picks an ephemeral source port on the first send
    pub fn new() -> Option<Self> {
        let fd = unsafe { miku::miku_socket_udp() };
        if fd < 0 {
            None
        } else {
            Some(Self { fd })
        }
    }

    pub fn send_to(&mut self, ip: &[u8; 4], port: u16, data: &[u8]) -> bool {
        let n = unsafe {
            miku::miku_sendto(self.fd, data.as_ptr(), data.len(), ip.as_ptr(), port)
        };
        n == data.len() as i64
    }

    /// One datagram, or None if none arrived within `wait_ms`. Returns the
    /// source so the caller can reject it
    pub fn recv_from(&mut self, buf: &mut [u8], wait_ms: u64) -> Option<(usize, [u8; 4], u16)> {
        let (readable, _) = wait_readable(self.fd, wait_ms);
        if !readable {
            return None;
        }
        let mut ip = [0u8; 4];
        let mut port: u16 = 0;
        let n = unsafe {
            miku::miku_recvfrom(
                self.fd,
                buf.as_mut_ptr(),
                buf.len(),
                ip.as_mut_ptr(),
                &mut port as *mut u16,
            )
        };
        if n <= 0 {
            None
        } else {
            Some((n as usize, ip, port))
        }
    }

    pub fn close(&mut self) {
        if self.fd >= 0 {
            unsafe { miku::miku_close(self.fd) };
            self.fd = -1;
        }
    }
}

impl Drop for Dgram {
    fn drop(&mut self) {
        self.close();
    }
}

/// Raw ICMP socket: sends one ICMP message at a time, receives whole IP
/// packets. `mikunet::icmp` builds and parses them.
///
/// ICMP has no ports, so every consumer sees every message; the caller filters
pub struct Icmp {
    fd: i64,
}

impl Icmp {
    pub fn new() -> Option<Self> {
        let fd = unsafe { miku::miku_socket_icmp() };
        if fd < 0 { None } else { Some(Self { fd }) }
    }

    /// Hop limit of outgoing packets, for traceroute
    pub fn set_ttl(&mut self, ttl: u8) -> bool {
        unsafe { miku::miku_set_ttl(self.fd, ttl) == 0 }
    }

    pub fn send_to(&mut self, ip: &[u8; 4], msg: &[u8]) -> bool {
        let n = unsafe {
            miku::miku_sendto(self.fd, msg.as_ptr(), msg.len(), ip.as_ptr(), 0)
        };
        n == msg.len() as i64
    }

    /// One IP packet, or None if none arrived inside `wait_ms`
    pub fn recv(&mut self, buf: &mut [u8], wait_ms: u64) -> Option<usize> {
        let (readable, _) = wait_readable(self.fd, wait_ms);
        if !readable {
            return None;
        }
        let mut ip = [0u8; 4];
        let mut port: u16 = 0;
        let n = unsafe {
            miku::miku_recvfrom(
                self.fd,
                buf.as_mut_ptr(),
                buf.len(),
                ip.as_mut_ptr(),
                &mut port as *mut u16,
            )
        };
        if n <= 0 { None } else { Some(n as usize) }
    }

    pub fn close(&mut self) {
        if self.fd >= 0 {
            unsafe { miku::miku_close(self.fd) };
            self.fd = -1;
        }
    }
}

impl Drop for Icmp {
    fn drop(&mut self) {
        self.close();
    }
}
