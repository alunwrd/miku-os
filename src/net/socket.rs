// BSD-style socket layer for userspace
//
// This is the kernel side of the socket syscalls (see src/syscall/net.rs).
// It owns a fixed table of kernel sockets; each is exposed to userspace as
// a file descriptor in a dedicated high range (SOCK_FD_BASE..) that the VFS
// never hands out, so sys_read/sys_write/sys_close can tell a socket fd from
// an ordinary file fd by range alone and route accordingly.
//
// Design notes:
//   - Blocking semantics only (phase 1). connect/send/recv block the calling
//     thread, exactly like a default BSD socket without O_NONBLOCK.
//   - The global table mutex is NEVER held across a blocking network call:
//     send/recv/connect take the inner TcpSocket out of its slot (marking it
//     Busy), drop the lock, run the blocking op, then put it back. This keeps
//     the table responsive and avoids lock-order inversions with net::NET.
//   - Sockets are per-process. On process exit, close_all_for_pid frees every
//     socket the pid still owns (wired from free_process_resources).
//
// Phase 1 implements AF_INET / SOCK_STREAM (TCP) client sockets. UDP and the
// listen/accept server path are added later.

extern crate alloc;
use alloc::boxed::Box;
use spin::Mutex;

use super::tcp::TcpSocket;

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

/// A blocking recv/connect gives up after this many timer ticks so a dead
/// peer can never wedge a process forever (tick = 4 ms => 30 s)
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
}

#[derive(Clone, Copy, PartialEq)]
enum SockState {
    /// Allocated by socket(), no connection yet
    Unconnected,
    /// connect() succeeded, tcp is Some when not mid-operation
    Connected,
    /// A blocking op took the inner socket out; ops must not race
    Busy,
}

struct Slot {
    active: bool,
    owner_pid: u64,
    state: SockState,
    is_tcp: bool,
    tcp: Option<Box<TcpSocket>>,
}

impl Slot {
    const fn empty() -> Self {
        Slot {
            active: false,
            owner_pid: 0,
            state: SockState::Unconnected,
            is_tcp: true,
            tcp: None,
        }
    }
}

struct SocketTable {
    slots: [Slot; MAX_SOCKETS],
}

impl SocketTable {
    const fn new() -> Self {
        // Slot is not Copy (holds a Box), so build the array element-wise
        const EMPTY: Slot = Slot::empty();
        SocketTable { slots: [EMPTY; MAX_SOCKETS] }
    }
}

static SOCKETS: Mutex<SocketTable> = Mutex::new(SocketTable::new());

/// True if `fd` falls in the socket descriptor range
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

/// Allocate a TCP socket owned by `pid`. Returns the socket fd or None if the
/// table is full
pub fn create_tcp(pid: u64) -> Option<u64> {
    let mut tbl = SOCKETS.lock();
    for i in 0..MAX_SOCKETS {
        if !tbl.slots[i].active {
            tbl.slots[i] = Slot {
                active: true,
                owner_pid: pid,
                state: SockState::Unconnected,
                is_tcp: true,
                tcp: None,
            };
            return Some(idx_to_fd(i));
        }
    }
    None
}

/// Close and free the socket behind `fd`. Returns true if it was a live socket
pub fn close_fd(fd: u64) -> bool {
    let idx = match fd_to_idx(fd) { Some(i) => i, None => return false };

    // Take the inner socket out under the lock, then run the (blocking) FIN
    // handshake without holding the table lock
    let taken = {
        let mut tbl = SOCKETS.lock();
        if !tbl.slots[idx].active { return false; }
        let inner = tbl.slots[idx].tcp.take();
        tbl.slots[idx] = Slot::empty();
        inner
    };
    if let Some(mut t) = taken {
        t.close();
    }
    true
}

/// Free every socket still owned by `pid` (process-exit cleanup). Does not run
/// the graceful FIN handshake - the process is already gone, so we just drop
/// the connections (the peer will see an RST or time out, like a hard exit)
pub fn close_all_for_pid(pid: u64) {
    let mut tbl = SOCKETS.lock();
    for i in 0..MAX_SOCKETS {
        if tbl.slots[i].active && tbl.slots[i].owner_pid == pid {
            tbl.slots[i] = Slot::empty();
        }
    }
}

/// True if `fd` is a socket owned by `pid`
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
// Blocking operations (lock released during the network call)
// ----------------------------------------------------------------------

/// Connect the socket behind `fd` to `ip:port`. Blocking. Returns Ok on an
/// established connection, or an errno on failure
pub fn connect(fd: u64, ip: [u8; 4], port: u16) -> Result<(), SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;

    // Transition Unconnected -> Busy under the lock so a second connect/op
    // on the same fd cannot race the handshake
    {
        let mut tbl = SOCKETS.lock();
        let slot = &mut tbl.slots[idx];
        if !slot.active { return Err(SockError::BadFd); }
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

/// Take the inner TcpSocket out of a Connected slot for a blocking op.
/// Returns the boxed socket and leaves the slot marked Busy
fn take_connected(idx: usize) -> Result<Box<TcpSocket>, SockError> {
    let mut tbl = SOCKETS.lock();
    let slot = &mut tbl.slots[idx];
    if !slot.active { return Err(SockError::BadFd); }
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
    // else: closed under us - dropping `sock` runs its destructor
}

/// Send all of `data` on the socket. Blocking. Returns bytes sent or errno
pub fn send(fd: u64, data: &[u8]) -> Result<usize, SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
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

/// Receive up to `buf.len()` bytes. Blocking: waits until at least one byte
/// arrives or the peer closes (then returns 0 = EOF). Returns bytes read or
/// errno
pub fn recv(fd: u64, buf: &mut [u8]) -> Result<usize, SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    let mut sock = take_connected(idx)?;

    let start = crate::vfs::procfs::uptime_ticks();
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
        if crate::vfs::procfs::uptime_ticks().wrapping_sub(start) >= OP_TIMEOUT_TICKS {
            break;
        }
        crate::scheduler::yield_now();
    }

    return_socket(idx, sock);
    Ok(got)
}
