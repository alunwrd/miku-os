use super::eth::{EthFrame, ETHERTYPE_IP};
use super::{icmp, ipv4, rx};
use super::CTRL_C;
use core::sync::atomic::Ordering;

const MAX_HOPS: u8 = 30;
const PROBE_ID: u16 = 0x5452;

fn rdtsc() -> u64 {
    unsafe {
        let lo: u32; let hi: u32;
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem));
        ((hi as u64) << 32) | lo as u64
    }
}

fn build_icmp_with_ttl(
    our_ip: &[u8; 4],
    dst_ip: &[u8; 4],
    ttl: u8,
    id: u16,
    seq: u16,
    payload: &[u8],
    out: &mut [u8],
) -> usize {
    let mut icmp_buf = [0u8; 128];
    let icmp_len = icmp::build_echo_request(id, seq, payload, &mut icmp_buf);
    if icmp_len == 0 { return 0; }

    let total = 20 + icmp_len;
    if out.len() < total { return 0; }

    out[0]  = 0x45;
    out[1]  = 0;
    out[2]  = (total >> 8) as u8;
    out[3]  = total as u8;
    out[4]  = (id >> 8) as u8;
    out[5]  = id as u8;
    out[6]  = 0; out[7] = 0;
    out[8]  = ttl;
    out[9]  = ipv4::PROTO_ICMP;
    out[10] = 0; out[11] = 0;
    out[12..16].copy_from_slice(our_ip);
    out[16..20].copy_from_slice(dst_ip);
    out[20..20 + icmp_len].copy_from_slice(&icmp_buf[..icmp_len]);

    let csum = ipv4::checksum(&out[..20]);
    out[10] = (csum >> 8) as u8;
    out[11] = csum as u8;

    total
}

pub fn run(hostname: &str, target_ip: &[u8; 4]) {
    if !super::is_ready() {
        crate::print_error!("traceroute: no adapter");
        return;
    }

    let our_ip  = super::get_ip();
    let our_mac = super::get_mac();

    let payload = b"traceroute_probe_xxxxxxxxxxxxxxxx";

    crate::cprintln!(57, 197, 187,
        "traceroute to {} ({}.{}.{}.{}), {} hops max",
        hostname,
        target_ip[0], target_ip[1], target_ip[2], target_ip[3],
        MAX_HOPS
    );

    CTRL_C.store(false, Ordering::SeqCst);

    let icmp_h = match rx::register_icmp() {
        Some(h) => h,
        None => {
            crate::print_error!("traceroute: too many icmp waiters");
            return;
        }
    };

    'hops: for hop in 1..=MAX_HOPS {
        if CTRL_C.load(Ordering::SeqCst) {
            crate::println!("^C");
            break;
        }

        let gw_mac = match super::resolve_arp(target_ip, &our_ip, &our_mac) {
            Some(m) => m,
            None => {
                crate::print_error!("{:2}  * * * (arp failed)", hop);
                continue;
            }
        };

        let seq = hop as u16;
        let t_start_rtt = rdtsc();

        let mut ip_buf = [0u8; 200];
        let ip_len = build_icmp_with_ttl(
            &our_ip, target_ip, hop,
            PROBE_ID, seq, payload, &mut ip_buf,
        );
        if ip_len == 0 { continue; }

        let mut eth_buf = [0u8; 220];
        let eth_len = EthFrame::build(&gw_mac, &our_mac, ETHERTYPE_IP, &ip_buf[..ip_len], &mut eth_buf);
        if eth_len == 0 { continue; }

        {
            let mut state = super::NET.lock();
            if let Some(drv) = state.driver.as_mut() {
                drv.send(&eth_buf[..eth_len]);
            }
            state.tx_count += 1;
        }

        let mut got = false;
        let t_start_wait = crate::fs::procfs::uptime_ticks();

        loop {
            if CTRL_C.load(Ordering::SeqCst) {
                crate::println!("^C");
                break 'hops;
            }

            rx::pump();
            while let Some(ev) = rx::icmp_take(icmp_h) {
                let khz = crate::kcore::time::tsc_khz().max(1);
                let rtt_us = rdtsc().wrapping_sub(t_start_rtt) * 1000 / khz;
                let rtt_ms_i = rtt_us / 1000;
                let rtt_ms_f = (rtt_us % 1000) / 100;

                match ev.icmp_type {
                    11 => {
                        crate::cprintln!(230, 240, 240,
                            "{:2}  {}.{}.{}.{}  {}.{} ms",
                            hop,
                            ev.src_ip[0], ev.src_ip[1], ev.src_ip[2], ev.src_ip[3],
                            rtt_ms_i, rtt_ms_f,
                        );
                        got = true;
                    }
                    0 => {
                        if ev.id == PROBE_ID && ev.seq == seq {
                            crate::cprintln!(100, 220, 150,
                                "{:2}  {}.{}.{}.{}  {}.{} ms  [destination reached]",
                                hop,
                                target_ip[0], target_ip[1], target_ip[2], target_ip[3],
                                rtt_ms_i, rtt_ms_f,
                            );
                            got = true;
                            rx::unregister_icmp(icmp_h);
                            return;
                        }
                    }
                    _ => {}
                }

                if got { break; }
            }

            if got { break; }
            if crate::fs::procfs::uptime_ticks().wrapping_sub(t_start_wait) >= 2000 { break; }
            crate::sched::yield_now();
        }

        if !got {
            crate::cprintln!(120, 140, 140, "{:2}  * * * (timeout)", hop);
        }
    }

    rx::unregister_icmp(icmp_h);
}

pub fn cmd_traceroute(arg: &str) {
    if arg.is_empty() {
        crate::println!("Usage: traceroute <host>");
        return;
    }

    let ip = if let Some(ip) = parse_ip(arg) {
        ip
    } else {
        let dns = [8u8, 8, 8, 8];
        crate::print_info!("traceroute: resolving {}...", arg);
        match super::dns::resolve(arg, &dns) {
            Some(ip) => ip,
            None => {
                crate::print_error!("traceroute: cannot resolve {}", arg);
                return;
            }
        }
    };

    run(arg, &ip);
}

fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let mut p = s.split('.');
    Some([
        p.next()?.parse().ok()?,
        p.next()?.parse().ok()?,
        p.next()?.parse().ok()?,
        p.next()?.parse().ok()?,
    ])
}
