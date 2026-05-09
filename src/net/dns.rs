use super::eth::{EthFrame, ETHERTYPE_IP};
use super::ipv4;
use super::udp;
use super::CTRL_C;
use core::sync::atomic::Ordering;

pub fn resolve(hostname: &str, dns_server: &[u8; 4]) -> Option<[u8; 4]> {
    if hostname.is_empty() {
        return None;
    }

    CTRL_C.store(false, Ordering::SeqCst);

    let our_ip = super::get_ip();
    let our_mac = super::get_mac();

    let dst_mac = super::resolve_arp(dns_server, &our_ip, &our_mac)?;

    let mut query = [0u8; 512];
    let qlen = build_query(hostname, &mut query)?;

    let mut udp_buf = [0u8; 560];
    let udp_len = udp::build(54321, 53, &query[..qlen], &our_ip, dns_server, &mut udp_buf);
    if udp_len == 0 { return None; }

    let mut ip_buf = [0u8; 600];
    let ip_len = ipv4::build(&our_ip, dns_server, ipv4::PROTO_UDP, &udp_buf[..udp_len], &mut ip_buf);
    if ip_len == 0 { return None; }

    let mut eth_buf = [0u8; 650];
    let eth_len = EthFrame::build(&dst_mac, &our_mac, ETHERTYPE_IP, &ip_buf[..ip_len], &mut eth_buf);
    if eth_len == 0 { return None; }

    let mut raw: [[u8; 1520]; 4] = [[0; 1520]; 4];
    let mut raw_lens = [0usize; 4];

    for attempt in 0..4u32 {
        {
            let mut state = super::NET.lock();
            if let Some(drv) = state.driver.as_mut() {
                drv.send(&eth_buf[..eth_len]);
            }
            state.tx_count += 1;
        }
        crate::serial_println!("[dns] query attempt {}/4 for {}", attempt + 1, hostname);

        let start = crate::vfs::procfs::uptime_ticks();
        x86_64::instructions::interrupts::enable();

        loop {
            if CTRL_C.load(Ordering::SeqCst) {
                return None;
            }

            let now = crate::vfs::procfs::uptime_ticks();
            if now.wrapping_sub(start) > 750 {
                crate::serial_println!("[dns] attempt {}/4 timeout", attempt + 1);
                break;
            }

            x86_64::instructions::hlt();

            let mut raw_n = 0usize;
            {
                let mut state = super::NET.lock();
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
                    if frame.ethertype == super::eth::ETHERTYPE_ARP {
                        let mut state = super::NET.lock();
                        let mc = state.mac;
                        let ic = state.ip;
                        let mut rep = [0u8; 64];
                        let rlen = super::arp::handle(&frame, &mc, &ic, &mut state.arp, &mut rep);
                        if rlen > 0 {
                            let rc = rep;
                            if let Some(drv) = state.driver.as_mut() {
                                drv.send(&rc[..rlen]);
                            }
                        }
                        continue;
                    }
                    if frame.ethertype != ETHERTYPE_IP { continue; }
                    if let Some(ip) = ipv4::Ipv4Header::parse(frame.payload) {
                        if ip.proto != ipv4::PROTO_UDP { continue; }
                        let payload = ip.payload(frame.payload);
                        if payload.len() < 8 { continue; }
                        let src_port = u16::from_be_bytes([payload[0], payload[1]]);
                        if src_port != 53 { continue; }
                        let udp_data = &payload[8..];
                        if let Some(ip4) = parse_response(udp_data) {
                            return Some(ip4);
                        }
                    }
                }
            }
        }
    }
    None
}

fn build_query(hostname: &str, out: &mut [u8; 512]) -> Option<usize> {
    out[0] = 0x13;
    out[1] = 0x37;
    out[2] = 0x01;
    out[3] = 0x00;
    out[4] = 0x00;
    out[5] = 0x01;
    out[6] = 0x00; out[7] = 0x00;
    out[8] = 0x00; out[9] = 0x00;
    out[10] = 0x00; out[11] = 0x00;

    let mut pos = 12usize;

    for label in hostname.split('.') {
        if label.is_empty() { continue; }
        let lb = label.as_bytes();
        if pos + 1 + lb.len() >= 512 { return None; }
        out[pos] = lb.len() as u8;
        pos += 1;
        out[pos..pos + lb.len()].copy_from_slice(lb);
        pos += lb.len();
    }

    if pos + 5 >= 512 { return None; }
    out[pos] = 0x00;
    pos += 1;
    out[pos] = 0x00; out[pos+1] = 0x01;
    pos += 2;
    out[pos] = 0x00; out[pos+1] = 0x01;
    pos += 2;

    Some(pos)
}

fn parse_response(buf: &[u8]) -> Option<[u8; 4]> {
    if buf.len() < 12 { return None; }

    let flags = u16::from_be_bytes([buf[2], buf[3]]);
    if flags & 0x8000 == 0 { return None; }
    if flags & 0x000F != 0 { return None; }

    let qdcount = u16::from_be_bytes([buf[4], buf[5]]) as usize;
    let ancount = u16::from_be_bytes([buf[6], buf[7]]) as usize;

    if ancount == 0 { return None; }

    let mut pos = 12usize;

    for _ in 0..qdcount {
        pos = skip_name(buf, pos)?;
        pos += 4;
        if pos > buf.len() { return None; }
    }

    for _ in 0..ancount {
        pos = skip_name(buf, pos)?;
        if pos + 10 > buf.len() { return None; }

        let rtype = u16::from_be_bytes([buf[pos], buf[pos+1]]);
        let _rclass = u16::from_be_bytes([buf[pos+2], buf[pos+3]]);
        let rdlen = u16::from_be_bytes([buf[pos+8], buf[pos+9]]) as usize;
        pos += 10;

        if pos + rdlen > buf.len() { return None; }

        if rtype == 1 && rdlen == 4 {
            let mut ip = [0u8; 4];
            ip.copy_from_slice(&buf[pos..pos+4]);
            return Some(ip);
        }

        pos += rdlen;
    }

    None
}

fn skip_name(buf: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        if pos >= buf.len() { return None; }
        let b = buf[pos];
        if b == 0 {
            return Some(pos + 1);
        }
        if b & 0xC0 == 0xC0 {
            return Some(pos + 2);
        }
        pos += 1 + b as usize;
    }
}
