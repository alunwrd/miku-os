// BSD-style socket layer for userspace
//
// The kernel side of the socket syscalls (see `syscall::adapters::net`). It
// owns a fixed table of kernel sockets; each is exposed to userspace as a file
// descriptor in a dedicated high range (SOCK_FD_BASE..) that the VFS never
// hands out, so sys_read/sys_write/sys_close can tell a socket fd from a file
// fd by range alone.
//
//   - connect/send/recv/accept block the calling thread unless the socket is
//     `O_NONBLOCK`.
//   - The global table mutex is never held across a blocking network call:
//     ops take the inner TcpSocket/TcpListener out of the slot (marking it
//     Busy), drop the lock, run the blocking op, then put it back. This keeps
//     the table responsive and avoids lock-order inversions with net::NET.
//   - Sockets are per-process. On process exit, close_all_for_pid frees every
//     socket the pid still owns (called from free_process_resources).
//
// Supported: AF_INET SOCK_STREAM clients and servers, SOCK_DGRAM
// (bind/sendto/recvfrom, plus connect to pin a default peer), raw ICMP, AF_UNIX
// and AF_MAYLAND. RX for every network socket flows through the central demux
// (net::rx).

extern crate alloc;
use alloc::boxed::Box;
use alloc::string::String;
use spin::Mutex;

use super::rx;
use super::tcp::{Handshake, TcpSocket};
use super::tcp_listener::TcpListener;
use mikunet::stack::conn::Keepalive;

// Userspace address-family / socket-type constants (match Linux ABI)
pub const AF_INET: u16 = miku_abi::socket::AF_INET as u16;
/// The display protocol's transport. Not a network family: a connection to the
/// compositor is an fd to write requests to and read events from, with a
/// lifetime tied to the process, so it shares the fd range, the ownership check
/// and the exit cleanup
pub const AF_MAYLAND: u16 = miku_abi::socket::AF_MAYLAND as u16;
/// Local sockets: two processes on this machine, no network involved.
///
/// Here for the same reason as AF_MAYLAND: a descriptor with an owner, a
/// lifetime tied to the process and a place in the fd range
pub const AF_UNIX: u16 = miku_abi::socket::AF_UNIX as u16;
pub const SOCK_STREAM: u32 = miku_abi::socket::SOCK_STREAM as u32;
pub const SOCK_DGRAM: u32 = miku_abi::socket::SOCK_DGRAM as u32;
/// A socket carrying whole IP packets rather than a stream or a payload. Only
/// ICMP, for `ping` and `traceroute`
pub const SOCK_RAW: u32 = miku_abi::socket::SOCK_RAW as u32;
pub const IPPROTO_ICMP: u32 = 1;
pub const IPPROTO_TCP: u32 = 6;
pub const IPPROTO_UDP: u32 = 17;

/// setsockopt levels and names, the Linux numbers
pub const SOL_IP: u32 = 0;
pub const IP_TTL: u32 = miku_abi::inet::IP_TTL as u32;
pub const SOL_SOCKET: u32 = miku_abi::socket::SOL_SOCKET as u32;
pub const SO_KEEPALIVE: u32 = miku_abi::socket::SO_KEEPALIVE as u32;
/// The `IPPROTO_TCP` level's three keepalive numbers, in seconds and probes
pub const TCP_KEEPIDLE: u32 = miku_abi::inet::TCP_KEEPIDLE as u32;
pub const TCP_KEEPINTVL: u32 = miku_abi::inet::TCP_KEEPINTVL as u32;
pub const TCP_KEEPCNT: u32 = miku_abi::inet::TCP_KEEPCNT as u32;
/// What an IP packet leaves with when nothing said otherwise
pub const DEFAULT_TTL: u8 = 64;

/// Socket fds live above the VFS fd range (`MAX_OPEN_FILES`), so the two
/// namespaces never collide and routing is a range check
pub const SOCK_FD_BASE: usize = 4096;
// Machine-wide: the table is a flat array of slots, so this is the number of
// sockets on this computer at once, not per program
pub const MAX_SOCKETS:  usize = 256;

/// How many of those one process may spend on the display server
///
/// See `create_mayland`: without a per-process share, one program can empty the
/// table
pub const MAX_MAYLAND_PER_PID: usize = 4;

/// A blocking recv/connect/accept gives up after this many timer ticks so a
/// dead peer cannot wedge a process (tick = 4 ms => 30 s)
const OP_TIMEOUT_TICKS: u64 = 7500;

/// How long a local-socket wait sleeps when nothing wakes it (tick = 4 ms)
///
/// Every one of these waits has a wake site on the other side (a send, an
/// accept, a hangup), so this is insurance against a missed wakeup. The same
/// 100 ms the pipe path uses (`syscall::adapters::io`)
const LOCAL_BACKSTOP: u64 = 25;

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
    /// A non-blocking connect was started and has not finished
    InProgress,
    /// And was asked to start again before it did
    Already,
    /// The handshake ran out of time
    TimedOut,
}

#[derive(Clone, Copy, PartialEq)]
enum SockState {
    /// Allocated by socket(), no connection yet (TCP) / idle (listener)
    Unconnected,
    /// connect()/accept() succeeded, inner object is Some when not mid-op
    Connected,
    /// A blocking op took the inner object out; ops must not race
    Busy,
    /// TCP: the SYN is out and the handshake is not over. The socket is in the
    /// slot, and whoever looks at it next (`poll`, a second `connect`, `send`,
    /// `recv`) moves it along (`advance_connect`)
    Connecting,
}

/// Who the process at the other end of a local socket was, captured when the
/// connection was made.
///
/// Not looked up: a client that connects, is answered and exits leaves its pid
/// free for the next program, so asking about pid N later can describe another
/// process. This was written down before the server saw the connection
#[derive(Clone, Copy, Debug)]
pub struct PeerCred {
    pub pid:  u64,
    pub uid:  u16,
    pub gid:  u16,
    pub euid: u16,
    pub egid: u16,
    /// When that process started. Two processes can share a pid over the life
    /// of a machine; they cannot also share a start tick
    pub start_tick: u64,
}

impl PeerCred {
    fn of(pid: u64) -> Self {
        let c = crate::sched::cred_of(pid);
        Self {
            pid,
            uid:  c.uid,
            gid:  c.gid,
            euid: c.euid,
            egid: c.egid,
            start_tick: crate::sched::start_tick_of(pid),
        }
    }
}

/// How many descriptors may be in flight on one local socket at a time.
///
/// Small because a descriptor in flight is a vnode reference the receiver has
/// not taken, and an unbounded queue would let one process pin the vnode table
pub const MAX_INFLIGHT_FDS: usize = 8;

#[derive(Clone, Copy, PartialEq)]
enum SockKind {
    Tcp,
    Listener,
    Udp,
    /// A connection to the Mayland display server
    Mayland,
    /// One end of a connected local socket
    Unix,
    /// A local socket bound to a name and taking connections
    UnixListener,
    /// Raw ICMP: whole IP packets in and an ICMP message out
    Icmp,
    /// Not a socket: a second descriptor for one. See `dup_fd`
    Alias,
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
    /// ICMP queue handle in the RX demux, and the TTL this socket's packets
    /// leave with, which traceroute sets
    icmp_rx: Option<usize>,
    ttl: u8,
    /// `SO_KEEPALIVE` and `TCP_KEEP*`. Kept here as well as in the connection,
    /// because a program sets them before `connect` or while a blocking call
    /// has the connection out on loan, and the connection picks them up when it
    /// lands in the slot (`install_tcp`)
    keepalive: Keepalive,
    /// Mayland connection id, and events drained from the server but not yet
    /// handed to the reader: a recv with a small buffer must not lose the rest
    /// of a batch
    may_client: Option<u32>,
    may_pending: alloc::vec::Vec<u8>,

    // local sockets
    /// Bumped every time this slot is handed out. A peer link records the
    /// generation it was made against, so a slot closed and reused cannot be
    /// mistaken for the socket that lived in it
    gen: u64,
    /// The other end of a connected local socket
    peer: Option<(usize, u64)>,
    /// What the peer has written and this end has not read yet
    unix_rx: alloc::vec::Vec<u8>,
    /// The peer has gone: reads drain what is left and then report end of file
    unix_hup: bool,
    /// The name a listener is bound to, kept so it can be removed when the
    /// socket closes: otherwise the next bind() finds it there
    unix_path: Option<String>,
    /// The VFS node behind that name, which `connect` matches on: two paths can
    /// name one node
    unix_node: Option<usize>,
    /// Connections waiting to be accepted, as (slot, generation)
    unix_backlog: alloc::vec::Vec<(usize, u64)>,
    /// Who this end was when the connection was made
    my_cred: Option<PeerCred>,
    /// Who the far end was when the connection was made: what peercred(2)
    /// answers
    peer_cred: Option<PeerCred>,
    /// `O_NONBLOCK`, as `fcntl`(113) sets it. Sockets are not in the VFS
    /// descriptor table, so `OpenFlags::NONBLOCK` has nowhere to live here, and
    /// every reactor written for Unix opens a socket and then makes it
    /// non-blocking
    nonblocking: bool,
    /// `FD_CLOEXEC`. A socket never crosses a `fork` (it is owned by one pid)
    /// but survives an `exec`, which keeps the pid, and `exec` closes what
    /// carries this (`close_cloexec_for_pid`)
    cloexec: bool,
    /// An error that happened to this socket when nobody was calling it (a
    /// non-blocking connect that failed), kept for `SO_ERROR`, which reads and
    /// clears it
    so_error: Option<SockError>,
    /// How many alias descriptors (`dup_fd`) name this socket. It is not
    /// released while any do, whatever happens to its own descriptor
    refs: u32,
    /// Descriptors handed over by the peer and not yet received. In the
    /// receiver's slot, like unix_rx, since the table owns the slot's lifetime.
    /// Each holds a vnode reference until it is taken or the slot is released
    pending_fds: alloc::vec::Vec<crate::fs::vfs::fd::OpenFile>,
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
            icmp_rx: None,
            ttl: DEFAULT_TTL,
            keepalive: Keepalive::DEFAULT,
            may_client: None,
            may_pending: alloc::vec::Vec::new(),
            gen: 0,
            peer: None,
            unix_rx: alloc::vec::Vec::new(),
            unix_hup: false,
            unix_path: None,
            unix_node: None,
            unix_backlog: alloc::vec::Vec::new(),
            my_cred: None,
            peer_cred: None,
            nonblocking: false,
            cloexec: false,
            so_error: None,
            refs: 0,
            pending_fds: alloc::vec::Vec::new(),
        }
    }

    /// Releases demux resources held by this slot (TcpSocket/TcpListener
    /// unregister themselves on drop; UDP and ICMP handles are freed here)
    ///
    /// Returns what the caller has to dispose of outside the table lock: the
    /// name a local listener was bound to, to unlink, and any descriptors in
    /// flight that will now never be received, which hold vnode references
    fn release(&mut self) -> (Option<String>, alloc::vec::Vec<crate::fs::vfs::fd::OpenFile>) {
        if let Some(h) = self.udp_rx.take() {
            rx::unregister_udp(h);
        }
        // Eight ICMP waiters exist in total, and leaking one costs every later
        // ping
        if let Some(h) = self.icmp_rx.take() {
            rx::unregister_icmp(h);
        }
        if let Some(c) = self.may_client.take() {
            crate::mayland::server::disconnect(c);
        }
        let name = self.unix_path.take();
        let orphans = core::mem::take(&mut self.pending_fds);
        self.tcp = None;      // Drop unregisters the 4-tuple
        self.listener = None; // Drop unregisters the listen port
        *self = Slot::empty();
        (name, orphans)
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

/// Descriptor -> slot, for everything that acts on the socket.
///
/// A socket descriptor is `SOCK_FD_BASE` plus a slot index, so a second
/// descriptor for one socket (`dup`) needs a slot of its own to have a number.
/// That slot is an alias: `ALIAS[j] = i + 1` says descriptor `j` names the
/// socket in slot `i`, and this is where it is followed, so every operation
/// below works through an alias unawares.
///
/// `GONE[i]`: slot `i`'s own descriptor was closed while aliases still name the
/// socket, so the number is dead but the socket is not. Both are atomics rather
/// than slot fields so this is callable with the table lock held or not.
#[inline]
fn fd_to_idx(fd: u64) -> Option<usize> {
    let raw = raw_idx(fd)?;
    let a = ALIAS[raw].load(core::sync::atomic::Ordering::Acquire);
    if a != 0 { return Some(a - 1); }
    if GONE[raw].load(core::sync::atomic::Ordering::Acquire) { return None; }
    Some(raw)
}

/// Descriptor -> its own slot, alias or not, for what belongs to the descriptor
/// rather than the socket: `FD_CLOEXEC`, and closing it
#[inline]
fn raw_idx(fd: u64) -> Option<usize> {
    if is_socket_fd(fd) { Some(fd as usize - SOCK_FD_BASE) } else { None }
}

static ALIAS: [core::sync::atomic::AtomicUsize; MAX_SOCKETS] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; MAX_SOCKETS];
static GONE: [core::sync::atomic::AtomicBool; MAX_SOCKETS] =
    [const { core::sync::atomic::AtomicBool::new(false) }; MAX_SOCKETS];

/// A second descriptor for the socket `fd` names: `dup`(28) and `F_DUPFD` on a
/// socket.
///
/// As POSIX's `dup`: it shares the open description and gives the new number
/// its own descriptor flags. `O_NONBLOCK` lives on the socket and is shared,
/// `FD_CLOEXEC` lives on the alias slot and starts clear. The socket is
/// released when the last descriptor naming it is closed.
///
/// The number is the lowest free slot: socket numbers are a range of their own,
/// and `F_DUPFD`'s floor is about the VFS table
pub fn dup_fd(fd: u64, pid: u64) -> Result<u64, SockError> {
    let target = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    let mut tbl = SOCKETS.lock();
    let t = &tbl.slots[target];
    if !t.active || t.owner_pid != pid || t.kind == SockKind::Alias {
        return Err(SockError::BadFd);
    }
    let j = (0..MAX_SOCKETS).find(|&j| !tbl.slots[j].active).ok_or(SockError::TableFull)?;
    tbl.slots[j] = Slot::empty();
    tbl.slots[j].active = true;
    tbl.slots[j].owner_pid = pid;
    tbl.slots[j].kind = SockKind::Alias;
    tbl.slots[target].refs += 1;
    ALIAS[j].store(target + 1, core::sync::atomic::Ordering::Release);
    Ok(idx_to_fd(j))
}

#[inline]
fn idx_to_fd(idx: usize) -> u64 {
    (SOCK_FD_BASE + idx) as u64
}

/// Is this socket marked `O_NONBLOCK`?
///
/// False for anything not a live socket, including a closed descriptor: every
/// operation below checks `active` for itself anyway
pub fn is_nonblocking(fd: u64) -> bool {
    match fd_to_idx(fd) {
        Some(idx) => {
            let tbl = SOCKETS.lock();
            tbl.slots[idx].active && tbl.slots[idx].nonblocking
        }
        None => false,
    }
}

/// The same by slot index, for operations that have resolved one.
///
/// Called before taking the table lock, never inside it: `SOCKETS` is a
/// spinlock and not reentrant
fn nonblocking_at(idx: usize) -> bool {
    let tbl = SOCKETS.lock();
    tbl.slots[idx].active && tbl.slots[idx].nonblocking
}

/// Set or clear it. What `fcntl(F_SETFL)` does to a socket.
pub fn set_nonblocking(fd: u64, on: bool) -> Result<(), SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    let mut tbl = SOCKETS.lock();
    if !tbl.slots[idx].active {
        return Err(SockError::BadFd);
    }
    tbl.slots[idx].nonblocking = on;
    Ok(())
}

/// Is this socket marked `FD_CLOEXEC`? False for anything not live, as above
pub fn is_cloexec(fd: u64) -> bool {
    if fd_to_idx(fd).is_none() { return false; }
    match raw_idx(fd) {
        Some(idx) => {
            let tbl = SOCKETS.lock();
            tbl.slots[idx].active && tbl.slots[idx].cloexec
        }
        None => false,
    }
}

/// Set or clear it. What `fcntl(F_SETFD)` does to a socket.
pub fn set_cloexec(fd: u64, on: bool) -> Result<(), SockError> {
    fd_to_idx(fd).ok_or(SockError::BadFd)?;
    // The descriptor's own slot: an alias has its own flag, as a `dup`'d
    // descriptor does on any Unix
    let idx = raw_idx(fd).ok_or(SockError::BadFd)?;
    let mut tbl = SOCKETS.lock();
    if !tbl.slots[idx].active {
        return Err(SockError::BadFd);
    }
    tbl.slots[idx].cloexec = on;
    Ok(())
}

/// May this operation go to sleep?
///
/// The one place a socket operation decides that, called before every wait in
/// this file. A check rather than a wrapper, because the waits differ: a TCP
/// receive naps a tick against the RX pump, a local socket parks on `PIPE`
/// until its peer writes.
///
/// `EAGAIN` and not a timeout: `WouldBlock` is what `syscall::adapters::net`
/// maps to `EAGAIN`, and the thirty-second deadline answers the same: both mean
/// try again
#[inline]
fn may_block(nonblocking: bool) -> Result<(), SockError> {
    if nonblocking { Err(SockError::WouldBlock) } else { Ok(()) }
}

// Slot allocation / lifecycle

/// Handed out with every slot, never reused. See `Slot::gen`
static SOCK_GEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

/// The most sockets one unprivileged user may hold at once.
///
/// The table is machine-wide, so a user's program could fill it, and the
/// display server, whose client connections come out of this table, could not
/// accept anybody. CAP_SYS_ADMIN is above the limit, so a system service is not
/// refused. All of a desktop session is under one uid: maywm, aosh and mayterm
/// each hold a display connection (up to MAX_MAYLAND_PER_PID), and every
/// application holds one or more. Raised with the table, in the same
/// proportion: it only works if it stays well under the total
const MAX_SOCKETS_PER_UID: usize = 160;

fn create(pid: u64, kind: SockKind) -> Option<u64> {
    let cred = crate::sched::cred_of(pid);
    let mut tbl = SOCKETS.lock();

    if !cred.has(crate::process::cred::CAP_SYS_ADMIN) {
        let mine = tbl.slots.iter()
            .filter(|s| s.active && crate::sched::uid_of(s.owner_pid) == cred.uid)
            .count();
        if mine >= MAX_SOCKETS_PER_UID {
            return None;
        }
    }

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
                icmp_rx: None,
                ttl: DEFAULT_TTL,
                keepalive: Keepalive::DEFAULT,
                may_client: None,
                may_pending: alloc::vec::Vec::new(),
                gen: SOCK_GEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed),
                peer: None,
                unix_rx: alloc::vec::Vec::new(),
                unix_hup: false,
                unix_path: None,
                unix_node: None,
                unix_backlog: alloc::vec::Vec::new(),
                my_cred: None,
                peer_cred: None,
                // Blocking: `fcntl` changes it, never `socket()`
                nonblocking: false,
                cloexec: false,
                so_error: None,
                refs: 0,
                pending_fds: alloc::vec::Vec::new(),
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

/// Allocates a raw ICMP socket owned by 'pid'
///
/// Registers with the same demux every ICMP consumer uses. Every waiter gets a
/// copy of every non-echo-request message: ICMP has no ports, so each consumer
/// filters for the replies to what it sent
pub fn create_icmp(pid: u64) -> Option<u64> {
    let handle = rx::register_icmp()?;
    let fd = match create(pid, SockKind::Icmp) {
        Some(fd) => fd,
        None => {
            rx::unregister_icmp(handle);
            return None;
        }
    };
    let idx = fd_to_idx(fd)?;
    let mut tbl = SOCKETS.lock();
    tbl.slots[idx].icmp_rx = Some(handle);
    tbl.slots[idx].state = SockState::Connected;
    Some(fd)
}

/// Sets the TTL this socket's packets leave with
///
/// Traceroute sends probes with a rising TTL and reads the time-exceeded
/// messages that come back
pub fn set_ttl(fd: u64, ttl: u8) -> Result<(), SockError> {
    if ttl == 0 {
        // A packet with TTL 0 is discarded by the first router, and is usually
        // a caller counting from zero
        return Err(SockError::Invalid);
    }
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    let mut tbl = SOCKETS.lock();
    let slot = &mut tbl.slots[idx];
    if !slot.active { return Err(SockError::BadFd); }
    slot.ttl = ttl;
    Ok(())
}

/// Puts a TCP connection into its slot, carrying over the keepalive setting the
/// slot holds. Compared first, so a connection coming back from a blocking call
/// does not have its idle timer restarted when nothing changed
fn install_tcp(slot: &mut Slot, mut sock: Box<TcpSocket>) {
    if sock.keepalive() != slot.keepalive {
        sock.set_keepalive(slot.keepalive);
    }
    slot.tcp = Some(sock);
}

/// `SO_KEEPALIVE` (on or off), `TCP_KEEPIDLE`/`TCP_KEEPINTVL` (seconds) and
/// `TCP_KEEPCNT` (probes): `setsockopt`(80). TCP only. Linux's bounds: a time
/// of at least a second and at most 32767, at most 127 probes
pub fn set_keepalive_opt(fd: u64, pid: u64, level: u32, name: u32, value: u64) -> Result<(), SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    let mut tbl = SOCKETS.lock();
    let slot = &mut tbl.slots[idx];
    if !slot.active || slot.owner_pid != pid { return Err(SockError::BadFd); }
    if slot.kind != SockKind::Tcp { return Err(SockError::Invalid); }
    let mut k = slot.keepalive;
    match (level, name) {
        (SOL_SOCKET, SO_KEEPALIVE) => k.enabled = value != 0,
        (IPPROTO_TCP, TCP_KEEPIDLE | TCP_KEEPINTVL) => {
            if !(1..=32767).contains(&value) { return Err(SockError::Invalid); }
            if name == TCP_KEEPIDLE { k.idle_ms = value * 1000 } else { k.interval_ms = value * 1000 }
        }
        (IPPROTO_TCP, TCP_KEEPCNT) => {
            if !(1..=127).contains(&value) { return Err(SockError::Invalid); }
            k.count = value as u32;
        }
        _ => return Err(SockError::Invalid),
    }
    slot.keepalive = k;
    if let Some(sock) = slot.tcp.as_mut() {
        sock.set_keepalive(k);
    }
    Ok(())
}

pub fn ttl_of(fd: u64) -> u8 {
    fd_to_idx(fd)
        .map(|idx| SOCKETS.lock().slots[idx].ttl)
        .unwrap_or(DEFAULT_TTL)
}

/// Whether this fd is a raw ICMP socket, so the syscall layer can route
/// sendto/recvfrom to the right place
pub fn is_icmp(fd: u64) -> bool {
    match fd_to_idx(fd) {
        Some(idx) => {
            let tbl = SOCKETS.lock();
            tbl.slots[idx].active && tbl.slots[idx].kind == SockKind::Icmp
        }
        None => false,
    }
}

/// Sends one ICMP message to `ip`, wrapped in an IP header this builds
///
/// The program supplies the ICMP message (type, code, checksum and body); the
/// IP header is the kernel's, as for UDP and for a Linux raw socket without
/// `IP_HDRINCL`
pub fn icmp_send(fd: u64, ip: [u8; 4], msg: &[u8]) -> Result<usize, SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    if msg.len() < 8 || msg.len() > 1400 {
        return Err(SockError::Invalid);
    }
    let ttl = {
        let tbl = SOCKETS.lock();
        let slot = &tbl.slots[idx];
        if !slot.active || slot.kind != SockKind::Icmp {
            return Err(SockError::Invalid);
        }
        slot.ttl
    };

    if super::ip_send_ttl(&ip, super::ipv4::PROTO_ICMP, msg, ttl) {
        Ok(msg.len())
    } else {
        Err(SockError::ConnRefused) // no route, ARP failed, or the ring is full
    }
}

/// One received IP packet carrying ICMP, and the address it came from
///
/// Blocking, up to the same timeout every other socket operation uses
pub fn icmp_recv(fd: u64, buf: &mut [u8]) -> Result<(usize, [u8; 4], u16), SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    let handle = {
        let tbl = SOCKETS.lock();
        let slot = &tbl.slots[idx];
        if !slot.active || slot.kind != SockKind::Icmp {
            return Err(SockError::Invalid);
        }
        slot.icmp_rx.ok_or(SockError::NotConnected)?
    };

    let nb = is_nonblocking(fd);
    let start = crate::fs::procfs::uptime_ticks();
    loop {
        if crate::net::CTRL_C.load(core::sync::atomic::Ordering::SeqCst) {
            return Err(SockError::WouldBlock);
        }
        rx::pump();
        if let Some(ev) = rx::icmp_take(handle) {
            let n = ev.packet.len().min(buf.len());
            buf[..n].copy_from_slice(&ev.packet[..n]);
            // Port 0: ICMP has none, and a caller reading the address wants
            // the host that answered
            return Ok((n, ev.src_ip, 0));
        }
        if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= OP_TIMEOUT_TICKS {
            return Err(SockError::WouldBlock);
        }
        may_block(nb)?;
        let deadline = crate::kcore::time::now_ns().saturating_add(crate::net::irq::backstop_ns());
        crate::sched::wait::keyed(crate::sched::wait::key::net_icmp(handle))
            .wait_unless(deadline, || rx::icmp_pending(handle));
    }
}

/// Opens a connection to the Mayland display server
///
/// No connect() step and no address: one machine has one display server. The
/// connection is live when this returns
pub fn create_mayland(pid: u64) -> Option<u64> {
    // The table is system-wide, so one process opening connections in a loop
    // would leave no socket of any kind for anyone. A desktop program wants one
    // connection; four allows for reconnecting before the last one closed
    if mayland_connections(pid) >= MAX_MAYLAND_PER_PID {
        return None;
    }
    let fd = create(pid, SockKind::Mayland)?;
    let idx = fd_to_idx(fd)?;
    let client = crate::mayland::server::connect(pid);
    let mut tbl = SOCKETS.lock();
    tbl.slots[idx].may_client = Some(client);
    tbl.slots[idx].state = SockState::Connected;
    Some(fd)
}

/// How many display-server connections this process already has open
fn mayland_connections(pid: u64) -> usize {
    let tbl = SOCKETS.lock();
    tbl.slots.iter()
        .filter(|s| s.active && s.owner_pid == pid && s.kind == SockKind::Mayland)
        .count()
}

/// The connection id behind a Mayland socket, for the pool syscalls
pub fn mayland_client(fd: u64, pid: u64) -> Option<u32> {
    let idx = fd_to_idx(fd)?;
    let tbl = SOCKETS.lock();
    let slot = &tbl.slots[idx];
    if slot.active && slot.owner_pid == pid && slot.kind == SockKind::Mayland {
        slot.may_client
    } else {
        None
    }
}

fn may_client_of(idx: usize) -> Result<u32, SockError> {
    let tbl = SOCKETS.lock();
    let slot = &tbl.slots[idx];
    if !slot.active { return Err(SockError::BadFd); }
    slot.may_client.ok_or(SockError::NotConnected)
}

/// Requests go straight into the server's dispatch. There is no queue and no
/// partial write: the server consumes whole messages and keeps any tail
fn may_send(idx: usize, data: &[u8]) -> Result<usize, SockError> {
    let client = may_client_of(idx)?;
    crate::mayland::server::submit(client, data);
    Ok(data.len())
}

/// Events, blocking until there are some
///
/// A short buffer is not an error: whatever the server handed over stays in the
/// slot until read. Splitting a message across two reads is fine, the client
/// reassembles as for any stream
fn may_recv(idx: usize, buf: &mut [u8]) -> Result<usize, SockError> {
    let client = may_client_of(idx)?;
    let nb = nonblocking_at(idx);
    let start = crate::fs::procfs::uptime_ticks();

    loop {
        let more = crate::mayland::server::drain(client);
        {
            let mut tbl = SOCKETS.lock();
            let slot = &mut tbl.slots[idx];
            if !slot.active { return Err(SockError::BadFd); }
            if !more.is_empty() {
                slot.may_pending.extend_from_slice(&more);
            }
            if !slot.may_pending.is_empty() {
                let n = slot.may_pending.len().min(buf.len());
                buf[..n].copy_from_slice(&slot.may_pending[..n]);
                slot.may_pending.drain(..n);
                return Ok(n);
            }
        }
        // Dead and drained: reports EOF rather than a timeout, so a client
        // killed by a protocol error learns it promptly. The error event was
        // queued before the connection died and has been read by now
        if !crate::mayland::server::is_alive(client) {
            return Ok(0);
        }
        if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= OP_TIMEOUT_TICKS {
            return Err(SockError::WouldBlock);
        }
        // Off the run queue until the compositor has something: a client
        // waiting for its next frame must not spin here taking the compositor's
        // global lock. `server::wake_readers` ends this sleep when a frame
        // queues anything; the tick count below is for a missed wake
        may_block(nb)?;
        crate::sched::sleep(MAY_WAIT_BACKSTOP);
    }
}

/// How long a Mayland reader sleeps when nobody wakes it (ticks, 4 ms each)
///
/// `server::wake_readers` is the mechanism, and this is not reached on a
/// working desktop. A missing wake costs a hundred milliseconds of latency
const MAY_WAIT_BACKSTOP: u64 = 25;

/// Close and free the socket behind 'fd'. Returns true if it was a live socket
pub fn close_fd(fd: u64) -> bool {
    let raw = match raw_idx(fd) { Some(i) => i, None => return false };
    let idx = {
        let mut tbl = SOCKETS.lock();
        let a = ALIAS[raw].load(core::sync::atomic::Ordering::Acquire);
        if a != 0 {
            // An alias: the slot goes, and the socket goes with it only if
            // this was the last descriptor and its own is already closed
            let target = a - 1;
            ALIAS[raw].store(0, core::sync::atomic::Ordering::Release);
            tbl.slots[raw] = Slot::empty();
            let t = &mut tbl.slots[target];
            t.refs = t.refs.saturating_sub(1);
            if t.refs > 0 || !GONE[target].load(core::sync::atomic::Ordering::Acquire) {
                return true;
            }
            GONE[target].store(false, core::sync::atomic::Ordering::Release);
            target
        } else {
            if GONE[raw].load(core::sync::atomic::Ordering::Acquire) { return false; }
            if !tbl.slots[raw].active { return false; }
            if tbl.slots[raw].refs > 0 {
                // Its own number is closed; the socket lives on through the
                // aliases, and is released by the last of them
                GONE[raw].store(true, core::sync::atomic::Ordering::Release);
                return true;
            }
            raw
        }
    };
    release_socket(idx)
}

/// Closes the socket in slot `idx` for good, every descriptor naming it gone
fn release_socket(idx: usize) -> bool {

    // Take the inner socket out under the lock, then run the (blocking) FIN
    // handshake without holding the table lock
    let (taken, name, orphans) = {
        let mut tbl = SOCKETS.lock();
        if !tbl.slots[idx].active { return false; }
        // Before the slot is cleared and handed on: the far end of a local
        // connection has to learn this one is gone
        detach_unix(&mut tbl, idx);
        let inner = tbl.slots[idx].tcp.take();
        let (name, orphans) = tbl.slots[idx].release();
        (inner, name, orphans)
    };
    // Outside the table lock: unlinking takes the VFS lock, and so does
    // letting go of a descriptor nobody will now receive
    drop_inflight(orphans);
    if let Some(path) = name {
        if let Some((dir, base)) = split_path(&path) {
            crate::fs::vfs::core::unlink_path(dir, base);
        }
    }
    if let Some(mut t) = taken {
        t.close();
    }
    // A peer that was reading, or a poller watching this descriptor, has a
    // hangup to notice
    crate::sched::wait::PIPE.wake_all();
    crate::sched::wait::wake_pollers();
    true
}

/// Frees every socket still owned by 'pid' (process-exit cleanup). No graceful
/// FIN handshake, since the process is gone: the peer sees an RST or times out,
/// as after a hard exit
pub fn close_all_for_pid(pid: u64) {
    close_for_pid(pid, false)
}

/// Free the sockets 'pid' marked `FD_CLOEXEC` - what `exec` does, beside the
/// VFS's own `close_cloexec_fds`
pub fn close_cloexec_for_pid(pid: u64) {
    close_for_pid(pid, true)
}

fn close_for_pid(pid: u64, only_cloexec: bool) {
    if only_cloexec {
        // One descriptor at a time, through `close_fd`, because closing one
        // is not closing its socket when another descriptor still names it
        let fds: alloc::vec::Vec<u64> = {
            let tbl = SOCKETS.lock();
            (0..MAX_SOCKETS)
                .filter(|&i| {
                    let s = &tbl.slots[i];
                    s.active && s.owner_pid == pid && s.cloexec
                        && !GONE[i].load(core::sync::atomic::Ordering::Acquire)
                })
                .map(idx_to_fd)
                .collect()
        };
        for fd in fds { close_fd(fd); }
        return;
    }
    let (names, orphans) = {
        let mut tbl = SOCKETS.lock();
        let mut names = alloc::vec::Vec::new();
        let mut orphans = alloc::vec::Vec::new();
        // The process is gone, and every descriptor with it: sockets and their
        // aliases together
        for i in 0..MAX_SOCKETS {
            if tbl.slots[i].active && tbl.slots[i].owner_pid == pid {
                ALIAS[i].store(0, core::sync::atomic::Ordering::Release);
                GONE[i].store(false, core::sync::atomic::Ordering::Release);
                detach_unix(&mut tbl, i);
                let (n, o) = tbl.slots[i].release();
                if let Some(n) = n { names.push(n); }
                orphans.extend(o);
            }
        }
        (names, orphans)
    };
    drop_inflight(orphans);
    for path in names {
        if let Some((dir, base)) = split_path(&path) {
            crate::fs::vfs::core::unlink_path(dir, base);
        }
    }
    crate::sched::wait::PIPE.wake_all();
    crate::sched::wait::wake_pollers();
}

/// The queue a wake for this socket lands on (`sched::wait::keyed`): its
/// client's, for a connection to the compositor; its slot's, for a local
/// socket. None for a network socket, reached by the receive path's wake of
/// every poller
pub fn wait_key(fd: u64) -> Option<u64> {
    use crate::sched::wait::key;
    let idx = fd_to_idx(fd)?;
    let tbl = SOCKETS.lock();
    let slot = &tbl.slots[idx];
    if !slot.active { return None; }
    match slot.kind {
        SockKind::Mayland => slot.may_client.map(key::mayland),
        SockKind::Unix | SockKind::UnixListener => Some(key::socket(idx)),
        _ => None,
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

// bind / listen / accept

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
        // A local socket is bound to a name, not a port: see `unix_bind`
        SockKind::Unix | SockKind::UnixListener => Err(SockError::Invalid),
        // ICMP has no ports, so there is nothing an address could bind to
        SockKind::Icmp => Err(SockError::Invalid),
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
        // A display connection has no address to bind
        SockKind::Mayland => Err(SockError::NotSupported),
        // Unreachable: `fd_to_idx` resolves an alias to its socket first
        SockKind::Alias => Err(SockError::BadFd),
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

    // Zero ticks for a non-blocking listener: `accept_wait` looks once and
    // answers `None`, which becomes `EAGAIN` below. The listener still has to
    // go back in its slot afterwards, so this is a budget rather than an early
    // return
    let conn = listener.accept_wait(if is_nonblocking(fd) { 0 } else { OP_TIMEOUT_TICKS });

    // Put the listener back (unless the fd was closed while we blocked)
    {
        let mut tbl = SOCKETS.lock();
        let slot = &mut tbl.slots[idx];
        if slot.active && slot.state == SockState::Busy {
            slot.listener = Some(listener);
            slot.state = SockState::Unconnected;
        }
        // Otherwise dropped here, and Drop unregisters the listen port
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
        install_tcp(slot, Box::new(conn.into_socket()));
        slot.state = SockState::Connected;
    }
    Ok((new_fd, peer_ip, peer_port))
}

// connect

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
            // Every ICMP send names its own destination, so there is no peer to
            // remember
            SockKind::Icmp => return Err(SockError::Invalid),
            // Connected when the socket was created: there is one display
            // server
            SockKind::Mayland => return Err(SockError::AlreadyConnected),
            // Reached by address, and a local socket has none: see
            // `unix_connect`
            SockKind::Unix | SockKind::UnixListener => return Err(SockError::Invalid),
            // Unreachable: `fd_to_idx` resolves an alias to its socket first
            SockKind::Alias => return Err(SockError::BadFd),
            SockKind::Tcp => {}
        }
        // Transition Unconnected -> Busy under the lock so a second
        // connect/op on the same fd cannot race the handshake
        match slot.state {
            SockState::Unconnected => slot.state = SockState::Busy,
            SockState::Connected   => return Err(SockError::AlreadyConnected),
            SockState::Busy        => return Err(SockError::Busy),
            // A second `connect` on a socket still connecting: POSIX's
            // answers, from where the handshake has got to
            SockState::Connecting => {
                return match advance_connect(slot) {
                    SockState::Connected => Err(SockError::AlreadyConnected),
                    SockState::Connecting => Err(SockError::Already),
                    _ => Err(slot.so_error.take().unwrap_or(SockError::ConnRefused)),
                };
            }
        }
    }

    // A non-blocking TCP connect sends the SYN and returns `EINPROGRESS`.
    //
    // The socket waits in the slot as `Connecting`, and whoever looks at it
    // next moves the handshake along: `poll`, which a reactor calls, reports it
    // writable once established, or writable with a hangup if it failed, and
    // the failure waits in `so_error` for `SO_ERROR`
    if is_nonblocking(fd) {
        let started = TcpSocket::begin_connect(ip, port);
        let mut tbl = SOCKETS.lock();
        let slot = &mut tbl.slots[idx];
        if !slot.active { return Err(SockError::BadFd); }
        return match started {
            Some(sock) => {
                install_tcp(slot, Box::new(sock));
                slot.state = SockState::Connecting;
                Err(SockError::InProgress)
            }
            None => {
                slot.state = SockState::Unconnected;
                Err(SockError::ConnRefused)
            }
        };
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
            install_tcp(slot, Box::new(sock));
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

// TCP send/recv (lock released during the network call)

/// Moves a `Connecting` socket's handshake along by whatever has arrived, and
/// answers with the state afterwards. Does not block: `handshake_step` drains
/// the demux and runs the clock, as `poll` does for a connected socket under
/// this lock.
///
/// A failure is kept in `so_error` rather than returned, because the caller
/// noticing it is usually `poll`, and the one who asked finds out through
/// `SO_ERROR` or a second `connect`
fn advance_connect(slot: &mut Slot) -> SockState {
    // TCP's handshake only. A local socket is `Connecting` too, but the
    // server's `accept` moves it along
    if slot.state != SockState::Connecting || slot.kind != SockKind::Tcp { return slot.state; }
    let step = match slot.tcp.as_mut() {
        Some(sock) => sock.handshake_step(),
        None => Handshake::Failed,
    };
    match step {
        Handshake::Done => slot.state = SockState::Connected,
        Handshake::Pending => {}
        Handshake::Failed => {
            let timed_out = slot.tcp.as_ref()
                .is_some_and(|t| t.state() != super::tcp::TcpState::Closed);
            slot.tcp = None;
            slot.state = SockState::Unconnected;
            slot.so_error = Some(if timed_out { SockError::TimedOut } else { SockError::ConnRefused });
        }
    }
    slot.state
}

/// Take the inner TcpSocket out of a Connected slot for a blocking op.
/// Returns the boxed socket and leaves the slot marked Busy
fn take_connected(idx: usize) -> Result<Box<TcpSocket>, SockError> {
    let mut tbl = SOCKETS.lock();
    let slot = &mut tbl.slots[idx];
    if !slot.active { return Err(SockError::BadFd); }
    if slot.kind != SockKind::Tcp {
        return Err(SockError::Invalid);
    }
    // A socket still connecting is given one step; a send or recv on it answers
    // `EAGAIN` until the handshake is over. A blocking caller cannot be here:
    // its `connect` returned only when it finished
    if slot.state == SockState::Connecting {
        match advance_connect(slot) {
            SockState::Connecting => return Err(SockError::WouldBlock),
            SockState::Unconnected => {
                return Err(slot.so_error.take().unwrap_or(SockError::NotConnected));
            }
            _ => {}
        }
    }
    match slot.state {
        SockState::Connected => {}
        SockState::Unconnected | SockState::Connecting => return Err(SockError::NotConnected),
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

/// Is this descriptor a local socket, connected or listening?
///
/// The syscall layer asks to decide whether `bind`/`listen`/`accept` mean the
/// network ones or the local ones: `listen` and `accept` carry no address
pub fn is_local(fd: u64) -> bool {
    match fd_to_idx(fd) {
        Some(idx) => {
            let tbl = SOCKETS.lock();
            let s = &tbl.slots[idx];
            s.active && matches!(s.kind, SockKind::Unix | SockKind::UnixListener)
        }
        None => false,
    }
}

/// How many socket slots are live, for anyone reporting on the system's state
pub fn in_use() -> usize {
    SOCKETS.lock().slots.iter().filter(|s| s.active).count()
}

/// What a descriptor can do right now, without blocking
///
/// `hup` is not the opposite of `readable`: a closed peer leaves a socket
/// readable (the read returns end-of-file at once), and a poller treating
/// hangup as "not ready" would wait for a peer that has gone
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Readiness {
    pub readable: bool,
    pub writable: bool,
    /// The peer is gone
    pub hup: bool,
    /// Nothing is open on this descriptor
    pub invalid: bool,
}

impl Readiness {
    pub fn invalid() -> Self {
        Readiness { invalid: true, ..Default::default() }
    }
}

/// Would an operation on this socket make progress without sleeping?
///
/// Each branch asks what the matching blocking op asks: a poll saying readable
/// followed by a recv that blocks is worse than no poll.
///
/// A TCP socket's readiness lives in `rx_buf`, and bytes reach `rx_buf` only
/// when someone drains the demux into it, so this runs `recv_one`, as `recv`
/// does
pub fn poll_fd(fd: u64) -> Readiness {
    let idx = match fd_to_idx(fd) { Some(i) => i, None => return Readiness::invalid() };

    // Everything but TCP is answerable under the table lock alone, so it is
    // taken once and what TCP needs is carried out of it
    let (kind, state, listener_ready, udp_ready, may_client, may_queued) = {
        let tbl = SOCKETS.lock();
        let slot = &tbl.slots[idx];
        if !slot.active { return Readiness::invalid(); }
        (
            slot.kind,
            slot.state,
            // A listener taken out of its slot by an in-flight accept reads
            // as not-ready, the same as a busy TCP socket below
            slot.listener.as_ref().is_some_and(|l| l.pending()),
            slot.udp_rx.is_some_and(rx::udp_pending),
            slot.may_client,
            !slot.may_pending.is_empty(),
        )
    };

    match kind {
        SockKind::Listener => Readiness {
            // A listener is "readable" when an accept would return at once
            readable: listener_ready,
            ..Default::default()
        },

        // A datagram socket can always be written to; there is no connection
        // to be backed up
        SockKind::Udp => Readiness { readable: udp_ready, writable: true, ..Default::default() },

        // Same shape, and readable when the demux has queued a message for this
        // waiter. Every ICMP consumer gets a copy of every message, so this
        // means something arrived, not something for this socket: the program
        // recognises its own replies
        SockKind::Icmp => {
            let handle = SOCKETS.lock().slots[idx].icmp_rx;
            Readiness {
                readable: handle.is_some_and(rx::icmp_pending),
                writable: true,
                ..Default::default()
            }
        }

        SockKind::UnixListener => {
            let tbl = SOCKETS.lock();
            Readiness {
                readable: !tbl.slots[idx].unix_backlog.is_empty(),
                ..Default::default()
            }
        }

        SockKind::Unix => {
            // Created but never connected, or waiting in a listener's backlog:
            // nothing to read and nowhere to write, and not a hangup, which
            // would tell a poller to give up on a connect under way
            if state == SockState::Unconnected || state == SockState::Connecting {
                // Unless the listener went away underneath the request, which
                // is the connect failing: writable with a hangup, as a refused
                // TCP connect reports it
                let tbl = SOCKETS.lock();
                if state == SockState::Connecting && tbl.slots[idx].unix_hup {
                    return Readiness { writable: true, hup: true, ..Default::default() };
                }
                return Readiness::default();
            }
            let tbl = SOCKETS.lock();
            let slot = &tbl.slots[idx];
            let hup = slot.unix_hup || peer_of(&tbl, idx).is_none();
            Readiness {
                // A drained connection whose peer has gone still reads
                // without blocking: it reads end-of-file
                readable: !slot.unix_rx.is_empty() || hup,
                // Writable while the peer exists and has room. A full buffer
                // reported writable would make a poller spin
                writable: match peer_of(&tbl, idx) {
                    Some(p) => tbl.slots[p].unix_rx.len() < UNIX_BUF_MAX,
                    None => false,
                },
                hup,
                invalid: false,
            }
        }

        // Unreachable: `fd_to_idx` resolves an alias to its socket first
        SockKind::Alias => Readiness::invalid(),

        SockKind::Mayland => {
            let client = match may_client { Some(c) => c, None => return Readiness::invalid() };
            let queued = may_queued;
            let alive = crate::mayland::server::is_alive(client);
            Readiness {
                // Events the compositor has queued, plus whatever a short
                // read left behind in the slot. A dead connection reads as
                // end-of-file, which is progress and so counts as readable
                readable: queued
                    || crate::mayland::server::pending_bytes(client) > 0
                    || !alive,
                writable: alive,
                hup: !alive,
                ..Default::default()
            }
        }

        SockKind::Tcp => {
            if state == SockState::Connecting || state == SockState::Unconnected {
                let mut tbl = SOCKETS.lock();
                let slot = &mut tbl.slots[idx];
                if !slot.active { return Readiness::invalid(); }
                return match advance_connect(slot) {
                    // Established: writable, which is how a non-blocking
                    // connect says it finished
                    SockState::Connected => Readiness { writable: true, ..Default::default() },
                    // Failed, and not yet asked why: writable and a hangup,
                    // POSIX's POLLOUT|POLLERR. The caller wakes and `SO_ERROR`
                    // says what happened
                    SockState::Unconnected if slot.so_error.is_some() => Readiness {
                        writable: true,
                        hup: true,
                        ..Default::default()
                    },
                    // Nothing to read and nowhere to write, but not an error: a
                    // socket waiting for connect() is not ready yet
                    _ => Readiness::default(),
                };
            }
            // Borrowed in place rather than taken out of the slot, unlike the
            // blocking ops. Taking it would mark the slot Busy, and a poller in
            // an event loop revisits a socket every few milliseconds, so
            // another holder's blocking recv would fail with EBUSY.
            //
            // This holds the table lock across `recv_one`, which the rule at
            // the top of this file forbids for blocking calls. `recv_one` does
            // not block: it drains the demux into the socket's buffer and
            // returns, and `rx::pump` takes the network lock with try_lock
            let mut tbl = SOCKETS.lock();
            let slot = &mut tbl.slots[idx];
            if !slot.active { return Readiness::invalid(); }
            match slot.tcp.as_mut() {
                Some(sock) => {
                    sock.recv_one();
                    Readiness {
                        readable: sock.rx_available() > 0 || sock.peer_closed(),
                        writable: sock.is_connected(),
                        hup: sock.peer_closed(),
                        invalid: false,
                    }
                }
                // Out on loan to a blocking op, with its buffer not visible
                // from here: reports nothing, and the backstop brings it round
                None => Readiness::default(),
            }
        }
    }
}

/// Put a borrowed-out TcpSocket back into its slot after a blocking op.
/// If the slot was closed meanwhile, the socket is dropped here
fn return_socket(idx: usize, sock: Box<TcpSocket>) {
    let mut tbl = SOCKETS.lock();
    let slot = &mut tbl.slots[idx];
    if slot.active && slot.state == SockState::Busy {
        install_tcp(slot, sock);
        slot.state = SockState::Connected;
    }
    // Otherwise closed under us: dropping 'sock' runs its destructor
}

/// Send all of 'data' on the socket. Blocking. Returns bytes sent or errno.
/// For a connected UDP socket this is sendto(peer)
pub fn send(fd: u64, data: &[u8]) -> Result<usize, SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;

    // UDP and Mayland paths
    {
        let tbl = SOCKETS.lock();
        let slot = &tbl.slots[idx];
        if slot.active && slot.kind == SockKind::Mayland {
            drop(tbl);
            return may_send(idx, data);
        }
        if slot.active && slot.kind == SockKind::Unix {
            drop(tbl);
            return unix_send(idx, data);
        }
        if slot.active && slot.kind == SockKind::Udp {
            let (ip, port) = slot.udp_peer.ok_or(SockError::NotConnected)?;
            drop(tbl);
            return sendto(fd, ip, port, data);
        }
    }

    let mut sock = take_connected(idx)?;

    let ok = sock.send(data);
    let peer_gone = sock.peer_closed();
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

    // UDP and Mayland paths
    {
        let tbl = SOCKETS.lock();
        let slot = &tbl.slots[idx];
        if slot.active && slot.kind == SockKind::Mayland {
            drop(tbl);
            return may_recv(idx, buf);
        }
        if slot.active && slot.kind == SockKind::Unix {
            drop(tbl);
            return unix_recv(idx, buf);
        }
        if slot.active && slot.kind == SockKind::Udp {
            drop(tbl);
            return recvfrom(fd, buf).map(|(n, _, _)| n);
        }
    }

    let mut sock = take_connected(idx)?;

    let nb = nonblocking_at(idx);
    let start = crate::fs::procfs::uptime_ticks();
    let mut got = 0usize;
    loop {
        sock.recv_one_into(buf, &mut got);
        if got > 0 {
            break;
        }
        if sock.peer_closed() {
            break; // graceful EOF: return 0
        }
        // Out of time, or interrupted, with nothing read: EAGAIN, as every
        // other blocking socket path here answers. `Ok(0)` would be end of
        // file, and a server quiet for thirty seconds would look closed
        if crate::net::CTRL_C.load(core::sync::atomic::Ordering::SeqCst)
            || crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= OP_TIMEOUT_TICKS
        {
            return_socket(idx, sock);
            return Err(SockError::WouldBlock);
        }
        // The socket has to go back in its slot before this returns, so the
        // refusal cannot be a `?` here the way it is everywhere else
        if nb {
            return_socket(idx, sock);
            return Err(SockError::WouldBlock);
        }
        sock.wait();
    }

    return_socket(idx, sock);
    Ok(got)
}

// UDP sendto / recvfrom

/// Ensures the UDP socket has a local port, auto-binding an ephemeral one on
/// first send as BSD sockets do. Returns the bound port
fn udp_autobind(idx: usize) -> Result<u16, SockError> {
    let mut tbl = SOCKETS.lock();
    let slot = &mut tbl.slots[idx];
    if !slot.active { return Err(SockError::BadFd); }
    if slot.kind != SockKind::Udp { return Err(SockError::Invalid); }
    if let Some(_) = slot.udp_rx {
        return Ok(slot.bound_port);
    }
    for _ in 0..MAX_SOCKETS {
        let port = super::ephemeral_port();
        if let Some(h) = rx::register_udp(port) {
            slot.udp_rx = Some(h);
            slot.bound_port = port;
            return Ok(port);
        }
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

    let nb = is_nonblocking(fd);
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
        may_block(nb)?;
        // Until a datagram for this port is queued (`rx::handle_udp` wakes
        // its key), or the backstop
        let deadline = crate::kcore::time::now_ns().saturating_add(crate::net::irq::backstop_ns());
        crate::sched::wait::keyed(crate::sched::wait::key::net_udp(udp_h))
            .wait_unless(deadline, || rx::udp_pending(udp_h));
    }
}

// Local sockets (AF_UNIX)
//
// Two processes on one machine, talking through a byte queue, so a service can
// move to ring 3 and still be reachable.
//
// The queue lives in the reader's slot and the writer appends to it. The socket
// table owns every slot's lifetime, so there is nothing extra to keep alive or
// free when a process exits mid-write. What it costs is a check that the peer
// is still the socket it was: a slot closed and reused has a different
// generation, so a stale link is detected.

/// How much unread data one direction may hold before a writer has to wait
///
/// Not a fixed array in the slot, which would reserve megabytes for connections
/// that mostly do not exist. This grows with what is in flight
const UNIX_BUF_MAX: usize = 64 * 1024;

/// The peer's slot index, if the link is still good
///
/// A link is (index, generation). The index alone would dangle once the peer
/// closed and its slot was handed out, and the bytes would go to the new owner
fn peer_of(tbl: &SocketTable, idx: usize) -> Option<usize> {
    let (p, g) = tbl.slots[idx].peer?;
    let s = tbl.slots.get(p)?;
    if s.active && s.gen == g { Some(p) } else { None }
}

/// Tells the far end that this one has gone, before the slot is reused
fn detach_unix(tbl: &mut SocketTable, idx: usize) {
    if let Some(p) = peer_of(tbl, idx) {
        tbl.slots[p].unix_hup = true;
        tbl.slots[p].peer = None;
    }
    // A listener still holding unaccepted connections leaves those clients
    // waiting for an accept that is never coming
    let backlog = core::mem::take(&mut tbl.slots[idx].unix_backlog);
    for (ci, cg) in backlog {
        if let Some(c) = tbl.slots.get_mut(ci) {
            if c.active && c.gen == cg {
                c.unix_hup = true;
                c.peer = None;
                // A client that connected without blocking learns it was
                // refused through `SO_ERROR`, as a TCP one does
                if c.state == SockState::Connecting {
                    c.so_error = Some(SockError::ConnRefused);
                }
            }
        }
    }
    tbl.slots[idx].peer = None;
}

/// Allocate an unconnected local socket
pub fn create_unix(pid: u64) -> Option<u64> {
    create(pid, SockKind::Unix)
}

/// A connected pair, with no name and no rendezvous
///
/// What a parent and child use across a fork, and what a library wants for a
/// channel. Nothing is looked up, so nothing can be squatted on or raced for
pub fn socketpair(pid: u64) -> Option<(u64, u64)> {
    let a = create_unix(pid)?;
    let b = match create_unix(pid) {
        Some(b) => b,
        None => { close_fd(a); return None; }
    };
    let (ia, ib) = (fd_to_idx(a)?, fd_to_idx(b)?);
    let mut tbl = SOCKETS.lock();
    let (ga, gb) = (tbl.slots[ia].gen, tbl.slots[ib].gen);
    tbl.slots[ia].peer  = Some((ib, gb));
    tbl.slots[ib].peer  = Some((ia, ga));
    tbl.slots[ia].state = SockState::Connected;
    tbl.slots[ib].state = SockState::Connected;
    // Both ends are this process, and stay truthful after a fork hands one of
    // them to a child: the snapshot describes who made the pair
    let me = PeerCred::of(pid);
    tbl.slots[ia].my_cred   = Some(me);
    tbl.slots[ib].my_cred   = Some(me);
    tbl.slots[ia].peer_cred = Some(me);
    tbl.slots[ib].peer_cred = Some(me);
    Some((a, b))
}

/// Split "/run/netd.sock" into the directory to create in and the name
fn split_path(path: &str) -> Option<(&str, &str)> {
    match path.rfind('/') {
        Some(0) => Some(("/", &path[1..])),
        Some(i) => Some((&path[..i], &path[i + 1..])),
        // A bare name is relative to the root
        None => Some(("/", path)),
    }
    .filter(|(_, name)| !name.is_empty())
}

/// Gives a local socket a name in the filesystem
///
/// A node rather than a private registry, so `ls` shows it and it can be found,
/// diagnosed and cleaned up
pub fn unix_bind(fd: u64, path: &str) -> Result<(), SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    {
        let tbl = SOCKETS.lock();
        let slot = &tbl.slots[idx];
        if !slot.active { return Err(SockError::BadFd); }
        if slot.kind != SockKind::Unix { return Err(SockError::Invalid); }
        if slot.unix_path.is_some() { return Err(SockError::Invalid); }
        if slot.state != SockState::Unconnected { return Err(SockError::AlreadyConnected); }
    }

    let (dir, name) = split_path(path).ok_or(SockError::Invalid)?;
    // Built outside the socket lock: the VFS has its own, and taking it while
    // holding this one is the ordering the rest of the file avoids
    let vid = crate::fs::vfs::core::create_socket_node(dir, name)
        .map_err(|_| SockError::AddrInUse)?;

    let mut tbl = SOCKETS.lock();
    let slot = &mut tbl.slots[idx];
    if !slot.active {
        drop(tbl);
        crate::fs::vfs::core::unlink_path(dir, name);
        return Err(SockError::BadFd);
    }
    slot.unix_path = Some(String::from(path));
    slot.unix_node = Some(vid);
    Ok(())
}

/// Start taking connections on a bound local socket
pub fn unix_listen(fd: u64) -> Result<(), SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    let mut tbl = SOCKETS.lock();
    let slot = &mut tbl.slots[idx];
    if !slot.active { return Err(SockError::BadFd); }
    if slot.kind != SockKind::Unix { return Err(SockError::Invalid); }
    if slot.unix_node.is_none() { return Err(SockError::Invalid); } // listen before bind
    slot.kind = SockKind::UnixListener;
    Ok(())
}

/// Connect to a named local socket, waiting for the server to accept
pub fn unix_connect(fd: u64, path: &str) -> Result<(), SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    let nb = is_nonblocking(fd);
    let vid = crate::fs::vfs::core::socket_node_at(path).ok_or(SockError::ConnRefused)?;

    {
        let mut tbl = SOCKETS.lock();
        if !tbl.slots[idx].active { return Err(SockError::BadFd); }
        if tbl.slots[idx].kind != SockKind::Unix { return Err(SockError::Invalid); }
        match tbl.slots[idx].state {
            SockState::Unconnected => {}
            // Asked again while the first request is in the backlog: POSIX's
            // `EALREADY`, and no second request queued
            SockState::Connecting => return Err(SockError::Already),
            _ => return Err(SockError::AlreadyConnected),
        }
        // The node is the identity, not the path: two names can lead to one
        // socket, and a renamed listener is the same listener
        let listener = (0..MAX_SOCKETS).find(|&i| {
            let s = &tbl.slots[i];
            s.active && s.kind == SockKind::UnixListener && s.unix_node == Some(vid)
        }).ok_or(SockError::ConnRefused)?;

        // Written before the request is visible to the server, so the identity
        // the server gets is the one this process had when it asked, not what
        // it became afterwards
        tbl.slots[idx].my_cred = Some(PeerCred::of(crate::sched::current_tgid()));
        let (me, gen) = (idx, tbl.slots[idx].gen);
        tbl.slots[listener].unix_backlog.push((me, gen));
        // Half-open until the server accepts, which moves it to `Connected`
        tbl.slots[idx].state = SockState::Connecting;
    }
    // Woken outside the lock, per the rule in sched::wait
    crate::sched::wait::PIPE.wake_all();
    crate::sched::wait::wake_pollers();

    // Waits for an accept to link up. A server that never accepts is not there,
    // and the timeout says so
    let start = crate::fs::procfs::uptime_ticks();
    loop {
        {
            let tbl = SOCKETS.lock();
            let slot = &tbl.slots[idx];
            if !slot.active { return Err(SockError::BadFd); }
            if slot.state == SockState::Connected { return Ok(()); }
            if slot.unix_hup { return Err(SockError::ConnRefused); }
        }
        if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= OP_TIMEOUT_TICKS {
            // Take the request back out of the backlog so a server accepting
            // later does not hand a connection to a client that gave up
            let mut tbl = SOCKETS.lock();
            let gen = tbl.slots[idx].gen;
            for i in 0..MAX_SOCKETS {
                tbl.slots[i].unix_backlog.retain(|&(ci, cg)| !(ci == idx && cg == gen));
            }
            tbl.slots[idx].state = SockState::Unconnected;
            return Err(SockError::ConnRefused);
        }
        // A non-blocking connect answers `EINPROGRESS`, as TCP's does: the
        // request stays in the backlog, the socket is `Connecting`, the
        // server's `accept` makes it `Connected`, and `poll` reports it
        // writable then (`unix_accept` wakes the pollers). `mio`, and so tokio,
        // treats anything but `EINPROGRESS` as a failed connect
        if nb {
            return Err(SockError::InProgress);
        }
        // Looked at again once asleep-on-request, so an accept in between is
        // not lost to the backstop
        crate::sched::wait::PIPE.wait_unless(crate::sched::tick_deadline(LOCAL_BACKSTOP), || {
            let tbl = SOCKETS.lock();
            let slot = &tbl.slots[idx];
            !slot.active || slot.state == SockState::Connected || slot.unix_hup
        });
    }
}

/// Take one waiting connection, returning the new socket's fd
pub fn unix_accept(fd: u64, pid: u64) -> Result<u64, SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    {
        let tbl = SOCKETS.lock();
        if !tbl.slots[idx].active { return Err(SockError::BadFd); }
        if tbl.slots[idx].kind != SockKind::UnixListener { return Err(SockError::Invalid); }
    }

    let nb = is_nonblocking(fd);
    let start = crate::fs::procfs::uptime_ticks();
    loop {
        // A fresh slot per accepted connection, allocated before the client is
        // taken off the backlog: if the table is full the request stays where
        // it is and the client keeps waiting, rather than being dropped
        let pending = { !SOCKETS.lock().slots[idx].unix_backlog.is_empty() };
        if pending {
            let new_fd = create_unix(pid).ok_or(SockError::TableFull)?;
            let new_idx = fd_to_idx(new_fd).ok_or(SockError::BadFd)?;

            let mut tbl = SOCKETS.lock();
            // Walk past clients that gave up or died while queued
            let client = loop {
                match tbl.slots[idx].unix_backlog.first().copied() {
                    None => break None,
                    Some((ci, cg)) => {
                        tbl.slots[idx].unix_backlog.remove(0);
                        let alive = tbl.slots.get(ci)
                            .is_some_and(|c| c.active && c.gen == cg && !c.unix_hup);
                        if alive { break Some(ci); }
                    }
                }
            };
            match client {
                Some(ci) => {
                    let (gs, gc) = (tbl.slots[new_idx].gen, tbl.slots[ci].gen);
                    tbl.slots[new_idx].peer  = Some((ci, gc));
                    tbl.slots[ci].peer       = Some((new_idx, gs));
                    tbl.slots[new_idx].state = SockState::Connected;
                    tbl.slots[ci].state      = SockState::Connected;
                    // Each end learns who the other was. The client's snapshot
                    // was taken at connect; the server's is taken now
                    let server = PeerCred::of(pid);
                    let client = tbl.slots[ci].my_cred;
                    tbl.slots[new_idx].my_cred   = Some(server);
                    tbl.slots[new_idx].peer_cred = client;
                    tbl.slots[ci].peer_cred      = Some(server);
                    drop(tbl);
                    crate::sched::wait::PIPE.wake_all();
                    crate::sched::wait::wake_pollers();
                    return Ok(new_fd);
                }
                None => {
                    // Nothing usable was queued after all
                    drop(tbl);
                    close_fd(new_fd);
                }
            }
        }

        if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= OP_TIMEOUT_TICKS {
            return Err(SockError::WouldBlock);
        }
        may_block(nb)?;
        // Looked at again once asleep-on-request, so a connect in between is
        // not lost to the backstop
        crate::sched::wait::PIPE.wait_unless(crate::sched::tick_deadline(LOCAL_BACKSTOP), || {
            let tbl = SOCKETS.lock();
            !tbl.slots[idx].active || !tbl.slots[idx].unix_backlog.is_empty()
        });
    }
}

/// Appends to the peer's queue. Returns what was taken, which may be less than
/// offered: a stream socket may short-write and the caller loops
fn unix_send(idx: usize, data: &[u8]) -> Result<usize, SockError> {
    use crate::sched::wait::{key, keyed, wake_key, wait_any_unless, PIPE};
    if data.is_empty() { return Ok(0); }
    let nb = nonblocking_at(idx);
    let start = crate::fs::procfs::uptime_ticks();
    loop {
        let p = {
            let mut tbl = SOCKETS.lock();
            if !tbl.slots[idx].active { return Err(SockError::BadFd); }
            let p = match peer_of(&tbl, idx) {
                Some(p) => p,
                // Writing to a connection whose far end has gone is EPIPE,
                // the same as a pipe with no reader
                None => return Err(SockError::BrokenPipe),
            };
            let room = UNIX_BUF_MAX.saturating_sub(tbl.slots[p].unix_rx.len());
            if room > 0 {
                let n = room.min(data.len());
                tbl.slots[p].unix_rx.extend_from_slice(&data[..n]);
                drop(tbl);
                // The reader of that buffer, and nobody else on the machine
                wake_key(key::socket(p));
                return Ok(n);
            }
            p
        };
        // Full: this is the flow control. A reader that never reads slows its
        // writer down instead of growing a queue until memory runs out
        if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= OP_TIMEOUT_TICKS {
            return Err(SockError::WouldBlock);
        }
        may_block(nb)?;
        // On the buffer it wants room in, and on the queue a hangup wakes.
        // Looked at again once on both and asleep-on-request, so a read or
        // a hangup in between is not lost to the backstop
        wait_any_unless(&[keyed(key::socket(p)), &PIPE], crate::sched::tick_deadline(LOCAL_BACKSTOP), || {
            let tbl = SOCKETS.lock();
            !tbl.slots[idx].active || peer_of(&tbl, idx) != Some(p) || tbl.slots[p].unix_rx.len() < UNIX_BUF_MAX
        });
    }
}

/// Take what has been written to this end, blocking until there is some
fn unix_recv(idx: usize, buf: &mut [u8]) -> Result<usize, SockError> {
    use crate::sched::wait::{join, key, keyed, wake_key, PIPE};
    if buf.is_empty() { return Ok(0); }
    let nb = nonblocking_at(idx);
    let start = crate::fs::procfs::uptime_ticks();
    let queues = [keyed(key::socket(idx)), &PIPE];
    let mut joined = None;
    loop {
        {
            let mut tbl = SOCKETS.lock();
            if !tbl.slots[idx].active { return Err(SockError::BadFd); }
            let have = tbl.slots[idx].unix_rx.len();
            if have > 0 {
                let n = have.min(buf.len());
                buf[..n].copy_from_slice(&tbl.slots[idx].unix_rx[..n]);
                tbl.slots[idx].unix_rx.drain(..n);
                let freed = tbl.slots[idx].unix_rx.len() < UNIX_BUF_MAX;
                let peer = peer_of(&tbl, idx);
                drop(tbl);
                // A writer parked on a full queue now has room: it waits on
                // this buffer's key, and a poller on its own descriptor
                // asking whether it may write, on the peer's
                if freed {
                    wake_key(key::socket(idx));
                    if let Some(p) = peer {
                        wake_key(key::socket(p));
                    }
                }
                return Ok(n);
            }
            // Drained and the far end is gone: end of file, not a timeout.
            // Checked after the read and not before it, so the last thing a
            // peer wrote before closing is still delivered
            if tbl.slots[idx].unix_hup || peer_of(&tbl, idx).is_none() {
                return Ok(0);
            }
        }
        if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= OP_TIMEOUT_TICKS {
            return Err(SockError::WouldBlock);
        }
        may_block(nb)?;
        // Joined, looked at once more, then asleep (`wait::join`)
        match joined.take() {
            None => joined = Some(join(&queues)),
            Some(j) => { j.sleep(crate::sched::tick_deadline(LOCAL_BACKSTOP)); }
        }
    }
}


// Peer credentials and descriptor passing
//
// So a service can leave ring 0 and still be trustworthy. A service in ring 3
// has to answer who is asking and what terminal they are on, and neither can be
// answered by what the client says about itself.

/// What the kernel knows about the far end of a local socket.
///
/// `None` when the socket is not a connected local one, or when the caller does
/// not own it. Never a guess: saying "root" about a peer that does not exist
/// would turn a permission check inside out
pub fn peer_cred(fd: u64, pid: u64) -> Option<PeerCred> {
    let idx = fd_to_idx(fd)?;
    let tbl = SOCKETS.lock();
    let s = tbl.slots.get(idx)?;
    if !s.active || s.owner_pid != pid { return None; }
    if s.kind != SockKind::Unix { return None; }
    s.peer_cred
}

/// Puts a descriptor in flight to the peer.
///
/// The vnode reference is taken by the caller before this and is owned by the
/// queue from here on, so the sender may close its own copy at once
pub fn unix_push_fd(fd: u64, pid: u64, of: crate::fs::vfs::fd::OpenFile) -> Result<(), SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    let mut tbl = SOCKETS.lock();
    if !tbl.slots[idx].active || tbl.slots[idx].owner_pid != pid {
        return Err(SockError::BadFd);
    }
    if tbl.slots[idx].kind != SockKind::Unix { return Err(SockError::Invalid); }
    let peer = peer_of(&tbl, idx).ok_or(SockError::NotConnected)?;
    if tbl.slots[peer].pending_fds.len() >= MAX_INFLIGHT_FDS {
        return Err(SockError::WouldBlock);
    }
    tbl.slots[peer].pending_fds.push(of);
    Ok(())
}

/// Takes the oldest descriptor the peer sent, if any.
///
/// Non-blocking: a caller wanting to wait polls the socket for readability
/// first, since a descriptor and the bytes describing it travel together
pub fn unix_take_fd(fd: u64, pid: u64) -> Result<crate::fs::vfs::fd::OpenFile, SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    let mut tbl = SOCKETS.lock();
    if !tbl.slots[idx].active || tbl.slots[idx].owner_pid != pid {
        return Err(SockError::BadFd);
    }
    if tbl.slots[idx].kind != SockKind::Unix { return Err(SockError::Invalid); }
    if tbl.slots[idx].pending_fds.is_empty() {
        return Err(SockError::WouldBlock);
    }
    Ok(tbl.slots[idx].pending_fds.remove(0))
}

/// Lets go of descriptors in flight when their socket died.
///
/// Outside the socket table lock: releasing a vnode takes the VFS lock, and the
/// two are held in the other order everywhere else
fn drop_inflight(orphans: alloc::vec::Vec<crate::fs::vfs::fd::OpenFile>) {
    if orphans.is_empty() { return; }
    crate::fs::vfs::core::with_vfs(|vfs| {
        for of in &orphans {
            let vid = of.vnode_id as usize;
            vfs.drop_ref(vid);
        }
    });
}

/// `getsockopt`(114): the few options a socket can answer about itself.
///
/// `SO_ERROR` reads and clears the pending error, the only way a non-blocking
/// connect's failure reaches the program. `SO_TYPE` and `IP_TTL` are what the
/// socket knows. Anything else is `Err(Invalid)`, which the syscall layer
/// spells `ENOPROTOOPT`
pub fn getsockopt(fd: u64, pid: u64, level: u32, name: u32) -> Result<u64, SockError> {
    let idx = fd_to_idx(fd).ok_or(SockError::BadFd)?;
    let mut tbl = SOCKETS.lock();
    let slot = &mut tbl.slots[idx];
    if !slot.active || slot.owner_pid != pid { return Err(SockError::BadFd); }
    const SO_TYPE: u32 = miku_abi::socket::SO_TYPE as u32;
    const SO_ERROR: u32 = miku_abi::socket::SO_ERROR as u32;
    const SO_ACCEPTCONN: u32 = miku_abi::socket::SO_ACCEPTCONN as u32;
    match (level, name) {
        (SOL_SOCKET, SO_ERROR) => {
            // Asked while still connecting: take a step first, so a caller
            // polling `SO_ERROR` itself also sees the handshake finish
            advance_connect(slot);
            Ok(slot.so_error.take().map_or(0, |e| (-sock_errno(e)) as u64))
        }
        (SOL_SOCKET, SO_TYPE) => Ok(match slot.kind {
            SockKind::Udp => SOCK_DGRAM as u64,
            SockKind::Icmp => SOCK_RAW as u64,
            _ => SOCK_STREAM as u64,
        }),
        (SOL_SOCKET, SO_ACCEPTCONN) => {
            Ok(matches!(slot.kind, SockKind::Listener | SockKind::UnixListener) as u64)
        }
        (SOL_IP, IP_TTL) => Ok(slot.ttl as u64),
        (SOL_SOCKET, SO_KEEPALIVE) if slot.kind == SockKind::Tcp => Ok(slot.keepalive.enabled as u64),
        (IPPROTO_TCP, TCP_KEEPIDLE) if slot.kind == SockKind::Tcp => Ok(slot.keepalive.idle_ms / 1000),
        (IPPROTO_TCP, TCP_KEEPINTVL) if slot.kind == SockKind::Tcp => Ok(slot.keepalive.interval_ms / 1000),
        (IPPROTO_TCP, TCP_KEEPCNT) if slot.kind == SockKind::Tcp => Ok(slot.keepalive.count as u64),
        _ => Err(SockError::Invalid),
    }
}

/// The negative errno for a socket error: the one table, used by the syscall
/// layer's `sock_err` and by `SO_ERROR`
pub fn sock_errno(which: SockError) -> i64 {
    use crate::syscall::errno::*;
    match which {
        SockError::BadFd            => EBADF,
        SockError::NotConnected     => ENOTCONN,
        SockError::AlreadyConnected => EISCONN,
        SockError::Busy             => EBUSY,
        SockError::ConnRefused      => ECONNREFUSED,
        SockError::BrokenPipe       => EPIPE,
        SockError::ConnReset        => ECONNRESET,
        SockError::AddrInUse        => EADDRINUSE,
        SockError::WouldBlock       => EAGAIN,
        SockError::Invalid          => EINVAL,
        SockError::NotSupported     => EOPNOTSUPP,
        SockError::TableFull        => EMFILE,
        SockError::InProgress       => EINPROGRESS,
        SockError::Already          => EALREADY,
        SockError::TimedOut         => ETIMEDOUT,
    }
}
