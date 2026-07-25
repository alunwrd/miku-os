use super::eth::{EthFrame, ETHERTYPE_IP};
use super::{ipv4, rx};
use super::udp;
use super::CTRL_C;
use core::sync::atomic::Ordering;

const NTP_DELTA: u64 = 70 * 365 * 24 * 3600 + 17 * 24 * 3600; 

const NTP_PORT: u16 = 123;
const NTP_SRC_PORT: u16 = 32123;

pub struct NtpResult {
    pub unix_secs: u64,
    pub frac_ns: u32,
    pub stratum: u8,
    pub version: u8,
}

impl NtpResult {
    pub fn format(&self) -> NtpFormatted {
        let mut s = self.unix_secs;
        let secs_in_day = s % 86400;
        s /= 86400;
        let hour = secs_in_day / 3600;
        let min  = (secs_in_day % 3600) / 60;
        let sec  = secs_in_day % 60;

        let mut year = 1970u32;
        let mut days = s as u32;
        loop {
            let days_in_year = if is_leap(year) { 366 } else { 365 };
            if days < days_in_year { break; }
            days -= days_in_year;
            year += 1;
        }
        let months = if is_leap(year) {
            &[31u32,29,31,30,31,30,31,31,30,31,30,31]
        } else {
            &[31u32,28,31,30,31,30,31,31,30,31,30,31]
        };
        let mut month = 1u32;
        for m in months {
            if days < *m { break; }
            days -= m;
            month += 1;
        }
        NtpFormatted {
            year, month, day: days + 1,
            hour: hour as u32, min: min as u32, sec: sec as u32,
            ms: self.frac_ns / 1_000_000,
        }
    }
}

pub struct NtpFormatted {
    pub year: u32, pub month: u32, pub day: u32,
    pub hour: u32, pub min: u32,   pub sec: u32,
    pub ms: u32,
}

fn is_leap(y: u32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn build_ntp_request(out: &mut [u8; 48]) {
    out.fill(0);
    out[0] = 0b00_100_011;
}

fn parse_ntp_response(buf: &[u8]) -> Option<NtpResult> {
    if buf.len() < 48 { return None; }

    let li_vn_mode = buf[0];
    let _li      = li_vn_mode >> 6;
    let version  = (li_vn_mode >> 3) & 0x07;
    let mode     = li_vn_mode & 0x07;
    let stratum  = buf[1];

    if mode != 4 && mode != 5 { return None; }
    if stratum == 0 { return None; }

    let ts_secs  = u32::from_be_bytes([buf[40], buf[41], buf[42], buf[43]]) as u64;
    let ts_frac  = u32::from_be_bytes([buf[44], buf[45], buf[46], buf[47]]);

    if ts_secs < NTP_DELTA { return None; }
    let unix_secs = ts_secs - NTP_DELTA;

    let frac_ns = ((ts_frac as u64 * 1_000_000_000) >> 32) as u32;

    Some(NtpResult { unix_secs, frac_ns, stratum, version })
}

pub fn sync(server_ip: &[u8; 4]) -> Option<NtpResult> {
    if !super::is_ready() { return None; }
    CTRL_C.store(false, Ordering::SeqCst);

    let our_ip  = super::get_ip();
    let our_mac = super::get_mac();

    let dst_mac = super::resolve_arp(server_ip, &our_ip, &our_mac)?;

    let mut ntp_pkt = [0u8; 48];
    build_ntp_request(&mut ntp_pkt);

    let mut udp_buf = [0u8; 80];
    let udp_len = udp::build(NTP_SRC_PORT, NTP_PORT, &ntp_pkt, &our_ip, server_ip, &mut udp_buf);
    if udp_len == 0 { return None; }

    let mut ip_buf = [0u8; 120];
    let ip_len = ipv4::build(&our_ip, server_ip, ipv4::PROTO_UDP, &udp_buf[..udp_len], &mut ip_buf);
    if ip_len == 0 { return None; }

    let mut eth_buf = [0u8; 150];
    let eth_len = EthFrame::build(&dst_mac, &our_mac, ETHERTYPE_IP, &ip_buf[..ip_len], &mut eth_buf);
    if eth_len == 0 { return None; }

    let udp_h = rx::register_udp(NTP_SRC_PORT)?;

    {
        let mut state = super::NET.lock();
        if let Some(drv) = state.driver.as_mut() {
            drv.send(&eth_buf[..eth_len]);
            state.tx_count += 1;
        }
    }

    crate::log!("ntp: request sent to {}.{}.{}.{}", server_ip[0], server_ip[1], server_ip[2], server_ip[3]);

    let start = crate::fs::procfs::uptime_ticks();
    loop {
        if CTRL_C.load(Ordering::SeqCst) { break; }
        if crate::fs::procfs::uptime_ticks().wrapping_sub(start) >= 1250 { break; } // 5 s

        rx::pump();
        while let Some(dgram) = rx::udp_take(udp_h) {
            if dgram.src_ip != *server_ip || dgram.src_port != NTP_PORT { continue; }
            if let Some(result) = parse_ntp_response(&dgram.data) {
                crate::log!("ntp: got time unix={} stratum={}", result.unix_secs, result.stratum);
                rx::unregister_udp(udp_h);
                return Some(result);
            }
        }

        crate::sched::yield_now();
    }

    rx::unregister_udp(udp_h);
    crate::log_err!("ntp: timeout");
    None
}

pub fn cmd_ntp(arg: &str) {
    let server_ip = if arg.is_empty() {
        [216, 239, 35, 0] 
    } else {
        match parse_ip(arg) {
            Some(ip) => ip,
            None => {
                crate::print_error!("ntp: bad ip: {}", arg);
                return;
            }
        }
    };

    crate::print_info!("ntp: syncing with {}.{}.{}.{}...",
        server_ip[0], server_ip[1], server_ip[2], server_ip[3]);

    match sync(&server_ip) {
        Some(r) => {
            let fmt = r.format();
            crate::print_success!(
                "ntp: {:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} UTC  stratum={}",
                fmt.year, fmt.month, fmt.day,
                fmt.hour, fmt.min, fmt.sec, fmt.ms,
                r.stratum
            );
            crate::fs::procfs::set_wall_clock(r.unix_secs);
        }
        None => crate::print_error!("ntp: failed to get time"),
    }
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

