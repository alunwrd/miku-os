pub mod arp;
pub mod dns;
pub mod dhcp;
pub mod e1000;
pub mod eth;
pub mod icmp;
pub mod ipv4;
pub mod pci;
pub mod rtl8139;
pub mod rtl8168;
pub mod tcp;
pub mod tls;
pub mod tls_bignum;
pub mod tls_crypto;
pub mod tls_rsa;
pub mod udp;
pub mod tcp_listener;
pub mod virtio;
pub mod ntp;
pub mod traceroute;
pub mod http;
pub mod tls_ecdh;
pub mod http2;
pub mod tls_gcm;

extern crate alloc;
use alloc::boxed::Box;
use arp::ArpTable;
use core::sync::atomic::{AtomicBool, Ordering};
use eth::{EthFrame, ETHERTYPE_ARP, ETHERTYPE_IP};
use pci::{
    DEV_E1000_82540EM, DEV_E1000_82545EM, DEV_E1000_82574L, DEV_E1000_82579LM, DEV_E1000_I217,
    DEV_RTL8139, DEV_RTL8168, DEV_RTL8169, VENDOR_INTEL, VENDOR_REALTEK, DEV_VIRTIO_NET, VENDOR_VIRTIO,
};
use spin::Mutex;
use udp::UdpSocket;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{PageTable, PageTableFlags};

pub use crate::grub::HHDM as HHDM_OFFSET;
pub use crate::grub::{phys_to_virt, virt_to_phys};

static NET_READY: AtomicBool = AtomicBool::new(false);
pub static CTRL_C: AtomicBool = AtomicBool::new(false);
static DRIVER_NAME: Mutex<&'static str> = Mutex::new("none");

pub trait NetworkDriver: Send {
    fn send(&mut self, data: &[u8]) -> bool;
    fn recv(&mut self, handler: &mut dyn FnMut(&[u8]));
    fn has_packet(&self) -> bool;
    fn link_up(&self) -> bool;
    fn get_mac(&self) -> [u8; 6];
}

pub(crate) struct NetState {
    driver: Option<Box<dyn NetworkDriver>>,
    mac: [u8; 6],
    ip: [u8; 4],
    gw: [u8; 4],
    mask: [u8; 4],
    dns: [u8; 4],
    arp: ArpTable,
    udp: UdpSocket,
    tx_count: u64,
    rx_count: u64,
}

impl NetState {
    const fn new() -> Self {
        Self {
            driver: None,
            mac: [0; 6],
            ip: [10, 0, 2, 15],
            gw: [10, 0, 2, 2],
            mask: [255, 255, 255, 0],
            dns: [8, 8, 8, 8],
            arp: ArpTable::new(),
            udp: UdpSocket::new(6969),
            tx_count: 0,
            rx_count: 0,
        }
    }
}

pub static NET: Mutex<NetState> = Mutex::new(NetState::new());

fn alloc_pt_phys() -> u64 {
    let phys = crate::pmm::alloc_frame()
        .expect("map_mmio: out of physical memory for page table");
    let hhdm = crate::grub::hhdm();
    unsafe {
        let ptr = (phys + hhdm) as *mut u8;
        core::ptr::write_bytes(ptr, 0, 4096);
    }
    phys
}

unsafe fn split_huge_p3(p3: &mut PageTable, p3_idx: usize, hhdm: u64) {
    let huge_phys = p3[p3_idx].addr().as_u64();
    let huge_flags = p3[p3_idx].flags();

    let new_p2_phys = alloc_pt_phys();
    let new_p2_ptr = (new_p2_phys + hhdm) as *mut PageTable;
    let new_p2 = &mut *new_p2_ptr;

    for j in 0..512usize {
        let page_phys = huge_phys + (j as u64) * 0x20_0000;
        let mut flags = huge_flags;
        flags.remove(PageTableFlags::HUGE_PAGE);
        flags.insert(PageTableFlags::PRESENT | PageTableFlags::WRITABLE);

        let new_p1_phys = alloc_pt_phys();
        let new_p1_ptr = (new_p1_phys + hhdm) as *mut PageTable;
        let new_p1 = &mut *new_p1_ptr;

        for k in 0..512usize {
            let phys_4k = page_phys + (k as u64) * 0x1000;
            new_p1[k].set_addr(
                x86_64::PhysAddr::new(phys_4k),
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
            );
        }

        new_p2[j].set_addr(x86_64::PhysAddr::new(new_p1_phys), flags);
    }

    p3[p3_idx].set_addr(
        x86_64::PhysAddr::new(new_p2_phys),
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
    );
}

unsafe fn split_huge_p2(p2: &mut PageTable, p2_idx: usize, hhdm: u64) {
    let huge_phys = p2[p2_idx].addr().as_u64();
    let huge_flags = p2[p2_idx].flags();

    let new_p1_phys = alloc_pt_phys();
    let new_p1_ptr = (new_p1_phys + hhdm) as *mut PageTable;
    let new_p1 = &mut *new_p1_ptr;

    for k in 0..512usize {
        let phys_4k = huge_phys + (k as u64) * 0x1000;
        let mut flags = huge_flags;
        flags.remove(PageTableFlags::HUGE_PAGE);
        flags.insert(PageTableFlags::PRESENT | PageTableFlags::WRITABLE);
        new_p1[k].set_addr(x86_64::PhysAddr::new(phys_4k), flags);
    }

    p2[p2_idx].set_addr(
        x86_64::PhysAddr::new(new_p1_phys),
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
    );
}

pub fn map_mmio(phys_addr: u64, size: u64) {
    let hhdm = crate::grub::hhdm();
    let start_page = phys_addr & !0xFFF;
    let end_page = (phys_addr + size + 0xFFF) & !0xFFF;

    unsafe {
        let (p4_frame, _) = Cr3::read();
        let p4_ptr = (p4_frame.start_address().as_u64() + hhdm) as *mut PageTable;
        let p4 = &mut *p4_ptr;

        for page in (start_page..end_page).step_by(0x1000) {
            let virt = page + hhdm;
            let p4_idx = ((virt >> 39) & 0x1FF) as usize;
            let p3_idx = ((virt >> 30) & 0x1FF) as usize;
            let p2_idx = ((virt >> 21) & 0x1FF) as usize;
            let p1_idx = ((virt >> 12) & 0x1FF) as usize;

            if !p4[p4_idx].flags().contains(PageTableFlags::PRESENT) {
                p4[p4_idx].set_addr(
                    x86_64::PhysAddr::new(alloc_pt_phys()),
                    PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
                );
            }
            let p3_ptr = (p4[p4_idx].addr().as_u64() + hhdm) as *mut PageTable;
            let p3 = &mut *p3_ptr;

            if !p3[p3_idx].flags().contains(PageTableFlags::PRESENT) {
                p3[p3_idx].set_addr(
                    x86_64::PhysAddr::new(alloc_pt_phys()),
                    PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
                );
            } else if p3[p3_idx].flags().contains(PageTableFlags::HUGE_PAGE) {
                split_huge_p3(p3, p3_idx, hhdm);
                x86_64::instructions::tlb::flush_all();
            }

            let p2_ptr = (p3[p3_idx].addr().as_u64() + hhdm) as *mut PageTable;
            let p2 = &mut *p2_ptr;

            if !p2[p2_idx].flags().contains(PageTableFlags::PRESENT) {
                p2[p2_idx].set_addr(
                    x86_64::PhysAddr::new(alloc_pt_phys()),
                    PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
                );
            } else if p2[p2_idx].flags().contains(PageTableFlags::HUGE_PAGE) {
                split_huge_p2(p2, p2_idx, hhdm);
                x86_64::instructions::tlb::flush_all();
            }

            let p1_ptr = (p2[p2_idx].addr().as_u64() + hhdm) as *mut PageTable;
            let p1 = &mut *p1_ptr;
            let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;
            p1[p1_idx].set_addr(x86_64::PhysAddr::new(page), flags);
        }

        core::arch::asm!("mfence", options(nostack, nomem));
        x86_64::instructions::tlb::flush_all();
    }
}

pub fn init() -> Result<(), &'static str> {
    crate::serial_println!("[net] init: scanning PCI");
    let pci_dev = match pci::find_nic() {
        Some(d) => d,
        None => return Err("no network adapter found"),
    };

    crate::serial_println!(
        "[net] found: vendor={:04x} device={:04x} bus={:02x}:{:02x}.{}",
        pci_dev.vendor, pci_dev.device,
        pci_dev.bus, pci_dev.dev, pci_dev.func
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
            if let Some(drv) = e1000::E1000::new(&pci_dev) {
                state.mac = drv.get_mac();
                drv_name = pci::device_name(pci_dev.vendor, pci_dev.device);
                initialized_driver = Some(drv);
                crate::serial_println!("[net] init: e1000 ok");
            } else {
                crate::serial_println!("[net] init: e1000 driver returned None");
            }
        }
        (VENDOR_REALTEK, DEV_RTL8168) => {
            crate::serial_println!("[net] init: rtl8168 map_mmio");
            if let Some(mem_phys) = pci_dev.mem_bar(1).or_else(|| pci_dev.mem_bar(0)) {
                map_mmio(mem_phys, 0x1000);
            }
            crate::serial_println!("[net] init: rtl8168 driver init");
            if let Some(drv) = rtl8168::Rtl8168::new(&pci_dev) {
                state.mac = drv.get_mac();
                drv_name = "RTL8168 (r8168)";
                initialized_driver = Some(Box::new(drv));
            }
        }
        (VENDOR_REALTEK, DEV_RTL8139 | DEV_RTL8169) => {
            crate::serial_println!("[net] init: rtl8139 driver init");
            if let Some(drv) = rtl8139::Rtl8139::new(&pci_dev) {
                state.mac = drv.get_mac();
                drv_name = pci::device_name(pci_dev.vendor, pci_dev.device);
                initialized_driver = Some(Box::new(drv));
            }
        }
        (VENDOR_VIRTIO, DEV_VIRTIO_NET) => {
            crate::serial_println!("[net] init: virtio-net driver init");
            if let Some(drv) = virtio::VirtioNet::new(&pci_dev) {
                state.mac = drv.get_mac();
                drv_name = "VirtIO-net (legacy)";
                initialized_driver = Some(Box::new(drv));
            } else {
                return Err("virtio-net driver init failed");
            }
        }
        _ => return Err("unsupported network adapter"),
    }

    if let Some(drv) = initialized_driver {
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

pub fn poll_from_irq() {
    if !NET_READY.load(Ordering::Acquire) {
        return;
    }

    let mut state = match NET.try_lock() {
        Some(s) => s,
        None    => return,
    };

    let mut packets = [[0u8; 1520]; 4];
    let mut pkt_lens = [0usize; 4];
    let mut pkt_count = 0usize;

    if let Some(drv) = state.driver.as_mut() {
        drv.recv(&mut |buf| {
            if pkt_count < 4 {
                let len = buf.len().min(1520);
                packets[pkt_count][..len].copy_from_slice(&buf[..len]);
                pkt_lens[pkt_count] = len;
                pkt_count += 1;
            }
        });
    }

    let mac = state.mac;
    let ip  = state.ip;
    let mut tx_frames = [[0u8; 64]; 4];
    let mut tx_lens = [0usize; 4];
    let mut tx_count = 0usize;

    for i in 0..pkt_count {
        state.rx_count += 1;
        let buf = &packets[i][..pkt_lens[i]];

        if let Some(frame) = eth::EthFrame::parse(buf) {
            if frame.ethertype == eth::ETHERTYPE_ARP && tx_count < 4 {
                let mut arp_buf = [0u8; 64];
                let n = arp::handle(&frame, &mac, &ip, &mut state.arp, &mut arp_buf);
                if n > 0 {
                    tx_frames[tx_count][..n].copy_from_slice(&arp_buf[..n]);
                    tx_lens[tx_count] = n;
                    tx_count += 1;
                }
            }
        }
    }

    let mut tx_ok = 0u64;
    for i in 0..tx_count {
        if let Some(drv) = state.driver.as_mut() {
            if drv.send(&tx_frames[i][..tx_lens[i]]) {
                tx_ok += 1;
            }
        }
    }
    state.tx_count += tx_ok;
}

pub fn poll() {
    if !is_ready() {
        return;
    }
    let mut state = NET.lock();

    let mut packets = [[0u8; 1520]; 8];
    let mut pkt_lens = [0usize; 8];
    let mut pkt_count = 0usize;

    if let Some(drv) = state.driver.as_mut() {
        drv.recv(&mut |buf| {
            if pkt_count < 8 {
                let len = buf.len().min(1520);
                packets[pkt_count][..len].copy_from_slice(&buf[..len]);
                pkt_lens[pkt_count] = len;
                pkt_count += 1;
            }
        });
    }

    let mac = state.mac;
    let ip = state.ip;
    let mut tx_frames = [[0u8; 1520]; 4];
    let mut tx_lens = [0usize; 4];
    let mut tx_count = 0usize;

    for i in 0..pkt_count {
        state.rx_count += 1;
        let buf = &packets[i][..pkt_lens[i]];

        if let Some(frame) = EthFrame::parse(buf) {
            match frame.ethertype {
                ETHERTYPE_ARP => {
                    if tx_count < 4 {
                        let mut arp_buf = [0u8; 64];
                        let n = arp::handle(&frame, &mac, &ip, &mut state.arp, &mut arp_buf);
                        if n > 0 {
                            tx_frames[tx_count][..n].copy_from_slice(&arp_buf[..n]);
                            tx_lens[tx_count] = n;
                            tx_count += 1;
                        }
                    }
                }
                ETHERTYPE_IP => {
                    if let Some(ip_hdr) = ipv4::Ipv4Header::parse(frame.payload) {
                        let payload = ip_hdr.payload(frame.payload);
                        match ip_hdr.proto {
                            ipv4::PROTO_ICMP => {
                                if tx_count < 4 {
                                    let mut ip_reply = [0u8; 1500];
                                    let ip_len = ipv4::handle_icmp(
                                        &ip_hdr, &ip, payload, &mut ip_reply,
                                    );
                                    if ip_len > 0 {
                                        let n = EthFrame::build(
                                            &frame.src, &mac, ETHERTYPE_IP,
                                            &ip_reply[..ip_len], &mut tx_frames[tx_count],
                                        );
                                        if n > 0 {
                                            tx_lens[tx_count] = n;
                                            tx_count += 1;
                                        }
                                    }
                                }
                            }
                            ipv4::PROTO_UDP => {
                                if let Some(udp_hdr) = udp::UdpHeader::parse(payload) {
                                    if udp_hdr.dst_port == state.udp.port {
                                        state.udp.on_recv(
                                            &ip_hdr.src,
                                            udp_hdr.src_port,
                                            udp_hdr.payload(payload),
                                        );
                                    }
                                }
                            }
                            ipv4::PROTO_TCP => {
                                // active TCP sockets/listeners handle their own traffic;
                                // here we do nothing rather than blast RSTs with a broken
                                // (IP-less) payload that also tears down live connections.
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let mut tx_success = 0;
    for i in 0..tx_count {
        if let Some(drv) = state.driver.as_mut() {
            if drv.send(&tx_frames[i][..tx_lens[i]]) {
                tx_success += 1;
            }
        }
    }
    state.tx_count += tx_success;
}

pub fn send_udp(dst_ip: &[u8; 4], dst_port: u16, src_port: u16, data: &[u8]) -> bool {
    if !is_ready() {
        return false;
    }
    let mut state = NET.lock();
    let mac = state.mac;
    let ip = state.ip;

    let dst_mac = match state.arp.lookup(dst_ip) {
        Some(m) => m,
        None => {
            let mut arp_req = [0u8; 64];
            let n = arp::send_request(&mac, &ip, dst_ip, &mut arp_req);
            if let Some(drv) = state.driver.as_mut() {
                drv.send(&arp_req[..n]);
            }
            return false;
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

    if let Some(drv) = state.driver.as_mut() {
        if drv.send(&eth_buf[..eth_len]) {
            state.tx_count += 1;
            return true;
        }
    }
    false
}

pub fn set_ip(ip: [u8; 4], gw: [u8; 4], mask: [u8; 4]) {
    let mut state = NET.lock();
    state.ip = ip;
    state.gw = gw;
    state.mask = mask;
}

pub fn get_mac() -> [u8; 6] { NET.lock().mac }
pub fn get_ip() -> [u8; 4] { NET.lock().ip }

pub fn get_dns() -> [u8; 4] { NET.lock().dns }
pub fn set_dns(dns: [u8; 4]) { NET.lock().dns = dns; }

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
    let khz = crate::timing::tsc_khz().max(1);
    let target_cycles = ms * khz;
    let start = rdtsc();
    loop {
        if CTRL_C.load(Ordering::SeqCst) { return; }
        if rdtsc().wrapping_sub(start) >= target_cycles { return; }
        core::hint::spin_loop();
    }
}

pub fn cmd_ping(hostname: &str, target_ip: &[u8; 4], count: usize) {
    if !is_ready() {
        crate::print_error!("ping: no adapter");
        return;
    }

    let our_ip = get_ip();
    let our_mac = get_mac();
    let ping_id: u16 = 0x4D4B;
    let payload = b"MikuOS ping data 56b padding xxxxxxxxxxxxxxxxxxxxxxxxxxxxx";

    crate::cprintln!(57, 197, 187,
        "PING {} ({}.{}.{}.{}): 56 bytes of data.",
        hostname,
        target_ip[0], target_ip[1], target_ip[2], target_ip[3]
    );

    CTRL_C.store(false, Ordering::SeqCst);
    x86_64::instructions::interrupts::enable();

    let mut sent = 0usize;
    let mut received = 0usize;
    let mut rtt_min = u64::MAX;
    let mut rtt_max = 0u64;
    let mut rtt_sum = 0u64;

    let dst_mac = match resolve_arp(target_ip, &our_ip, &our_mac) {
        Some(m) => m,
        None => {
            crate::print_error!("ping: arp resolution failed");
            return;
        }
    };

    'ping: for seq in 1..=count {
        if CTRL_C.load(Ordering::SeqCst) {
            crate::println!("^C");
            break;
        }

        let mut icmp_buf = [0u8; 64];
        let icmp_len = icmp::build_echo_request(ping_id, seq as u16, &payload[..56], &mut icmp_buf);
        let mut ip_buf = [0u8; 100];
        let ip_len = ipv4::build(&our_ip, target_ip, ipv4::PROTO_ICMP, &icmp_buf[..icmp_len], &mut ip_buf);
        let mut eth_buf = [0u8; 128];
        let eth_len = EthFrame::build(&dst_mac, &our_mac, ETHERTYPE_IP, &ip_buf[..ip_len], &mut eth_buf);

        let t_start = rdtsc();

        {
            let mut state = NET.lock();
            if let Some(drv) = state.driver.as_mut() {
                drv.send(&eth_buf[..eth_len]);
            }
            state.tx_count += 1;
        }
        sent += 1;

        let mut got_reply = false;
        let t_start_wait = crate::vfs::procfs::uptime_ticks();

        loop {
            if CTRL_C.load(Ordering::SeqCst) {
                crate::println!("^C");
                break 'ping;
            }

            let mut raw: [[u8; 1520]; 4] = [[0; 1520]; 4];
            let mut raw_lens = [0usize; 4];
            let mut raw_n = 0usize;

            {
                let mut state = NET.lock();
                if let Some(drv) = state.driver.as_mut() {
                    drv.recv(&mut |buf| {
                        if raw_n < 4 {
                            let l = buf.len().min(1520);
                            raw[raw_n][..l].copy_from_slice(&buf[..l]);
                            raw_lens[raw_n] = l;
                            raw_n += 1;
                        }
                    });
                }
                state.rx_count += raw_n as u64;
            }

            for i in 0..raw_n {
                let buf = &raw[i][..raw_lens[i]];
                if let Some(frame) = EthFrame::parse(buf) {
                    if frame.ethertype == ETHERTYPE_IP {
                        if let Some(r) = icmp::parse_echo_reply(frame.payload) {
                            if r.id == ping_id && r.seq == seq as u16 {
                                let t_end = rdtsc();
                                let khz = crate::timing::tsc_khz().max(1);
                                let rtt_us = (t_end.wrapping_sub(t_start)) * 1000 / khz;
                                let ri = rtt_us / 1000;
                                let rf = (rtt_us % 1000) / 100;
                                rtt_sum += rtt_us;
                                if rtt_us < rtt_min { rtt_min = rtt_us; }
                                if rtt_us > rtt_max { rtt_max = rtt_us; }
                                received += 1;
                                crate::cprintln!(100, 220, 150,
                                    "64 bytes from {}.{}.{}.{}: icmp_seq={} ttl={} time={}.{} ms",
                                    target_ip[0], target_ip[1], target_ip[2], target_ip[3],
                                    seq, r.ttl, ri, rf
                                );
                                got_reply = true;
                            }
                        }
                    }
                    if frame.ethertype == ETHERTYPE_ARP {
                        let mut state = NET.lock();
                        let mc = state.mac;
                        let ic = state.ip;
                        let mut rep = [0u8; 64];
                        let rlen = arp::handle(&frame, &mc, &ic, &mut state.arp, &mut rep);
                        if rlen > 0 {
                            let rc = rep;
                            if let Some(drv) = state.driver.as_mut() { drv.send(&rc[..rlen]); }
                        }
                    }
                }
            }

            if got_reply { break; }
            if crate::vfs::procfs::uptime_ticks().wrapping_sub(t_start_wait) >= 2000 { break; }
            core::hint::spin_loop();
        }

        if !got_reply && !CTRL_C.load(Ordering::SeqCst) {
            crate::print_error!("request timeout for icmp_seq={}", seq);
        }

        if seq < count {
            wait_rdtsc_ms(1000);
        }
    }

    crate::cprintln!(57, 197, 187, "");
    crate::cprintln!(57, 197, 187,
        "--- {}.{}.{}.{} ping statistics ---",
        target_ip[0], target_ip[1], target_ip[2], target_ip[3]
    );
    let loss = if sent > 0 { ((sent - received) * 100) / sent } else { 100 };
    crate::cprintln!(230, 240, 240,
        "{} packets transmitted, {} received, {}% packet loss",
        sent, received, loss
    );
    if received > 0 {
        let avg = rtt_sum / received as u64;
        crate::cprintln!(230, 240, 240,
            "rtt min/avg/max = {}.{}/{}.{}/{}.{} ms",
            rtt_min / 1000, (rtt_min % 1000) / 100,
            avg / 1000, (avg % 1000) / 100,
            rtt_max / 1000, (rtt_max % 1000) / 100,
        );
    }
}

fn is_same_subnet(ip: &[u8; 4], our_ip: &[u8; 4], mask: &[u8; 4]) -> bool {
    ip[0] & mask[0] == our_ip[0] & mask[0]
        && ip[1] & mask[1] == our_ip[1] & mask[1]
        && ip[2] & mask[2] == our_ip[2] & mask[2]
        && ip[3] & mask[3] == our_ip[3] & mask[3]
}

fn net_recv_once() {
    let mut raw: [[u8; 1520]; 4] = [[0; 1520]; 4];
    let mut raw_lens = [0usize; 4];
    let mut raw_n = 0usize;
    {
        let mut state = NET.lock();
        if let Some(drv) = state.driver.as_mut() {
            drv.recv(&mut |buf| {
                if raw_n < 4 {
                    let l = buf.len().min(1520);
                    raw[raw_n][..l].copy_from_slice(&buf[..l]);
                    raw_lens[raw_n] = l;
                    raw_n += 1;
                }
            });
        }
        state.rx_count += raw_n as u64;
    }
    for i in 0..raw_n {
        let buf = &raw[i][..raw_lens[i]];
        if let Some(frame) = EthFrame::parse(buf) {
            if frame.ethertype == ETHERTYPE_ARP {
                let mut state = NET.lock();
                let mc = state.mac;
                let ic = state.ip;
                let mut rep = [0u8; 64];
                let rlen = arp::handle(&frame, &mc, &ic, &mut state.arp, &mut rep);
                if rlen > 0 {
                    let rep_copy = rep;
                    if let Some(drv) = state.driver.as_mut() {
                        drv.send(&rep_copy[..rlen]);
                    }
                }
            }
        }
    }
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

    if let Some(m) = NET.lock().arp.lookup(&arp_target) {
        return Some(m);
    }

    for _attempt in 0..5 {
        if CTRL_C.load(Ordering::SeqCst) { return None; }

        {
            let mut req = [0u8; 64];
            let n = arp::send_request(our_mac, our_ip, &arp_target, &mut req);
            let mut s = NET.lock();
            if let Some(d) = s.driver.as_mut() { d.send(&req[..n]); }
        }

        let start = crate::vfs::procfs::uptime_ticks();
        loop {
            if CTRL_C.load(Ordering::SeqCst) { return None; }
            net_recv_once();
            if let Some(m) = NET.lock().arp.lookup(&arp_target) {
                return Some(m);
            }
            if crate::vfs::procfs::uptime_ticks().wrapping_sub(start) >= 500 { break; }
            core::hint::spin_loop();
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
        _ => crate::println!("net status|poll|ip <ip> <gw> <mask>|dns [<ip>]|send ...|pci|arp"),
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
    let (devs, n) = pci::scan();
    if n == 0 { crate::cprintln!(120, 140, 140, "no nics found"); return; }
    crate::cprintln!(57, 197, 187, "network cards (PCI class 0x02):");
    for i in 0..n {
        let d = &devs[i];
        crate::cprintln!(230, 240, 240,
            "  [{:02x}:{:02x}.{}] {:04x}:{:04x}  {}  irq={}",
            d.bus, d.dev, d.func, d.vendor, d.device,
            pci::device_name(d.vendor, d.device), d.irq
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

fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let mut p = s.split('.');
    Some([p.next()?.parse().ok()?, p.next()?.parse().ok()?,
          p.next()?.parse().ok()?, p.next()?.parse().ok()?])
}

fn parse_port(s: &str) -> u16 { s.parse().unwrap_or(8080) }
