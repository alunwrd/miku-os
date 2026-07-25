// BSD-style socket layer for userspace
//
// This is the kernel side of the socket syscalls (see src/syscall/net.rs).
// It owns a fixed table of kernel sockets; each is exposed to userspace as
// a file descriptor in a dedicated high range (SOCK_FD_BASE..) that the VFS
// never hands out, so sys_read/sys_write/sys_close can tell a socket fd from
// an ordinary file fd by range alone and route accordingly.
//
// Design notes:
//   - Blocking semantics only. connect/send/recv/accept block the calling
//     thread, exactly like a default BSD socket without O_NONBLOCK.
//   - The global table mutex is NEVER held across a blocking network call:
//     ops take the inner TcpSocket/TcpListener out of the slot (marking it
//     Busy), drop the lock, run the blocking op, then put it back. This keeps
//     the table responsive and avoids lock-order inversions with net::NET.
//   - Sockets are per-process. On process exit, close_all_for_pid frees every
//     socket the pid still owns (wired from free_process_resources).
//
// Supported: AF_INET SOCK_STREAM clients (socket/connect/send/recv), servers
// (bind/listen/accept), and SOCK_DGRAM (bind/sendto/recvfrom, plus connect
// to pin a default peer). RX for every socket flows through the central
// demux (net::rx), so any number of sockets can be live at once.

extern crate alloc;
use alloc::boxed::Box;
use spin::Mutex;

use super::rx;
use super::tcp::TcpSocket;
use super::tcp_listener::TcpListener;

// Userspace address-family / socket-type constants (match Linux ABI)
pub const AF_INET:     u16 = 2;
pub const SOCK_STREAM: u32 = 1;
pub const SOCK_DGRAM:  u32 = 2;
pub const IPPROTO_TCP: u32 = 6;
pub const IPPROTO_UDP: u32 = 17;

/// Socket fds live above the VFS fd range (MAX_OPEN_FILES = 128) so the two
/// namespaces never collide and routing is a simple range check
pub const SOCK_FD_BASE: usize = 4096;
pub const MAX_SOCKETS:  usize = 64;

/// A blocking recv/connect/accept gives up after this many timer ticks so a
/// dead peer can never wedge a process forever (tick = 4 ms => 30 s)
const OP_TIMEOUT_TICKS: u64 = 7500;

/// Socket-layer errors. The syscall layer (syscall::net) maps these to errno
/// values; keeping a dedicated enum here avoids net depending on syscall
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SockError {
    BadFd,
    NotConnected,
    AlreadyConnected,
    Busy,
    ConnRefused,
    BrokenPipe,
    ConnReset,
    AddrInUse,
    WouldBlock,
    Invalid,
    NotSupported,
    TableFull,
}

#[derive(Clone, Copy, PartialEq)]
enum SockState {
    /// Allocated by socket(), no connection yet (TCP) / idle (listener)
    Unconnected,
    /// connect()/accept() succeeded, inner object is Some when not mid-op
    Connected,
    /// A blocking op took the inner object out; ops must not race
    Busy,
}

#[derive(Clone, Copy, PartialEq)]
enum SockKind {
    Tcp,
    Listener,
    Udp,
}

struct Slot {
    active: bool,
    owner_pid: u64,
    state: SockState,
    kind: SockKind,
    tcp: Option<Box<TcpSocket>>,
    listener: Option<Box<TcpListener>>,
    /// bind() port: pending listen port for TCP, bound port for UDP
    bound_port: u16,
    /// UDP queue handle in the RX demux
    udp_rx: Option<usize>,
    /// UDP default peer set by connect()
    udp_peer: Option<([u8; 4], u16)>,
}

impl Slot {
    const fn empty() -> Self {
        Slot {
            active: false,
            owner_pid: 0,
            state: SockState::Unconnected,
            kind: SockKind::Tcp,
            tcp: None,
            listener: None,
            bound_port: 0,
            udp_rx: None,
            udp_peer: None,
        }
    }

    /// Release demux resources held by this slot (TcpSocket/TcpListener
    /// unregister themselves on drop; UDP handles are ours to free)
    fn release(&mut self) {
        if let Some(h) = self.udp_rx.take() {
            rx::unregister_udp(h);
        }
        self.tcp = None;      // Drop unregisters the 4-tuple
        self.listener = None; // Drop unregisters the listen port
        *self = Slot::empty();
    }
}

struct SocketTable {
    slots: [Slot; MAX_SOCKETS],
}

impl SocketTable {
    const fn new() -> Self {
        // Slot is not Copy (holds Boxes), so build the array element-wise
        const EMPTY: Slot = Slot::empty();
        SocketTable { slots: [EMPTY; MAX_SOCKETS] }
    }
}

static SOCKETS: Mutex<SocketTable> = Mutex::new(SocketTable::new());

/// True if 'fd' falls in the socket descriptor range
#[inline]
pub fn is_socket_fd(fd: u64) -> bool {
    let fd = fd as usize;
    fd >= SOCK_FD_BASE && fd < SOCK_FD_BASE + MAX_SOCKETS
}

#[inline]
fn fd_to_idx(fd: u64) -> Option<usize> {
    if is_socket_fd(fd) { Some(fd as usize - SOCK_FD_BASE) } else { None }
}

#[inline]
fn idx_to_fd(idx: usize) -> u64 {
    (SOCK_FD_BASE + idx) as u64
}

// ----------------------------------------------------------------------
// Slot allocation / lifecycle
// ----------------------------------------------------------------------

fn create(pid: u64, kind: SockKind) -> Option<u64> {
    let mut tbl = SOCKETS.lock();
    for i in 0..MAX_SOCKETS {
        if !tbl.slots[i].active {
            tbl.slots[i] = Slot {
                active: true,
                owner_pid: pid,
                state: SockState::Unconnected,
                kind,
                tcp: None,
                listener: None,
                bound_port: 0,
                udp_rx: None,
                udp_peer: None,
            };
            return Some(idx_to_fd(i));
        }
    }
    None
}

/// Allocate a TCP socket owned by 'pid'. Returns the socket fd or None if the
/// table is full
pub fn create_tcp(pid: u64) -> Option<u64> {
    create(pid, SockKind::Tcp)
}

/// Allocate a UDP socket owned by 'pid'
pub fn create_udp(pid: u64) -> Option<u64> {
    create(pid, SockKind::Udp)
}

/// Close and free the socket behind 'fd'. Returns true if it was a live socket
pub fn close_fd(fd: u64) -> bool {
    let idx = match fd_to_idx(fd) { Some(i) => i, None => return false };

    // Take the inner socket out under the lock, then run the (blocking) FIN
    // handshake without holding the table lock
    let taken = {
        let mut tbl = SOCKETS.lock();
        if !tbl.slots[idx].active { return false; }
        let inner = tbl.slots[idx].tcp.take();
        tbl.slots[idx].release();
        inner
    };
    if let Some(mut t) = taken {
        t.close();
    }
    true
}

/// Free every socket still owned by 'pid' (process-exit cleanup). Does not run
/// the graceful FIN handshake - the process is already gone, so we just drop
/// the connections (the peer will see an RST or time out, like a hard exit)
pub fn close_all_for_pid(pid: u64) {
    let mut tbl = SOCKETS.lock();
    for i in 0..MAX_SOCKETS {
        if tbl.slots[i].active && tbl.slots[i].owner_pid == pid {
            tbl.slots[i].release();
        }
    }
}

/// True if 'fd' is a socket owned by 'pid'
pub fn owned_by(fd: u64, pid: u64) -> bool {
    match fd_to_idx(fd) {
        Some(idx) => {
            let tbl = SOCKETS.lock();
            tbl.slots[idx].active && tbl.slots[idx].owner_pid == pid
        }
        None => false,
    }
}

// ----------------------------------------------------------------------
// bind / listen / accept
// ----------------------------------------------------------------------

/// bind() the socket to a local port. For TCP the port takes effect at
/// listen(); for UDP the demux queue is registered immediately
pub fn bind(fd: u64, port: u16) -> Result<(), SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    if port == 0 {
        return Err(SockError::Invalid);
    }
    let mut tbl = SOCKETS.lock();
    let slot = &mut tbl.slots[idx];
    if !slot.active { return Err(SockError::BadFd); }

    match slot.kind {
        SockKind::Tcp => {
            if slot.state != SockState::Unconnected {
                return Err(SockError::AlreadyConnected);
            }
            slot.bound_port = port;
            Ok(())
        }
        SockKind::Udp => {
            if slot.udp_rx.is_some() {
                return Err(SockError::Invalid); // already bound
            }
            match rx::register_udp(port) {
                Some(h) => {
                    slot.udp_rx = Some(h);
                    slot.bound_port = port;
                    Ok(())
                }
                None => Err(SockError::AddrInUse),
            }
        }
        SockKind::Listener => Err(SockError::Invalid),
    }
}

/// listen() turns a bound TCP socket into a listener. The demux starts
/// queueing SYNs for the port immediately
pub fn listen(fd: u64) -> Result<(), SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    let mut tbl = SOCKETS.lock();
    let slot = &mut tbl.slots[idx];
    if !slot.active { return Err(SockError::BadFd); }
    if slot.kind != SockKind::Tcp || slot.state != SockState::Unconnected {
        return Err(SockError::Invalid);
    }
    if slot.bound_port == 0 {
        return Err(SockError::Invalid); // listen() before bind()
    }
    match TcpListener::bind(slot.bound_port) {
        Some(l) => {
            slot.listener = Some(Box::new(l));
            slot.kind = SockKind::Listener;
            Ok(())
        }
        None => Err(SockError::AddrInUse),
    }
}

/// accept() one connection. Blocking (up to OP_TIMEOUT_TICKS). On success
/// returns the new connection's fd plus the peer address
pub fn accept(fd: u64, pid: u64) -> Result<(u64, [u8; 4], u16), SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;

    // Take the listener out (Busy) so a second accept cannot race
    let listener = {
        let mut tbl = SOCKETS.lock();
        let slot = &mut tbl.slots[idx];
        if !slot.active { return Err(SockError::BadFd); }
        if slot.kind != SockKind::Listener {
            return Err(SockError::Invalid);
        }
        match slot.state {
            SockState::Busy => return Err(SockError::Busy),
            _ => {}
        }
        match slot.listener.take() {
            Some(l) => {
                slot.state = SockState::Busy;
                l
            }
            None => return Err(SockError::Busy),
        }
    };

    let conn = listener.accept_wait(OP_TIMEOUT_TICKS);

    // Put the listener back (unless the fd was closed while we blocked)
    {
        let mut tbl = SOCKETS.lock();
        let slot = &mut tbl.slots[idx];
        if slot.active && slot.state == SockState::Busy {
            slot.listener = Some(listener);
            slot.state = SockState::Unconnected;
        }
        // else: dropped here, Drop unregisters the listen port
    }

    let conn = conn.ok_or(SockError::WouldBlock)?;
    let peer_ip = conn.peer_ip();
    let peer_port = conn.peer_port();

    // Park the accepted connection in a fresh slot
    let new_fd = create(pid, SockKind::Tcp).ok_or(SockError::TableFull)?;
    let new_idx = fd_to_idx(new_fd).ok_or(SockError::BadFd)?;
    {
        let mut tbl = SOCKETS.lock();
        let slot = &mut tbl.slots[new_idx];
        slot.tcp = Some(Box::new(conn.into_socket()));
        slot.state = SockState::Connected;
    }
    Ok((new_fd, peer_ip, peer_port))
}

// ----------------------------------------------------------------------
// connect
// ----------------------------------------------------------------------

/// Connect the socket behind 'fd' to 'ip:port'. For TCP this runs the
/// blocking three-way handshake; for UDP it just pins the default peer
pub fn connect(fd: u64, ip: [u8; 4], port: u16) -> Result<(), SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;

    // UDP "connect" only records the peer
    {
        let mut tbl = SOCKETS.lock();
        let slot = &mut tbl.slots[idx];
        if !slot.active { return Err(SockError::BadFd); }
        match slot.kind {
            SockKind::Udp => {
                slot.udp_peer = Some((ip, port));
                return Ok(());
            }
            SockKind::Listener => return Err(SockError::Invalid),
            SockKind::Tcp => {}
        }
        // Transition Unconnected -> Busy under the lock so a second
        // connect/op on the same fd cannot race the handshake
        match slot.state {
            SockState::Unconnected => slot.state = SockState::Busy,
            SockState::Connected   => return Err(SockError::AlreadyConnected),
            SockState::Busy        => return Err(SockError::Busy),
        }
    }

    // Blocking three-way handshake with no table lock held
    let result = TcpSocket::connect(ip, port);

    let mut tbl = SOCKETS.lock();
    let slot = &mut tbl.slots[idx];
    // The socket may have been closed (process teardown) while we blocked
    if !slot.active {
        return Err(SockError::BadFd);
    }
    match result {
        Some(sock) => {
            slot.tcp = Some(Box::new(sock));
            slot.state = SockState::Connected;
            Ok(())
        }
        None => {
            // Handshake failed: stay Unconnected so the caller may retry
            slot.state = SockState::Unconnected;
            Err(SockError::ConnRefused)
        }
    }
}

// ----------------------------------------------------------------------
// TCP send/recv (lock released during the network call)
// ----------------------------------------------------------------------

/// Take the inner TcpSocket out of a Connected slot for a blocking op.
/// Returns the boxed socket and leaves the slot marked Busy
fn take_connected(idx: usize) -> Result<Box<TcpSocket>, SockError> {
    let mut tbl = SOCKETS.lock();
    let slot = &mut tbl.slots[idx];
    if !slot.active { return Err(SockError::BadFd); }
    if slot.kind != SockKind::Tcp {
        return Err(SockError::Invalid);
    }
    match slot.state {
        SockState::Connected => {}
        SockState::Unconnected => return Err(SockError::NotConnected),
        SockState::Busy => return Err(SockError::Busy),
    }
    match slot.tcp.take() {
        Some(t) => {
            slot.state = SockState::Busy;
            Ok(t)
        }
        None => Err(SockError::NotConnected),
    }
}

/// Put a borrowed-out TcpSocket back into its slot after a blocking op.
/// If the slot was closed meanwhile, the socket is dropped here
fn return_socket(idx: usize, sock: Box<TcpSocket>) {
    let mut tbl = SOCKETS.lock();
    let slot = &mut tbl.slots[idx];
    if slot.active && slot.state == SockState::Busy {
        slot.tcp = Some(sock);
        slot.state = SockState::Connected;
    }
    // else: closed under us - dropping 'sock' runs its destructor
}

/// Send all of 'data' on the socket. Blocking. Returns bytes sent or errno.
/// For a connected UDP socket this is sendto(peer)
pub fn send(fd: u64, data: &[u8]) -> Result<usize, SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;

    // UDP path
    {
        let tbl = SOCKETS.lock();
        let slot = &tbl.slots[idx];
        if slot.active && slot.kind == SockKind::Udp {
            let (ip, port) = slot.udp_peer.ok_or(SockError::NotConnected)?;
            drop(tbl);
            return sendto(fd, ip, port, data);
        }
    }

    let mut sock = take_connected(idx)?;

    let ok = sock.send(data);
    let peer_gone = sock.peer_closed;
    return_socket(idx, sock);

    if ok {
        Ok(data.len())
    } else if peer_gone {
        Err(SockError::BrokenPipe)
    } else {
        Err(SockError::ConnReset)
    }
}

/// Receive up to 'buf.len()' bytes. Blocking: waits until at least one byte
/// arrives or the peer closes (then returns 0 = EOF). Returns bytes read or
/// errno. For UDP this is recvfrom with the source address discarded
pub fn recv(fd: u64, buf: &mut [u8]) -> Result<usize, SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;

    // UDP path
    {
        let tbl = SOCKETS.lock();
        let slot = &tbl.slots[idx];
        if slot.active && slot.kind == SockKind::Udp {
            drop(tbl);
            return recvfrom(fd, buf).map(|(n, _, _)| n);
        }
    }

    let mut sock = take_connected(idx)?;

    let start = crate::fs::procfs::uptime_ticks();
    let mut got = 0usize;
    loop {
        if crate::net::CTRL_C.load(core::sync::atomic::Ordering::SeqCst) {
            break;
        }
        sock.recv_one_into(buf, &mut got);
        if got > 0 {
            break;
        }
        if sock.peer_closed {
            break; // graceful EOF: return 0
        }
        if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= OP_TIMEOUT_TICKS {
            break;
        }
        crate::sched::yield_now();
    }

    return_socket(idx, sock);
    Ok(got)
}

// ----------------------------------------------------------------------
// UDP sendto / recvfrom
// ----------------------------------------------------------------------

/// Ensure the UDP socket has a local port, auto-binding an ephemeral one on
/// first send like BSD sockets do. Returns the bound port
fn udp_autobind(idx: usize) -> Result<u16, SockError> {
    let mut tbl = SOCKETS.lock();
    let slot = &mut tbl.slots[idx];
    if !slot.active { return Err(SockError::BadFd); }
    if slot.kind != SockKind::Udp { return Err(SockError::Invalid); }
    if let Some(_) = slot.udp_rx {
        return Ok(slot.bound_port);
    }
    let mut port = 49152 + (crate::fs::procfs::uptime_ticks() as u16 & 0x3FFF);
    for _ in 0..MAX_SOCKETS {
        if let Some(h) = rx::register_udp(port) {
            slot.udp_rx = Some(h);
            slot.bound_port = port;
            return Ok(port);
        }
        port = port.wrapping_add(1).max(49152);
    }
    Err(SockError::AddrInUse)
}

/// Send one datagram to ip:port. Blocking only for ARP resolution
pub fn sendto(fd: u64, ip: [u8; 4], port: u16, data: &[u8]) -> Result<usize, SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    if data.len() > super::udp::MAX_UDP_PAYLOAD {
        return Err(SockError::Invalid);
    }
    let src_port = udp_autobind(idx)?;

    if super::udp_send_resolved(src_port, &ip, port, data) {
        Ok(data.len())
    } else {
        Err(SockError::ConnRefused) // no route / ARP failed / driver error
    }
}

/// Receive one datagram. Blocking (up to OP_TIMEOUT_TICKS). Returns the
/// byte count (truncated to buf.len()) and the source address
pub fn recvfrom(fd: u64, buf: &mut [u8]) -> Result<(usize, [u8; 4], u16), SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;

    let (udp_h, peer) = {
        let tbl = SOCKETS.lock();
        let slot = &tbl.slots[idx];
        if !slot.active { return Err(SockError::BadFd); }
        if slot.kind != SockKind::Udp { return Err(SockError::Invalid); }
        match slot.udp_rx {
            Some(h) => (h, slot.udp_peer),
            None => return Err(SockError::NotConnected), // never bound: nothing can arrive
        }
    };

    let start = crate::fs::procfs::uptime_ticks();
    loop {
        if crate::net::CTRL_C.load(core::sync::atomic::Ordering::SeqCst) {
            return Err(SockError::WouldBlock);
        }

        rx::pump();
        while let Some(dgram) = rx::udp_take(udp_h) {
            // A connected UDP socket only sees datagrams from its peer
            if let Some((pip, pport)) = peer {
                if dgram.src_ip != pip || dgram.src_port != pport {
                    continue;
                }
            }
            let n = dgram.data.len().min(buf.len());
            buf[..n].copy_from_slice(&dgram.data[..n]);
            return Ok((n, dgram.src_ip, dgram.src_port));
        }

        if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= OP_TIMEOUT_TICKS {
            return Err(SockError::WouldBlock);
        }
        crate::sched::yield_now();
    }
}
