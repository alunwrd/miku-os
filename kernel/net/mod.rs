// The byte layers are `mikunet`, the same crate `cd lib/net && cargo test` runs
// against
pub use mikunet::ipv4;
pub use mikunet::icmp;
pub use mikunet::stack::{arp, eth, udp};

pub mod dhcp;
pub mod tcp;
pub mod tcp_listener;

/// Uptime in milliseconds
///
/// `mikunet` counts in milliseconds because its tests need a clock they can
/// set. This is the one place the kernel's 250 Hz tick is converted
#[inline]
pub fn now_ms() -> u64 {
    crate::fs::procfs::uptime_ticks() * (1000 / crate::arch::x86_64::interrupts::PIT_HZ as u64)
}
pub mod irq;
pub mod socket;
pub mod raw;
pub mod rx;
pub mod selftest;

extern crate alloc;
use alloc::boxed::Box;
use arp::ArpTable;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use eth::{EthFrame, ETHERTYPE_IP};
use crate::drivers::bus::pci::{
    DEV_E1000_82540EM, DEV_E1000_82545EM, DEV_E1000_82574L, DEV_E1000_82579LM, DEV_E1000_I217,
    DEV_RTL8139, DEV_RTL8168, DEV_RTL8169, VENDOR_INTEL, VENDOR_REALTEK, DEV_VIRTIO_NET, VENDOR_VIRTIO,
};
use spin::Mutex;

static NET_READY: AtomicBool = AtomicBool::new(false);
pub static CTRL_C: AtomicBool = AtomicBool::new(false);
static DRIVER_NAME: Mutex<&'static str> = Mutex::new("none");

/// What the driver knows about a frame as it hands it up
#[derive(Clone, Copy, Default)]
pub struct RxMeta {
    /// True when the transport checksum must not be verified in software: the
    /// device already validated it, or (virtio) the backend handed the frame
    /// over with a deliberately incomplete checksum only it could complete.
    /// Verifying either would drop good packets
    pub skip_l4_csum: bool,
}

pub trait NetworkDriver: Send {
    fn send(&mut self, data: &[u8]) -> bool;
    /// Drains the receive ring, passing each frame to `handler`.
    ///
    /// The handler returns false to stop. A driver must honour that before it
    /// releases the descriptor and advances its tail, so the frame it did not
    /// take stays in the ring for the next call
    fn recv(&mut self, handler: &mut dyn FnMut(&[u8], RxMeta) -> bool);
    fn has_packet(&self) -> bool;
    fn link_up(&self) -> bool;
    fn get_mac(&self) -> [u8; 6];
    fn diag(&self) {}
    /// Have the card signal received frames (`net::irq`). Whether it will;
    /// a driver that cannot is pumped every tick
    fn enable_irq(&mut self, _f: crate::drivers::bus::msi::PciFn) -> bool {
        false
    }
}

pub(crate) struct NetState {
    driver: Option<Box<dyn NetworkDriver>>,
    mac: [u8; 6],
    ip: [u8; 4],
    gw: [u8; 4],
    mask: [u8; 4],
    dns: [u8; 4],
    arp: ArpTable,
    tx_count: u64,
    rx_count: u64,
    /// Frames the demux threw away, by reason. A healthy loopback run leaves
    /// all three at zero
    pub(crate) drop_csum: u64,
    pub(crate) drop_frag: u64,
    pub(crate) drop_addr: u64,
}

impl NetState {
    const fn new() -> Self {
        Self {
            driver: None,
            mac: [0; 6],
            ip: [0, 0, 0, 0],
            gw: [0, 0, 0, 0],
            mask: [0, 0, 0, 0],
            dns: [8, 8, 8, 8],
            arp: ArpTable::new(),
            tx_count: 0,
            rx_count: 0,
            drop_csum: 0,
            drop_frag: 0,
            drop_addr: 0,
        }
    }
}

pub static NET: Mutex<NetState> = Mutex::new(NetState::new());

/// Hands one built ethernet frame to the driver.
///
/// The single place a frame leaves the machine, and the only place tx_count is
/// raised, so a full transmit ring does not count as sent
pub fn tx_frame(frame: &[u8]) -> bool {
    let mut state = NET.lock();
    let sent = match state.driver.as_mut() {
        Some(drv) => drv.send(frame),
        None => false,
    };
    if sent {
        state.tx_count += 1;
    }
    sent
}

pub fn map_mmio(phys_addr: u64, size: u64) {
    crate::mm::vmm::map_mmio_uc(phys_addr, size);
}

pub fn init() -> Result<(), &'static str> {
    crate::serial_println!("[net] init: scanning PCI");
    let pci_dev = match crate::drivers::bus::pci::find_nic() {
        Some(d) => d,
        None => return Err("no network adapter found"),
    };

    crate::serial_println!(
        "[net] found: vendor={:04x} device={:04x} bus={:02x}:{:02x}.{}",
        pci_dev.vendor, pci_dev.device,
        pci_dev.bus, pci_dev.dev, pci_dev.func
    );
    crate::serial_println!(
        "[net] bars: {:08x} {:08x} {:08x} {:08x} {:08x} {:08x}  irq={}",
        pci_dev.bars[0], pci_dev.bars[1], pci_dev.bars[2],
        pci_dev.bars[3], pci_dev.bars[4], pci_dev.bars[5],
        pci_dev.irq
    );

    let mut state = NET.lock();
    let mut initialized_driver: Option<Box<dyn NetworkDriver>> = None;
    let mut drv_name: &'static str = "unknown";

    match (pci_dev.vendor, pci_dev.device) {
        (VENDOR_INTEL, DEV_E1000_82540EM | DEV_E1000_82545EM | DEV_E1000_82574L
            | DEV_E1000_82579LM | DEV_E1000_I217) => {
            crate::serial_println!("[net] init: e1000 map_mmio");
            if let Some(mem_phys) = pci_dev.mem_bar(0) {
                map_mmio(mem_phys, 128 * 1024);
            }
            crate::serial_println!("[net] init: e1000 driver init");
            if let Some(drv) = crate::drivers::net::e1000::E1000::new(&pci_dev) {
                state.mac = drv.get_mac();
                drv_name = crate::drivers::bus::pci::device_name(pci_dev.vendor, pci_dev.device);
                initialized_driver = Some(drv);
                crate::serial_println!("[net] init: e1000 ok");
            } else {
                crate::serial_println!("[net] init: e1000 driver returned None");
            }
        }
        (VENDOR_REALTEK, DEV_RTL8168) => {
            crate::serial_println!("[net] init: rtl8168 map_mmio");
            if let Some(mem_phys) = pci_dev.mem_bar(2)
                .or_else(|| pci_dev.mem_bar(1))
                .or_else(|| pci_dev.mem_bar(0))
            {
                map_mmio(mem_phys, 0x1000);
            }
            crate::serial_println!("[net] init: rtl8168 driver init");
            if let Some(drv) = crate::drivers::net::rtl8168::Rtl8168::new(&pci_dev) {
                state.mac = drv.get_mac();
                drv_name = "RTL8168 (r8168)";
                initialized_driver = Some(Box::new(drv));
            }
        }
        (VENDOR_REALTEK, DEV_RTL8139 | DEV_RTL8169) => {
            crate::serial_println!("[net] init: rtl8139 driver init");
            if let Some(drv) = crate::drivers::net::rtl8139::Rtl8139::new(&pci_dev) {
                state.mac = drv.get_mac();
                drv_name = crate::drivers::bus::pci::device_name(pci_dev.vendor, pci_dev.device);
                initialized_driver = Some(Box::new(drv));
            }
        }
        (VENDOR_VIRTIO, DEV_VIRTIO_NET) => {
            crate::serial_println!("[net] init: virtio-net driver init");
            if let Some(drv) = crate::drivers::net::virtio_net::VirtioNet::new(&pci_dev) {
                state.mac = drv.get_mac();
                drv_name = "VirtIO-net (legacy)";
                initialized_driver = Some(Box::new(drv));
            } else {
                return Err("virtio-net driver init failed");
            }
        }
        _ => return Err("unsupported network adapter"),
    }

    if let Some(mut drv) = initialized_driver {
        let f = crate::drivers::bus::msi::PciFn { bus: pci_dev.bus, dev: pci_dev.dev, func: pci_dev.func };
        if !drv.enable_irq(f) {
            crate::serial_println!("[net] {} is polled every tick", drv_name);
        }
        state.driver = Some(drv);
        let mac = state.mac;
        drop(state);
        *DRIVER_NAME.lock() = drv_name;
        NET_READY.store(true, Ordering::Release);
        crate::serial_println!(
            "[net] {} ready  mac: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            drv_name, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );
        Ok(())
    } else {
        Err("driver init failed")
    }
}

pub fn is_ready() -> bool {
    NET_READY.load(Ordering::Acquire)
}

/// Drains and dispatches pending RX. All the work is in the central demux (see
/// rx.rs)
pub fn poll() {
    rx::pump();
}

pub fn send_udp(dst_ip: &[u8; 4], dst_port: u16, src_port: u16, data: &[u8]) -> bool {
    if !is_ready() {
        return false;
    }
    let (mac, ip, dst_mac) = {
        let state = NET.lock();
        let mac = state.mac;
        let ip = state.ip;
        match state.arp.lookup(dst_ip, now_ms()) {
            Some(m) => (mac, ip, m),
            None => {
                drop(state);
                let mut arp_req = [0u8; 64];
                let n = arp::request_frame(&mac, &ip, dst_ip, &mut arp_req);
                tx_frame(&arp_req[..n]);
                return false;
            }
        }
    };

    let mut udp_buf = [0u8; 1500];
    let udp_len = udp::build(src_port, dst_port, data, &ip, dst_ip, &mut udp_buf);
    if udp_len == 0 { return false; }

    let mut ip_buf = [0u8; 1520];
    let ip_len = ipv4::build(&ip, dst_ip, ipv4::PROTO_UDP, &udp_buf[..udp_len], &mut ip_buf);
    if ip_len == 0 { return false; }

    let mut eth_buf = [0u8; 1540];
    let eth_len = EthFrame::build(&dst_mac, &mac, ETHERTYPE_IP, &ip_buf[..ip_len], &mut eth_buf);
    if eth_len == 0 { return false; }

    tx_frame(&eth_buf[..eth_len])
}

/// Send one UDP datagram, resolving the destination MAC first (limited
/// broadcast goes to FF:FF:FF:FF:FF:FF). Blocks only for ARP resolution.
/// This is the TX path of userspace SOCK_DGRAM sockets
pub fn udp_send_resolved(src_port: u16, dst_ip: &[u8; 4], dst_port: u16, data: &[u8]) -> bool {
    if !is_ready() {
        return false;
    }
    let our_ip = get_ip();
    let our_mac = get_mac();

    let dst_mac = if *dst_ip == [255, 255, 255, 255] {
        eth::BROADCAST_MAC
    } else {
        match resolve_arp(dst_ip, &our_ip, &our_mac) {
            Some(m) => m,
            None => return false,
        }
    };

    let mut udp_buf = [0u8; 1500];
    let udp_len = udp::build(src_port, dst_port, data, &our_ip, dst_ip, &mut udp_buf);
    if udp_len == 0 { return false; }

    let mut ip_buf = [0u8; 1520];
    let ip_len = ipv4::build(&our_ip, dst_ip, ipv4::PROTO_UDP, &udp_buf[..udp_len], &mut ip_buf);
    if ip_len == 0 { return false; }

    let mut eth_buf = [0u8; 1540];
    let eth_len = EthFrame::build(&dst_mac, &our_mac, ETHERTYPE_IP, &ip_buf[..ip_len], &mut eth_buf);
    if eth_len == 0 { return false; }

    tx_frame(&eth_buf[..eth_len])
}

/// Sends one IP packet with a payload the caller has already built
///
/// The route, the ARP lookup and the Ethernet frame are as for UDP; the
/// protocol and the hop limit come from the caller. A raw ICMP socket is the
/// only user
pub fn ip_send_ttl(dst_ip: &[u8; 4], proto: u8, payload: &[u8], ttl: u8) -> bool {
    if !is_ready() {
        return false;
    }
    let our_ip = get_ip();
    let our_mac = get_mac();

    let dst_mac = if *dst_ip == [255, 255, 255, 255] {
        eth::BROADCAST_MAC
    } else {
        match resolve_arp(dst_ip, &our_ip, &our_mac) {
            Some(m) => m,
            None => return false,
        }
    };

    let mut ip_buf = [0u8; 1520];
    let ip_len = ipv4::build_ttl(&our_ip, dst_ip, proto, payload, ttl, &mut ip_buf);
    if ip_len == 0 { return false; }

    let mut eth_buf = [0u8; 1540];
    let eth_len = EthFrame::build(&dst_mac, &our_mac, ETHERTYPE_IP, &ip_buf[..ip_len], &mut eth_buf);
    if eth_len == 0 { return false; }

    tx_frame(&eth_buf[..eth_len])
}

pub fn set_ip(ip: [u8; 4], gw: [u8; 4], mask: [u8; 4]) {
    let mut state = NET.lock();
    state.ip = ip;
    state.gw = gw;
    state.mask = mask;
}

pub fn get_mac() -> [u8; 6] { NET.lock().mac }

/// Where the next ephemeral port is taken; `u32::MAX` until the first
static NEXT_PORT: AtomicU32 = AtomicU32::new(u32::MAX);

/// An ephemeral port (49152-65535) for a connection or an unbound datagram
/// socket: each in turn from a random start (RFC 6056 3.3.2). The caller walks
/// on past one in use.
///
/// A port reused at once would give an HTTP client's next connection the
/// 4-tuple the server just closed and keeps in TIME_WAIT, and its SYN would be
/// taken for an old duplicate
pub fn ephemeral_port() -> u16 {
    if NEXT_PORT.load(Ordering::Relaxed) == u32::MAX {
        let start = crate::kcore::random::random_u64() as u32 & 0x3FFF;
        let _ = NEXT_PORT.compare_exchange(u32::MAX, start, Ordering::Relaxed, Ordering::Relaxed);
    }
    49152 + (NEXT_PORT.fetch_add(1, Ordering::Relaxed) & 0x3FFF) as u16
}
pub fn get_ip() -> [u8; 4] { NET.lock().ip }

pub fn get_dns() -> [u8; 4] { NET.lock().dns }
pub fn set_dns(dns: [u8; 4]) { NET.lock().dns = dns; }

/// netd is a mikuD background service, roughly analogous to systemd-networkd.
/// It waits for link-up, runs DHCP, and sleeps once a lease is applied.
/// Failures are logged but do not block boot
fn netd_link_wait(timeout_ticks: u64) -> bool {
    let start = crate::fs::procfs::uptime_ticks();
    loop {
        if is_ready() {
            let up = { NET.lock().driver.as_ref().map(|d| d.link_up()).unwrap_or(false) };
            if up { return true; }
        }
        if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= timeout_ticks {
            return false;
        }
        crate::sched::sleep(25); // 100 ms at 250 Hz
    }
}

pub fn netd_thread() -> ! {
    // Without a NIC there is nothing to do
    if !is_ready() {
        crate::serial_println!("[netd] no network adapter - going dormant");
        loop { crate::sched::sleep(60_000); }
    }

    // The card belongs to a program, so the wire is not touched at all: a DHCP
    // discover from here would be answered to a stack that never sees the
    // reply, and the lease would be taken from the one that does. Nor does this
    // pump; the reader's own `poll` drives the receive path
    if raw::enabled() {
        crate::serial_println!("[netd] card handed to /dev/net0 - standing down");
        loop { crate::sched::sleep(60_000); }
    }

    // Real switches and PHYs need a moment to come up. 3 s is enough for most
    // desktop NICs; if the cable is unplugged it proceeds anyway (the discover
    // times out and is retried later)
    let linked = netd_link_wait(750); // 3 s
    if !linked {
        crate::serial_println!("[netd] link not up after 3 s - trying DHCP anyway");
    } else {
        crate::serial_println!("[netd] link up - starting DHCP");
    }

    loop {
        // A few tries before giving up; do_dhcp() already retries discover 4x
        // internally, so this mostly covers an unplugged cable
        let mut lease_secs: u64 = 0;
        let mut got_lease = false;
        for attempt in 1..=3u32 {
            match dhcp::do_dhcp() {
                Some(r) => {
                    set_ip(r.ip, r.gw, r.mask);
                    set_dns(r.dns);
                    // The resolver cache is a program's, and a program lives
                    // for one command, so a new lease leaves no stale answer
                    // behind
                    lease_secs = r.lease_secs as u64;
                    crate::serial_println!(
                        "[netd] DHCP lease: ip={}.{}.{}.{} gw={}.{}.{}.{} dns={}.{}.{}.{} lease={}s",
                        r.ip[0], r.ip[1], r.ip[2], r.ip[3],
                        r.gw[0], r.gw[1], r.gw[2], r.gw[3],
                        r.dns[0], r.dns[1], r.dns[2], r.dns[3],
                        lease_secs,
                    );
                    got_lease = true;
                    break;
                }
                None => {
                    crate::serial_println!("[netd] DHCP attempt {} timed out", attempt);
                    crate::sched::sleep(500); // 2 s
                }
            }
        }
        if !got_lease {
            crate::serial_println!("[netd] no DHCP response - retrying in 5 min ('dhcp' forces it now)");
        }

        // RFC 2131: renew at T1 = lease/2. A server sending no lease time gets
        // a 1 h default; with no lease at all retry in 5 min
        let renew_ticks: u64 = if got_lease {
            let secs = if lease_secs > 0 { lease_secs } else { 3600 };
            secs.saturating_mul(TICK_HZ) / 2
        } else {
            300 * TICK_HZ
        };

        // Background pump until renewal is due: keeps ARP answered and TCP
        // segments queued even when no thread is blocked in a socket op. With
        // an interrupting card this sleeps until a frame comes; without one, a
        // tick (`net::irq::wait`)
        let start = crate::fs::procfs::uptime_ticks();
        while crate::fs::procfs::uptime_ticks().wrapping_sub(start) < renew_ticks {
            irq::take_pending();
            rx::pump();
            irq::wait();
        }
        crate::serial_println!("[netd] DHCP lease renewal due");
    }
}

/// Scheduler tick rate (ticks per second) used for lease arithmetic
const TICK_HZ: u64 = 250;

pub fn cmd_dhcp() {
    if !is_ready() {
        crate::print_error!("net: no adapter");
        return;
    }
    crate::print_info!("dhcp: sending discover...");
    match dhcp::do_dhcp() {
        Some(r) => {
            set_ip(r.ip, r.gw, r.mask);
            set_dns(r.dns);
            crate::print_success!(
                "dhcp: ip={}.{}.{}.{}  gw={}.{}.{}.{}  mask={}.{}.{}.{}  dns={}.{}.{}.{}",
                r.ip[0], r.ip[1], r.ip[2], r.ip[3],
                r.gw[0], r.gw[1], r.gw[2], r.gw[3],
                r.mask[0], r.mask[1], r.mask[2], r.mask[3],
                r.dns[0], r.dns[1], r.dns[2], r.dns[3],
            );
        }
        None => crate::print_error!("dhcp: no response (timeout)"),
    }
}

#[inline]
fn rdtsc() -> u64 {
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem, preserves_flags)
        );
        ((hi as u64) << 32) | lo as u64
    }
}

fn wait_rdtsc_ms(ms: u64) {
    if ms == 0 { return; }
    let khz = crate::kcore::time::tsc_khz().max(1);
    let target_cycles = ms * khz;
    let start = rdtsc();
    loop {
        if CTRL_C.load(Ordering::SeqCst) { return; }
        if rdtsc().wrapping_sub(start) >= target_cycles { return; }
        rx::pump();
        crate::sched::sleep(1); // a tick, not a spin: see sched::wait
    }
}

/// What a run of echo requests came back with. Times are microseconds
#[derive(Clone, Copy, Default)]
pub struct PingStats {
    pub sent: usize,
    pub received: usize,
    pub rtt_min_us: u64,
    pub rtt_max_us: u64,
    pub rtt_avg_us: u64,
}

/// Sends `count` echo requests and collects the replies, printing a line per
/// reply when `verbose`.
///
/// `/bin/ping` goes over an ICMP socket; this is for the selftest, which checks
/// reachability of the layer that is in here
pub fn ping_probe(target_ip: &[u8; 4], our_ip: &[u8; 4], count: usize, verbose: bool) -> PingStats {
    let mut stats = PingStats::default();
    if !is_ready() {
        return stats;
    }
    let our_ip = *our_ip;
    let our_mac = get_mac();
    let ping_id: u16 = 0x4D4B;
    let payload = b"MikuOS ping data 56b padding xxxxxxxxxxxxxxxxxxxxxxxxxxxxx";

    let mut sent = 0usize;
    let mut received = 0usize;
    let mut rtt_min = u64::MAX;
    let mut rtt_max = 0u64;
    let mut rtt_sum = 0u64;

    let dst_mac = match resolve_arp(target_ip, &our_ip, &our_mac) {
        Some(m) => m,
        None => {
            if verbose { crate::print_error!("ping: arp resolution failed"); }
            return stats;
        }
    };

    // Subscribe to ICMP events; the demux fans echo replies out to us while
    // any concurrent TCP/UDP traffic keeps flowing to its own consumers
    let icmp_h = match rx::register_icmp() {
        Some(h) => h,
        None => {
            if verbose { crate::print_error!("ping: too many icmp waiters"); }
            return stats;
        }
    };

    'ping: for seq in 1..=count {
        if CTRL_C.load(Ordering::SeqCst) {
            if verbose { crate::println!("^C"); }
            break;
        }

        let icmp_buf = icmp::echo_request(ping_id, seq as u16, &payload[..56]);
        let mut ip_buf = [0u8; 100];
        let ip_len = ipv4::build(&our_ip, target_ip, ipv4::PROTO_ICMP, &icmp_buf, &mut ip_buf);
        let mut eth_buf = [0u8; 128];
        let eth_len = EthFrame::build(&dst_mac, &our_mac, ETHERTYPE_IP, &ip_buf[..ip_len], &mut eth_buf);

        let t_start = rdtsc();

        if !tx_frame(&eth_buf[..eth_len]) {
            if verbose {
                crate::print_warn!("ping: transmit ring full, icmp_seq={} not sent", seq);
            }
            continue;
        }
        sent += 1;

        let mut got_reply = false;
        let t_start_wait = crate::fs::procfs::uptime_ticks();

        loop {
            if CTRL_C.load(Ordering::SeqCst) {
                if verbose { crate::println!("^C"); }
                break 'ping;
            }

            rx::pump();
            while let Some(ev) = rx::icmp_take(icmp_h) {
                if ev.icmp_type == icmp::ECHO_REPLY
                    && ev.id == ping_id
                    && ev.seq == seq as u16
                {
                    let t_end = rdtsc();
                    let khz = crate::kcore::time::tsc_khz().max(1);
                    let rtt_us = (t_end.wrapping_sub(t_start)) * 1000 / khz;
                    let ri = rtt_us / 1000;
                    let rf = (rtt_us % 1000) / 100;
                    rtt_sum += rtt_us;
                    if rtt_us < rtt_min { rtt_min = rtt_us; }
                    if rtt_us > rtt_max { rtt_max = rtt_us; }
                    received += 1;
                    if verbose {
                        crate::cprintln!(100, 220, 150,
                            "64 bytes from {}.{}.{}.{}: icmp_seq={} ttl={} time={}.{} ms",
                            target_ip[0], target_ip[1], target_ip[2], target_ip[3],
                            seq, ev.ttl, ri, rf
                        );
                    }
                    got_reply = true;
                }
            }

            if got_reply { break; }
            if crate::fs::procfs::uptime_ticks().wrapping_sub(t_start_wait) >= 2000 { break; }
            // Until the reply is queued (`rx::handle_icmp` wakes the key), or
            // the backstop
            let deadline = crate::kcore::time::now_ns().saturating_add(irq::backstop_ns());
            crate::sched::wait::keyed(crate::sched::wait::key::net_icmp(icmp_h))
                .wait_unless(deadline, || rx::icmp_pending(icmp_h));
        }

        if !got_reply && !CTRL_C.load(Ordering::SeqCst) && verbose {
            crate::print_error!("request timeout for icmp_seq={}", seq);
        }

        if seq < count {
            wait_rdtsc_ms(1000);
        }
    }

    rx::unregister_icmp(icmp_h);

    stats.sent = sent;
    stats.received = received;
    if received > 0 {
        stats.rtt_min_us = rtt_min;
        stats.rtt_max_us = rtt_max;
        stats.rtt_avg_us = rtt_sum / received as u64;
    }
    stats
}

fn is_same_subnet(ip: &[u8; 4], our_ip: &[u8; 4], mask: &[u8; 4]) -> bool {
    ip[0] & mask[0] == our_ip[0] & mask[0]
        && ip[1] & mask[1] == our_ip[1] & mask[1]
        && ip[2] & mask[2] == our_ip[2] & mask[2]
        && ip[3] & mask[3] == our_ip[3] & mask[3]
}

pub fn resolve_arp(target_ip: &[u8; 4], our_ip: &[u8; 4], our_mac: &[u8; 6]) -> Option<[u8; 6]> {
    let (mask, gw) = {
        let s = NET.lock();
        (s.mask, s.gw)
    };

    let arp_target = if is_same_subnet(target_ip, our_ip, &mask) {
        *target_ip
    } else {
        gw
    };

    if let Some(m) = NET.lock().arp.lookup(&arp_target, now_ms()) {
        return Some(m);
    }

    // The demux inserts every seen ARP (request or reply) into the table,
    // so we just send requests and poll the table
    for _attempt in 0..5 {
        if CTRL_C.load(Ordering::SeqCst) { return None; }

        {
            let mut req = [0u8; 64];
            let n = arp::request_frame(our_mac, our_ip, &arp_target, &mut req);
            tx_frame(&req[..n]);
        }

        let start = crate::fs::procfs::uptime_ticks();
        loop {
            if CTRL_C.load(Ordering::SeqCst) { return None; }
            rx::pump();
            if let Some(m) = NET.lock().arp.lookup(&arp_target, now_ms()) {
                return Some(m);
            }
            if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= 500 { break; }
            crate::sched::sleep(1); // a tick, not a spin: see sched::wait
        }
    }
    None
}

pub fn cmd_net(args: &str) {
    let args = args.trim();
    let mut parts = args.split_whitespace();
    let sub = parts.next().unwrap_or("status");
    match sub {
        "status" | "" => cmd_status(),
        "poll" => { poll(); crate::print_success!("poll done"); }
        "ip" => cmd_setip(
            parts.next().unwrap_or(""),
            parts.next().unwrap_or(""),
            parts.next().unwrap_or(""),
        ),
        "dns" => match parse_ip(parts.next().unwrap_or("")) {
            Some(ip) => {
                set_dns(ip);
                crate::print_success!("dns set: {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
            }
            None => {
                let d = get_dns();
                crate::cprintln!(230, 240, 240,
                    "dns: {}.{}.{}.{}", d[0], d[1], d[2], d[3]);
            }
        },
        "send" => cmd_send(
            parts.next().unwrap_or(""),
            parts.next().unwrap_or(""),
            parts.next().unwrap_or(""),
        ),
        "pci" => cmd_pci_scan(),
        "arp" => cmd_arp(),
        "diag" => cmd_diag(),
        "test" => cmd_selftest(),
        _ => crate::println!(
            "net status|poll|ip <ip> <gw> <mask>|dns [<ip>]|send ...|pci|arp|diag|test"),
    }
}

/// `net test`: runs the network selftest and prints the result.
///
/// Reachable from the boot line as `run=net_test`, so a headless QEMU run gets
/// a pass or fail from one boot
fn cmd_selftest() {
    crate::cprintln!(57, 197, 187, "net: selftest");
    let report = selftest::run();

    for c in &report.cases {
        if c.skipped {
            crate::cprintln!(220, 220, 100, "  skip  {}", c.name);
        } else if c.ok {
            crate::cprintln!(100, 220, 150, "  ok    {}", c.name);
        } else {
            crate::print_error!("  FAIL  {}", c.name);
        }
        crate::println!("          {}", c.detail);
        crate::serial_println!("[net-test] {} {} - {}",
            if c.skipped { "SKIP" } else if c.ok { "ok  " } else { "FAIL" },
            c.name, c.detail);
    }

    let (pass, fail, skip) = (report.passed(), report.failed(), report.skipped());
    crate::serial_println!("[net-test] {} passed, {} failed, {} skipped", pass, fail, skip);
    if fail == 0 {
        crate::print_success!("net: {} passed, {} skipped", pass, skip);
    } else {
        crate::print_error!("net: {} passed, {} FAILED, {} skipped", pass, fail, skip);
    }
}

fn cmd_status() {
    if !is_ready() { crate::print_error!("net: no adapter"); return; }
    let state = NET.lock();
    let drv_name = *DRIVER_NAME.lock();
    let link = state.driver.as_ref().map(|d| d.link_up()).unwrap_or(false);
    let mac = state.mac;
    crate::cprintln!(57, 197, 187, "  driver: {}", drv_name);
    if link {
        crate::cprintln!(100, 220, 150, "  link:   up");
    } else {
        crate::cprintln!(255, 80, 80,  "  link:   down");
    }
    crate::cprintln!(230, 240, 240,
        "  mac:    {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
    crate::cprintln!(230, 240, 240,
        "  ip:     {}.{}.{}.{}", state.ip[0], state.ip[1], state.ip[2], state.ip[3]);
    crate::cprintln!(230, 240, 240,
        "  gw:     {}.{}.{}.{}", state.gw[0], state.gw[1], state.gw[2], state.gw[3]);
    crate::cprintln!(230, 240, 240,
        "  mask:   {}.{}.{}.{}", state.mask[0], state.mask[1], state.mask[2], state.mask[3]);
    crate::cprintln!(230, 240, 240,
        "  dns:    {}.{}.{}.{}", state.dns[0], state.dns[1], state.dns[2], state.dns[3]);
    crate::cprintln!(120, 200, 200, "  tx:     {}", state.tx_count);
    crate::cprintln!(120, 200, 200, "  rx:     {}", state.rx_count);
    if state.drop_csum | state.drop_frag | state.drop_addr != 0 {
        crate::cprintln!(255, 180, 80,
            "  drops:  csum={} frag={} addr={}",
            state.drop_csum, state.drop_frag, state.drop_addr);
    }
}

fn cmd_setip(ip_str: &str, gw_str: &str, mask_str: &str) {
    let ip = parse_ip(ip_str).unwrap_or([10, 0, 2, 15]);
    let gw = parse_ip(gw_str).unwrap_or([10, 0, 2, 2]);
    let mask = parse_ip(mask_str).unwrap_or([255, 255, 255, 0]);
    set_ip(ip, gw, mask);
    crate::print_success!("ip set: {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
}

fn cmd_send(ip_str: &str, port_str: &str, msg: &str) {
    let ip = match parse_ip(ip_str) {
        Some(v) => v,
        None => { crate::println!("bad ip"); return; }
    };
    let port = parse_port(port_str);
    for _ in 0..3 { poll(); }
    if send_udp(&ip, port, 12345, msg.as_bytes()) {
        crate::print_success!("udp sent -> {}.{}.{}.{}:{}", ip[0], ip[1], ip[2], ip[3], port);
    } else {
        crate::print_error!("send failed (arp not resolved?)");
    }
}

fn cmd_pci_scan() {
    let (devs, n) = crate::drivers::bus::pci::scan();
    if n == 0 { crate::cprintln!(120, 140, 140, "no nics found"); return; }
    crate::cprintln!(57, 197, 187, "network cards (PCI class 0x02):");
    for i in 0..n {
        let d = &devs[i];
        crate::cprintln!(230, 240, 240,
            "  [{:02x}:{:02x}.{}] {:04x}:{:04x}  {}  irq={}",
            d.bus, d.dev, d.func, d.vendor, d.device,
            crate::drivers::bus::pci::device_name(d.vendor, d.device), d.irq
        );
    }
}

fn cmd_arp() {
    let state = NET.lock();
    crate::cprintln!(57, 197, 187, "arp table:");
    let mut found = false;
    for e in &state.arp.entries {
        if e.valid {
            found = true;
            crate::cprintln!(230, 240, 240,
                "  {}.{}.{}.{}  ->  {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                e.ip[0], e.ip[1], e.ip[2], e.ip[3],
                e.mac[0], e.mac[1], e.mac[2], e.mac[3], e.mac[4], e.mac[5]
            );
        }
    }
    if !found { crate::cprintln!(120, 140, 140, "  (empty)"); }
}

fn cmd_diag() {
    if !is_ready() { crate::print_error!("net: no adapter"); return; }
    let state = NET.lock();
    if let Some(drv) = state.driver.as_ref() {
        drv.diag();
    }
}

fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let mut p = s.split('.');
    Some([p.next()?.parse().ok()?, p.next()?.parse().ok()?,
          p.next()?.parse().ok()?, p.next()?.parse().ok()?])
}

fn parse_port(s: &str) -> u16 { s.parse().unwrap_or(8080) }
