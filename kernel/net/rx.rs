// Central RX demultiplexer
//
// Exactly one place drains the NIC: pump(). Every received frame is parsed
// once and routed to its consumer through a bounded per-consumer queue:
//
//   ARP                      -> handled here (table update + reply)
//   ICMP echo request        -> replied here
//   ICMP everything else     -> every registered ICMP waiter (ping, traceroute)
//   UDP                      -> bound port queue (DHCP, DNS, NTP, user sockets)
//   TCP matching a 4-tuple   -> that connection's segment queue
//   TCP SYN to a listen port -> that listener's backlog queue
//
// Any thread may call pump() at any time; a reentrancy guard makes
// concurrent calls cheap no-ops, and consumers only ever touch their own
// queue, so two sockets (or ping running alongside a download) can't steal
// each other's packets.
//
// Locking: pump() holds NET only while draining the driver or transmitting a
// reply, never while a registry mutex is held, so consumers that send with
// NET locked cannot deadlock against dispatch.

extern crate alloc;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use super::eth::{self, EthFrame, ETHERTYPE_ARP, ETHERTYPE_IP};
use super::tcp::{TcpSegment, FLAG_ACK, FLAG_FIN, FLAG_RST, FLAG_SYN};
use super::{arp, ipv4, udp, RxMeta};

/// How many of each the demux holds at once. The tables are vectors that grow
/// to these, so an idle machine pays for what it uses
const MAX_TCP_CONNS: usize = 1024;
const MAX_LISTENERS: usize = 64;
const MAX_UDP_BINDS: usize = 256;
const MAX_ICMP_WAITERS: usize = 8;

/// A listener's backlog and an ICMP waiter's queue, in packets; on overflow
/// the newest is dropped (the sender's retransmit machinery is the recovery
/// path, same as a real NIC ring overrun)
const QUEUE_CAP: usize = 64;
/// A UDP port's queue, in datagrams: a DNS burst or a game's tick
const UDP_QUEUE_CAP: usize = 256;
/// A TCP connection's queue holds a receive window: its payload bytes counted
/// against what the window lets a peer send, plus a margin for the bare ACKs
/// and retransmissions that come with it
const TCP_QUEUE_BYTES: usize = mikunet::stack::conn::RX_CAP + 64 * 1024;
const TCP_QUEUE_SEGS: usize = 1024;
/// Frames taken from the driver per pump() call. Whatever does not fit
/// stays in the ring and comes up on the next call
const RX_BATCH: usize = 64;

/// One TCP segment as delivered to a connection queue
pub struct TcpPacket {
    pub seg: TcpSegment,
    pub payload: Vec<u8>,
    pub src_ip: [u8; 4],
    pub src_mac: [u8; 6],
    /// The segment's options, whole: a SYN-ACK's window shift and SACK
    /// permission, an ACK's SACK blocks
    pub opts: Vec<u8>,
}

/// A SYN parked on a listener's backlog, everything accept() needs to
/// finish the handshake
#[derive(Clone, Copy)]
pub struct SynRequest {
    pub src_ip: [u8; 4],
    pub src_mac: [u8; 6],
    pub src_port: u16,
    pub seq: u32,
    /// What the SYN's options said: MSS, window shift, SACK permitted
    pub opts: mikunet::stack::tcp::SynOptions,
    /// The window the client advertised in its SYN. Carried so the first burst
    /// back is sized on what the peer said it could hold
    pub window: u16,
}

pub struct UdpDgram {
    pub src_ip: [u8; 4],
    pub src_port: u16,
    pub data: Vec<u8>,
}

/// Non-echo-request ICMP event fanned out to all ICMP waiters. For echo replies
/// id/seq come from the ICMP header; for time-exceeded and unreachable they are
/// 0 (consumers match on icmp_type + src_ip).
///
/// `packet` is the whole IP packet: a raw ICMP socket in ring 3 hands its
/// program the bytes, and traceroute reads the embedded header inside a
/// time-exceeded message. The fields below are what in-kernel consumers need
#[derive(Clone)]
pub struct IcmpEvent {
    pub icmp_type: u8,
    pub src_ip: [u8; 4],
    pub id: u16,
    pub seq: u16,
    pub ttl: u8,
    pub packet: Vec<u8>,
}

struct TcpConnSlot {
    local_port: u16,
    remote_ip: [u8; 4],
    remote_port: u16,
    queue: VecDeque<TcpPacket>,
    /// Payload bytes in `queue`
    queued: usize,
}

struct ListenSlot {
    port: u16,
    queue: VecDeque<SynRequest>,
}

struct UdpSlot {
    port: u16,
    queue: VecDeque<UdpDgram>,
}

struct IcmpSlot {
    queue: VecDeque<IcmpEvent>,
}

// Slot tables behind their own mutexes; None = free. Registration returns the
// slot index, which acts as the queue handle and stays one while the slot is
// held, as a vector only grows here
static TCP_CONNS: Mutex<Vec<Option<TcpConnSlot>>> = Mutex::new(Vec::new());
static LISTENERS: Mutex<Vec<Option<ListenSlot>>> = Mutex::new(Vec::new());
static UDP_BINDS: Mutex<Vec<Option<UdpSlot>>> = Mutex::new(Vec::new());
static ICMP_WAITERS: Mutex<Vec<Option<IcmpSlot>>> = Mutex::new(Vec::new());

/// A free slot in `tbl`, or a new one while it is below `max`
fn take_slot<T>(tbl: &mut Vec<Option<T>>, max: usize, v: T) -> Option<usize> {
    if let Some(i) = tbl.iter().position(|s| s.is_none()) {
        tbl[i] = Some(v);
        return Some(i);
    }
    if tbl.len() < max {
        tbl.push(Some(v));
        return Some(tbl.len() - 1);
    }
    None
}

static PUMPING: AtomicBool = AtomicBool::new(false);
/// A pump found another running: that one goes round once more before it
/// stops, for a frame that may have come after it read the ring
static AGAIN: AtomicBool = AtomicBool::new(false);

// Registration

pub fn register_tcp(local_port: u16, remote_ip: [u8; 4], remote_port: u16) -> Option<usize> {
    let slot = TcpConnSlot { local_port, remote_ip, remote_port, queue: VecDeque::new(), queued: 0 };
    take_slot(&mut TCP_CONNS.lock(), MAX_TCP_CONNS, slot)
}

pub fn unregister_tcp(idx: usize) {
    if let Some(s) = TCP_CONNS.lock().get_mut(idx) {
        *s = None;
    }
}

/// Whether a connection's queue has a segment, without taking it: what a waiter
/// looks at once it has asked to sleep
pub fn tcp_pending(idx: usize) -> bool {
    TCP_CONNS.lock().get(idx).and_then(|s| s.as_ref()).is_some_and(|s| !s.queue.is_empty())
}

pub fn tcp_take(idx: usize) -> Option<TcpPacket> {
    let mut tbl = TCP_CONNS.lock();
    let slot = tbl.get_mut(idx)?.as_mut()?;
    let pkt = slot.queue.pop_front()?;
    slot.queued -= pkt.payload.len();
    Some(pkt)
}

/// True if some other connection already uses this (local, remote) pair
pub fn tcp_tuple_in_use(local_port: u16, remote_ip: [u8; 4], remote_port: u16) -> bool {
    TCP_CONNS.lock().iter().flatten().any(|s| {
        s.local_port == local_port && s.remote_ip == remote_ip && s.remote_port == remote_port
    })
}

pub fn register_listener(port: u16) -> Option<usize> {
    let mut tbl = LISTENERS.lock();
    if tbl.iter().flatten().any(|s| s.port == port) {
        return None; // port already has a listener
    }
    take_slot(&mut tbl, MAX_LISTENERS, ListenSlot { port, queue: VecDeque::new() })
}

pub fn unregister_listener(idx: usize) {
    if let Some(s) = LISTENERS.lock().get_mut(idx) {
        *s = None;
    }
}

pub fn listener_take(idx: usize) -> Option<SynRequest> {
    LISTENERS.lock().get_mut(idx)?.as_mut()?.queue.pop_front()
}

/// Is a connection waiting, without taking it?
///
/// For `poll`: `listener_take` would consume the request, and the `accept`
/// after it would find nothing
pub fn listener_pending(idx: usize) -> bool {
    LISTENERS.lock().get(idx)
        .and_then(|s| s.as_ref())
        .is_some_and(|s| !s.queue.is_empty())
}

pub fn register_udp(port: u16) -> Option<usize> {
    let mut tbl = UDP_BINDS.lock();
    if tbl.iter().flatten().any(|s| s.port == port) {
        return None; // EADDRINUSE
    }
    take_slot(&mut tbl, MAX_UDP_BINDS, UdpSlot { port, queue: VecDeque::new() })
}

pub fn unregister_udp(idx: usize) {
    if let Some(s) = UDP_BINDS.lock().get_mut(idx) {
        *s = None;
    }
}

pub fn udp_take(idx: usize) -> Option<UdpDgram> {
    UDP_BINDS.lock().get_mut(idx)?.as_mut()?.queue.pop_front()
}

/// Is a datagram waiting, without taking it? See `listener_pending`
pub fn udp_pending(idx: usize) -> bool {
    UDP_BINDS.lock().get(idx)
        .and_then(|s| s.as_ref())
        .is_some_and(|s| !s.queue.is_empty())
}

pub fn udp_port_bound(port: u16) -> bool {
    UDP_BINDS.lock().iter().flatten().any(|s| s.port == port)
}

pub fn register_icmp() -> Option<usize> {
    take_slot(&mut ICMP_WAITERS.lock(), MAX_ICMP_WAITERS, IcmpSlot { queue: VecDeque::new() })
}

pub fn unregister_icmp(idx: usize) {
    if let Some(s) = ICMP_WAITERS.lock().get_mut(idx) {
        *s = None;
    }
}

pub fn icmp_take(idx: usize) -> Option<IcmpEvent> {
    ICMP_WAITERS.lock().get_mut(idx)?.as_mut()?.queue.pop_front()
}

// The pump

/// What a batch left behind in the ring
enum Left {
    Nothing,
    /// The batch was full, or more came while it was read
    More,
    /// `NET` was held: maybe by this pump's own caller
    Busy,
}

/// Drains the NIC and routes every frame to its consumer queue. Safe to call
/// from any thread at any time; a call finding another running leaves the ring
/// to it (`AGAIN`).
///
/// netd clears its pending signal before it pumps, so returning quietly when it
/// could not drain (another pump running, `NET` held, the batch full) would
/// leave it asleep on frames in the ring until its 20 ms backstop
/// (`dev/net-architecture.md`, 3a)
pub fn pump() {
    if !super::is_ready() {
        return;
    }
    loop {
        if PUMPING.swap(true, Ordering::SeqCst) {
            AGAIN.store(true, Ordering::SeqCst);
            // The other may have stopped between the two: either it sees
            // `AGAIN`, or this sees it gone and pumps
            if PUMPING.load(Ordering::SeqCst) {
                return;
            }
            continue;
        }
        let left = pump_batch();
        PUMPING.store(false, Ordering::SeqCst);
        match left {
            Left::More => continue,
            // Not spun on: the holder may be this pump's caller. netd goes
            // round again instead of sleeping
            Left::Busy => {
                super::irq::rearm();
                return;
            }
            Left::Nothing => {
                if !AGAIN.swap(false, Ordering::SeqCst) {
                    return;
                }
            }
        }
    }
}

/// One batch from the ring, dispatched. The caller holds `PUMPING`
fn pump_batch() -> Left {
    let mut frames: Vec<(Vec<u8>, RxMeta)> = Vec::new();
    let more = {
        let Some(mut state) = super::NET.try_lock() else {
            return Left::Busy;
        };
        let mut more = false;
        if let Some(drv) = state.driver.as_mut() {
            // Take the frame first, then say whether to keep going. Returning
            // false before taking it would leave the driver holding a frame it
            // has dequeued
            drv.recv(&mut |buf, meta| {
                frames.push((Vec::from(buf), meta));
                frames.len() < RX_BATCH
            });
            more = drv.has_packet();
        }
        state.rx_count += frames.len() as u64;
        more
    };

    // When the card belongs to a program, the batch goes to it whole and the
    // rest of this function never runs. The drain above still happens once and
    // in this one place
    if super::raw::enabled() {
        for (frame, _) in &frames {
            super::raw::push(frame);
        }
        if !frames.is_empty() {
            crate::sched::wait::wake_pollers();
        }
        return if more { Left::More } else { Left::Nothing };
    }

    // One snapshot of who we are for the whole batch, to avoid taking the NET
    // mutex per frame
    let me = {
        let s = super::NET.lock();
        Local { mac: s.mac, ip: s.ip, mask: s.mask }
    };

    let mut dropped_addr = 0u64;
    for (raw, meta) in &frames {
        if !dispatch_frame(raw, *meta, &me) {
            dropped_addr += 1;
        }
    }
    if dropped_addr > 0 {
        super::NET.lock().drop_addr += dropped_addr;
    }

    // Something arrived and is in a consumer's queue. Which consumer does not
    // matter: a poller re-scans its own set.
    //
    // Only on a non-empty batch: netd pumps 250 times a second on an idle
    // machine
    if !frames.is_empty() {
        crate::sched::wait::wake_pollers();
    }
    if more { Left::More } else { Left::Nothing }
}

/// Transmit a raw ethernet frame through the driver
fn tx(frame: &[u8]) {
    super::tx_frame(frame);
}

/// This machine's addresses, read once per batch
struct Local {
    mac: [u8; 6],
    ip: [u8; 4],
    mask: [u8; 4],
}

/// Returns false when the frame was not addressed to us
fn dispatch_frame(raw: &[u8], meta: RxMeta, me: &Local) -> bool {
    let frame = match EthFrame::parse(raw) {
        Some(f) => f,
        None => return true,
    };

    // Ours, broadcast, or multicast. A NIC in promiscuous mode, a hub or a
    // mirrored switch port hands up traffic between two other machines, which
    // must not reach the demux
    let to_us = frame.dst == me.mac
        || frame.dst == eth::BROADCAST_MAC
        || frame.dst[0] & 0x01 != 0;
    if !to_us {
        return false;
    }

    match frame.ethertype {
        ETHERTYPE_ARP => handle_arp(&frame),
        ETHERTYPE_IP => handle_ip(&frame, meta, me),
        _ => {}
    }
    true
}

fn handle_arp(frame: &EthFrame) {
    let mut reply = [0u8; 64];
    let rlen = {
        let mut state = super::NET.lock();
        let mac = state.mac;
        let ip = state.ip;
        arp::handle(frame, &mac, &ip, &mut state.arp, super::now_ms(), &mut reply)
    };
    if rlen > 0 {
        tx(&reply[..rlen]);
    }
}

fn handle_ip(frame: &EthFrame, meta: RxMeta, me: &Local) {
    let ip_hdr = match ipv4::Ipv4Header::parse(frame.payload) {
        Some(h) => h,
        None => return,
    };

    let hdr_bytes = &frame.payload[..(ip_hdr.ihl as usize).min(frame.payload.len())];
    if !ipv4::verify_header(hdr_bytes) {
        super::NET.lock().drop_csum += 1;
        return;
    }

    // No reassembly yet, so a fragment cannot be handled, and must not be
    // handed on: after an IP header of a non-first fragment is the middle of
    // someone's payload, which handle_udp would read a UDP header from
    if ip_hdr.fragmented {
        super::NET.lock().drop_frag += 1;
        return;
    }

    if !addressed_to_us(&ip_hdr.dst, me) {
        super::NET.lock().drop_addr += 1;
        return;
    }

    let payload = ip_hdr.payload(frame.payload);

    match ip_hdr.proto {
        ipv4::PROTO_ICMP => handle_icmp(frame, &ip_hdr, payload, frame.payload),
        ipv4::PROTO_UDP => handle_udp(&ip_hdr, payload, meta),
        ipv4::PROTO_TCP => handle_tcp(frame, &ip_hdr, payload, meta),
        _ => {}
    }
}

/// Our address, a broadcast, or anything at all while we have no address yet
/// (DHCP runs before there is one, and its replies are addressed to the lease
/// we have not accepted yet)
fn addressed_to_us(dst: &[u8; 4], me: &Local) -> bool {
    let (ip, mask) = (me.ip, me.mask);
    if ip == [0, 0, 0, 0] {
        return true;
    }
    if dst == &ip || dst == &[255, 255, 255, 255] {
        return true;
    }
    // Directed broadcast for our subnet: host bits all ones
    let mut bcast = [0u8; 4];
    for i in 0..4 {
        bcast[i] = ip[i] | !mask[i];
    }
    dst == &bcast || dst[0] >= 224 && dst[0] <= 239
}

fn handle_icmp(frame: &EthFrame, ip_hdr: &ipv4::Ipv4Header, icmp: &[u8], full: &[u8]) {
    if icmp.len() < 8 {
        return;
    }

    if icmp[0] == 8 {
        // Echo request: answer it right here
        let (our_ip, our_mac) = {
            let s = super::NET.lock();
            (s.ip, s.mac)
        };
        // Only what was addressed to this machine: a directed broadcast ping
        // reaches every host on the segment, and answering from all of them
        // amplifies one forged packet into a flood
        if ip_hdr.dst == our_ip {
            if let Some(reply) = crate::net::icmp::echo_reply(icmp) {
                let mut ip_reply = [0u8; 1500];
                let ip_len = ipv4::build(&our_ip, &ip_hdr.src, ipv4::PROTO_ICMP, &reply, &mut ip_reply);
                if ip_len > 0 {
                    let mut eth_buf = [0u8; 1520];
                    let n = EthFrame::build(&frame.src, &our_mac, ETHERTYPE_IP,
                                            &ip_reply[..ip_len], &mut eth_buf);
                    if n > 0 {
                        tx(&eth_buf[..n]);
                    }
                }
            }
        }
        return;
    }

    // Echo reply carries its own id/seq; time-exceeded (11) and
    // unreachable (3) get zeros, consumers match on type + source
    let (id, seq) = if icmp[0] == 0 {
        (
            u16::from_be_bytes([icmp[4], icmp[5]]),
            u16::from_be_bytes([icmp[6], icmp[7]]),
        )
    } else {
        (0, 0)
    };

    let ev = IcmpEvent {
        icmp_type: icmp[0],
        src_ip: ip_hdr.src,
        id,
        seq,
        ttl: ip_hdr.ttl,
        packet: full.to_vec(),
    };

    let mut woke = alloc::vec::Vec::new();
    let mut tbl = ICMP_WAITERS.lock();
    for (i, slot) in tbl.iter_mut().enumerate() {
        if let Some(slot) = slot {
            if slot.queue.len() < QUEUE_CAP {
                slot.queue.push_back(ev.clone());
                woke.push(i);
            }
        }
    }
    drop(tbl);
    for i in woke {
        crate::sched::wait::wake_key(crate::sched::wait::key::net_icmp(i));
    }
    // A program blocked in poll() on an ICMP socket has nothing else that
    // would wake it: unlike TCP there is no data path that pumps on its own
    crate::sched::wait::wake_pollers();
}

/// Whether an ICMP waiter has anything queued, without taking it
pub fn icmp_pending(idx: usize) -> bool {
    ICMP_WAITERS
        .lock()
        .get(idx)
        .and_then(|s| s.as_ref())
        .is_some_and(|s| !s.queue.is_empty())
}

fn handle_udp(ip_hdr: &ipv4::Ipv4Header, payload: &[u8], meta: RxMeta) {
    let hdr = match udp::UdpHeader::parse(payload) {
        Some(h) => h,
        None => return,
    };

    // A zero checksum field means the sender chose not to compute one, which
    // UDP permits; anything else has to check out
    let stored = u16::from_be_bytes([payload[6], payload[7]]);
    if !meta.skip_l4_csum
        && stored != 0
        && !ipv4::verify_l4(&ip_hdr.src, &ip_hdr.dst, ipv4::PROTO_UDP, payload)
    {
        super::NET.lock().drop_csum += 1;
        return;
    }

    let data = hdr.payload(payload);

    let woke = {
        let mut tbl = UDP_BINDS.lock();
        let found = tbl.iter_mut().enumerate().find_map(|(i, s)| {
            s.as_mut().filter(|s| s.port == hdr.dst_port).map(|s| (i, s))
        });
        match found {
            Some((i, slot)) => {
                if slot.queue.len() < UDP_QUEUE_CAP {
                    slot.queue.push_back(UdpDgram {
                        src_ip: ip_hdr.src,
                        src_port: hdr.src_port,
                        data: Vec::from(data),
                    });
                }
                Some(i)
            }
            // No binding: drop silently (no ICMP port-unreachable yet)
            None => None,
        }
    };
    // Its reader, after the table's lock is let go
    if let Some(i) = woke {
        crate::sched::wait::wake_key(crate::sched::wait::key::net_udp(i));
    }
}

fn handle_tcp(frame: &EthFrame, ip_hdr: &ipv4::Ipv4Header, payload: &[u8], meta: RxMeta) {
    let seg = match TcpSegment::parse(payload) {
        Some(s) => s,
        None => return,
    };

    if !meta.skip_l4_csum
        && !ipv4::verify_l4(&ip_hdr.src, &ip_hdr.dst, ipv4::PROTO_TCP, payload)
    {
        super::NET.lock().drop_csum += 1;
        return;
    }

    // Established / in-handshake connection?
    let conn = {
        let mut tbl = TCP_CONNS.lock();
        let found = tbl.iter_mut().enumerate().find_map(|(i, s)| {
            s.as_mut()
                .filter(|s| {
                    s.local_port == seg.dst_port
                        && s.remote_port == seg.src_port
                        && s.remote_ip == ip_hdr.src
                })
                .map(|s| (i, s))
        });
        found.map(|(i, slot)| {
            let data = seg.payload(payload);
            if slot.queue.len() < TCP_QUEUE_SEGS && slot.queued + data.len() <= TCP_QUEUE_BYTES {
                slot.queued += data.len();
                slot.queue.push_back(TcpPacket {
                    seg,
                    payload: Vec::from(data),
                    src_ip: ip_hdr.src,
                    src_mac: frame.src,
                    opts: Vec::from(seg.options(payload)),
                });
            }
            i
        })
    };
    if let Some(i) = conn {
        // Its reader, after the table's lock is let go
        crate::sched::wait::wake_key(crate::sched::wait::key::net_tcp(i));
        return;
    }

    // Fresh SYN aimed at a listening port?
    if seg.flags & FLAG_SYN != 0 && seg.flags & FLAG_ACK == 0 {
        let opts = mikunet::stack::tcp::parse_syn_options(seg.options(payload));
        let listener = {
            let mut tbl = LISTENERS.lock();
            let found = tbl.iter_mut().enumerate().find_map(|(i, s)| {
                s.as_mut().filter(|s| s.port == seg.dst_port).map(|s| (i, s))
            });
            found.map(|(i, slot)| {
                if slot.queue.len() < QUEUE_CAP {
                    slot.queue.push_back(SynRequest {
                        src_ip: ip_hdr.src,
                        src_mac: frame.src,
                        src_port: seg.src_port,
                        seq: seg.seq,
                        opts,
                        window: seg.window,
                    });
                }
                i
            })
        };
        if let Some(i) = listener {
            crate::sched::wait::wake_key(crate::sched::wait::key::net_listen(i));
            return;
        }
    }

    // Nothing here owns this 4-tuple. Tell the sender so: after a socket closes
    // mid-transfer the peer would otherwise resend the same segment with
    // exponential backoff for minutes. Every segment reaching here has had its
    // checksum verified
    if seg.flags & FLAG_RST == 0 {
        send_rst(frame, ip_hdr, &seg, payload.len());
    }
}

/// RFC 793 §3.4: the reset for a segment that arrives on no connection.
/// An acceptable ACK gives the reset its sequence number; otherwise the reset
/// acknowledges everything the segment carried
fn send_rst(frame: &EthFrame, ip_hdr: &ipv4::Ipv4Header, seg: &TcpSegment, tcp_len: usize) {
    let data_len = tcp_len.saturating_sub(seg.data_offset as usize);
    // SYN and FIN each occupy one sequence number
    let mut seg_len = data_len as u32;
    if seg.flags & FLAG_SYN != 0 { seg_len += 1; }
    if seg.flags & FLAG_FIN != 0 { seg_len += 1; }

    let (seq, ack, flags) = if seg.flags & FLAG_ACK != 0 {
        (seg.ack, 0, super::tcp::FLAG_RST)
    } else {
        (0, seg.seq.wrapping_add(seg_len), super::tcp::FLAG_RST | FLAG_ACK)
    };

    let our_ip = super::NET.lock().ip;
    let our_mac = super::NET.lock().mac;

    let mut tcp_buf = [0u8; 40];
    let n = super::tcp::build(
        seg.dst_port, seg.src_port, seq, ack, flags, 0, &[],
        &our_ip, &ip_hdr.src, &mut tcp_buf,
    );
    if n == 0 { return; }

    let mut ip_buf = [0u8; 80];
    let ip_len = ipv4::build(&our_ip, &ip_hdr.src, ipv4::PROTO_TCP, &tcp_buf[..n], &mut ip_buf);
    if ip_len == 0 { return; }

    let mut eth_buf = [0u8; 100];
    let eth_len = EthFrame::build(&frame.src, &our_mac, ETHERTYPE_IP, &ip_buf[..ip_len], &mut eth_buf);
    if eth_len > 0 {
        tx(&eth_buf[..eth_len]);
    }
}
