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
// Before this module each protocol pulled frames straight off the driver and
// silently dropped everything not addressed to it, so two concurrent
// consumers (e.g. two TCP sockets, or ping during a download) stole each
// other's packets. Now any thread may call pump() at any time; a reentrancy
// guard makes concurrent calls cheap no-ops, and consumers only ever touch
// their own queue.
//
// Locking: pump() holds NET only while draining the driver or transmitting a
// reply, never while a registry mutex is held, so consumers that send with
// NET locked cannot deadlock against dispatch.

extern crate alloc;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use super::eth::{EthFrame, ETHERTYPE_ARP, ETHERTYPE_IP};
use super::tcp::{TcpSegment, FLAG_ACK, FLAG_SYN};
use super::{arp, ipv4, udp};

const MAX_TCP_CONNS: usize = 64;
const MAX_LISTENERS: usize = 16;
const MAX_UDP_BINDS: usize = 32;
const MAX_ICMP_WAITERS: usize = 8;

/// Per-queue depth limit; on overflow the newest packet is dropped (the
/// sender's retransmit machinery is the recovery path, same as a real NIC
/// ring overrun)
const QUEUE_CAP: usize = 64;
/// Frames drained from the driver per pump() call
const RX_BATCH: usize = 16;

/// One TCP segment as delivered to a connection queue
pub struct TcpPacket {
    pub seg: TcpSegment,
    pub payload: Vec<u8>,
    pub src_ip: [u8; 4],
    pub src_mac: [u8; 6],
    /// MSS option value when this segment is a SYN (536 if absent)
    pub syn_mss: u16,
}

/// A SYN parked on a listener's backlog, everything accept() needs to
/// finish the handshake
#[derive(Clone, Copy)]
pub struct SynRequest {
    pub src_ip: [u8; 4],
    pub src_mac: [u8; 6],
    pub src_port: u16,
    pub seq: u32,
    pub mss: u16,
}

pub struct UdpDgram {
    pub src_ip: [u8; 4],
    pub src_port: u16,
    pub data: Vec<u8>,
}

/// Non-echo-request ICMP event fanned out to all ICMP waiters. For echo
/// replies id/seq come from the ICMP header; for time-exceeded and
/// unreachable they are 0 (consumers match on icmp_type + src_ip)
#[derive(Clone, Copy)]
pub struct IcmpEvent {
    pub icmp_type: u8,
    pub src_ip: [u8; 4],
    pub id: u16,
    pub seq: u16,
    pub ttl: u8,
}

struct TcpConnSlot {
    local_port: u16,
    remote_ip: [u8; 4],
    remote_port: u16,
    queue: VecDeque<TcpPacket>,
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

// Slot arrays behind their own mutexes; None = free. Registration returns
// the slot index, which acts as the queue handle
static TCP_CONNS: Mutex<[Option<TcpConnSlot>; MAX_TCP_CONNS]> =
    Mutex::new([const { None }; MAX_TCP_CONNS]);
static LISTENERS: Mutex<[Option<ListenSlot>; MAX_LISTENERS]> =
    Mutex::new([const { None }; MAX_LISTENERS]);
static UDP_BINDS: Mutex<[Option<UdpSlot>; MAX_UDP_BINDS]> =
    Mutex::new([const { None }; MAX_UDP_BINDS]);
static ICMP_WAITERS: Mutex<[Option<IcmpSlot>; MAX_ICMP_WAITERS]> =
    Mutex::new([const { None }; MAX_ICMP_WAITERS]);

static PUMPING: AtomicBool = AtomicBool::new(false);

// Registration

pub fn register_tcp(local_port: u16, remote_ip: [u8; 4], remote_port: u16) -> Option<usize> {
    let mut tbl = TCP_CONNS.lock();
    for (i, slot) in tbl.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(TcpConnSlot {
                local_port,
                remote_ip,
                remote_port,
                queue: VecDeque::new(),
            });
            return Some(i);
        }
    }
    None
}

pub fn unregister_tcp(idx: usize) {
    if idx < MAX_TCP_CONNS {
        TCP_CONNS.lock()[idx] = None;
    }
}

pub fn tcp_take(idx: usize) -> Option<TcpPacket> {
    TCP_CONNS.lock().get_mut(idx)?.as_mut()?.queue.pop_front()
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
    for (i, slot) in tbl.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(ListenSlot { port, queue: VecDeque::new() });
            return Some(i);
        }
    }
    None
}

pub fn unregister_listener(idx: usize) {
    if idx < MAX_LISTENERS {
        LISTENERS.lock()[idx] = None;
    }
}

pub fn listener_take(idx: usize) -> Option<SynRequest> {
    LISTENERS.lock().get_mut(idx)?.as_mut()?.queue.pop_front()
}

pub fn register_udp(port: u16) -> Option<usize> {
    let mut tbl = UDP_BINDS.lock();
    if tbl.iter().flatten().any(|s| s.port == port) {
        return None; // EADDRINUSE
    }
    for (i, slot) in tbl.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(UdpSlot { port, queue: VecDeque::new() });
            return Some(i);
        }
    }
    None
}

pub fn unregister_udp(idx: usize) {
    if idx < MAX_UDP_BINDS {
        UDP_BINDS.lock()[idx] = None;
    }
}

pub fn udp_take(idx: usize) -> Option<UdpDgram> {
    UDP_BINDS.lock().get_mut(idx)?.as_mut()?.queue.pop_front()
}

pub fn udp_port_bound(port: u16) -> bool {
    UDP_BINDS.lock().iter().flatten().any(|s| s.port == port)
}

pub fn register_icmp() -> Option<usize> {
    let mut tbl = ICMP_WAITERS.lock();
    for (i, slot) in tbl.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(IcmpSlot { queue: VecDeque::new() });
            return Some(i);
        }
    }
    None
}

pub fn unregister_icmp(idx: usize) {
    if idx < MAX_ICMP_WAITERS {
        ICMP_WAITERS.lock()[idx] = None;
    }
}

pub fn icmp_take(idx: usize) -> Option<IcmpEvent> {
    ICMP_WAITERS.lock().get_mut(idx)?.as_mut()?.queue.pop_front()
}

// The pump

/// Drain the NIC and route every frame to its consumer queue. Safe to call
/// from any thread at any time; concurrent calls return immediately
pub fn pump() {
    if !super::is_ready() {
        return;
    }
    if PUMPING.swap(true, Ordering::Acquire) {
        return;
    }

    let mut frames: Vec<Vec<u8>> = Vec::new();
    {
        let mut state = match super::NET.try_lock() {
            Some(s) => s,
            None => {
                PUMPING.store(false, Ordering::Release);
                return;
            }
        };
        if let Some(drv) = state.driver.as_mut() {
            drv.recv(&mut |buf| {
                if frames.len() < RX_BATCH {
                    frames.push(Vec::from(buf));
                }
            });
        }
        state.rx_count += frames.len() as u64;
    }

    for raw in &frames {
        dispatch_frame(raw);
    }

    PUMPING.store(false, Ordering::Release);
}

/// Transmit a raw ethernet frame through the driver
fn tx(frame: &[u8]) {
    let mut state = super::NET.lock();
    if let Some(drv) = state.driver.as_mut() {
        if drv.send(frame) {
            state.tx_count += 1;
        }
    }
}

fn dispatch_frame(raw: &[u8]) {
    let frame = match EthFrame::parse(raw) {
        Some(f) => f,
        None => return,
    };

    match frame.ethertype {
        ETHERTYPE_ARP => handle_arp(&frame),
        ETHERTYPE_IP => handle_ip(&frame),
        _ => {}
    }
}

fn handle_arp(frame: &EthFrame) {
    let mut reply = [0u8; 64];
    let rlen = {
        let mut state = super::NET.lock();
        let mac = state.mac;
        let ip = state.ip;
        arp::handle(frame, &mac, &ip, &mut state.arp, &mut reply)
    };
    if rlen > 0 {
        tx(&reply[..rlen]);
    }
}

fn handle_ip(frame: &EthFrame) {
    let ip_hdr = match ipv4::Ipv4Header::parse(frame.payload) {
        Some(h) => h,
        None => return,
    };
    let payload = ip_hdr.payload(frame.payload);

    match ip_hdr.proto {
        ipv4::PROTO_ICMP => handle_icmp(frame, &ip_hdr, payload),
        ipv4::PROTO_UDP => handle_udp(&ip_hdr, payload),
        ipv4::PROTO_TCP => handle_tcp(frame, &ip_hdr, payload),
        _ => {}
    }
}

fn handle_icmp(frame: &EthFrame, ip_hdr: &ipv4::Ipv4Header, icmp: &[u8]) {
    if icmp.len() < 8 {
        return;
    }

    if icmp[0] == 8 {
        // Echo request: answer it right here
        let (our_ip, our_mac) = {
            let s = super::NET.lock();
            (s.ip, s.mac)
        };
        let mut ip_reply = [0u8; 1500];
        let ip_len = ipv4::handle_icmp(ip_hdr, &our_ip, icmp, &mut ip_reply);
        if ip_len > 0 {
            let mut eth_buf = [0u8; 1520];
            let n = EthFrame::build(&frame.src, &our_mac, ETHERTYPE_IP, &ip_reply[..ip_len], &mut eth_buf);
            if n > 0 {
                tx(&eth_buf[..n]);
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
    };

    let mut tbl = ICMP_WAITERS.lock();
    for slot in tbl.iter_mut().flatten() {
        if slot.queue.len() < QUEUE_CAP {
            slot.queue.push_back(ev);
        }
    }
}

fn handle_udp(ip_hdr: &ipv4::Ipv4Header, payload: &[u8]) {
    let hdr = match udp::UdpHeader::parse(payload) {
        Some(h) => h,
        None => return,
    };
    let data = hdr.payload(payload);

    let mut tbl = UDP_BINDS.lock();
    for slot in tbl.iter_mut().flatten() {
        if slot.port == hdr.dst_port {
            if slot.queue.len() < QUEUE_CAP {
                slot.queue.push_back(UdpDgram {
                    src_ip: ip_hdr.src,
                    src_port: hdr.src_port,
                    data: Vec::from(data),
                });
            }
            return;
        }
    }
    // No binding: drop silently (no ICMP port-unreachable yet)
}

fn handle_tcp(frame: &EthFrame, ip_hdr: &ipv4::Ipv4Header, payload: &[u8]) {
    let seg = match TcpSegment::parse(payload) {
        Some(s) => s,
        None => return,
    };

    // Established / in-handshake connection?
    {
        let mut tbl = TCP_CONNS.lock();
        for slot in tbl.iter_mut().flatten() {
            if slot.local_port == seg.dst_port
                && slot.remote_port == seg.src_port
                && slot.remote_ip == ip_hdr.src
            {
                if slot.queue.len() < QUEUE_CAP {
                    let syn_mss = if seg.flags & FLAG_SYN != 0 {
                        parse_mss_option(payload, &seg).unwrap_or(536)
                    } else {
                        536
                    };
                    slot.queue.push_back(TcpPacket {
                        seg,
                        payload: Vec::from(seg.payload(payload)),
                        src_ip: ip_hdr.src,
                        src_mac: frame.src,
                        syn_mss,
                    });
                }
                return;
            }
        }
    }

    // Fresh SYN aimed at a listening port?
    if seg.flags & FLAG_SYN != 0 && seg.flags & FLAG_ACK == 0 {
        let mss = parse_mss_option(payload, &seg).unwrap_or(536);
        let mut tbl = LISTENERS.lock();
        for slot in tbl.iter_mut().flatten() {
            if slot.port == seg.dst_port {
                if slot.queue.len() < QUEUE_CAP {
                    slot.queue.push_back(SynRequest {
                        src_ip: ip_hdr.src,
                        src_mac: frame.src,
                        src_port: seg.src_port,
                        seq: seg.seq,
                        mss,
                    });
                }
                return;
            }
        }
    }

    // Unknown connection: ignore. Deliberately no RST here - a stray RST
    // built from a half-parsed segment is how live connections get torn
    // down by accident (see the old poll() comment)
}

/// Extract the MSS option (kind 2) from a SYN's option block, if present
fn parse_mss_option(tcp_bytes: &[u8], seg: &TcpSegment) -> Option<u16> {
    let opts_end = (seg.data_offset as usize).min(tcp_bytes.len());
    if opts_end <= 20 {
        return None;
    }
    let opts = &tcp_bytes[20..opts_end];
    let mut i = 0;
    while i < opts.len() {
        match opts[i] {
            0 => return None,        // end of options
            1 => i += 1,             // NOP
            2 => {
                if i + 4 <= opts.len() && opts[i + 1] == 4 {
                    return Some(u16::from_be_bytes([opts[i + 2], opts[i + 3]]));
                }
                return None;
            }
            _ => {
                if i + 1 >= opts.len() {
                    return None;
                }
                let l = opts[i + 1] as usize;
                if l < 2 {
                    return None;
                }
                i += l;
            }
        }
    }
    None
}
