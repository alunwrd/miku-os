// SNTP over a datagram socket

use crate::sock::Dgram;

const NTP_DELTA: u64 = 70 * 365 * 24 * 3600 + 17 * 24 * 3600; 

const NTP_PORT: u16 = 123;

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
    miku_base::date::is_leap(y as i64)
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

/// Asks one server for the time: one request, one answer, five seconds, with no
/// averaging or drift correction
pub fn sync(server_ip: &[u8; 4]) -> Option<NtpResult> {
    crate::clear_interrupt();

    let mut ntp_pkt = [0u8; 48];
    build_ntp_request(&mut ntp_pkt);

    let mut sock = Dgram::new()?;
    if !sock.send_to(server_ip, NTP_PORT, &ntp_pkt) {
        netlog_err!("ntp: request not sent");
        return None;
    }
    netlog!("ntp: request sent to {}.{}.{}.{}", server_ip[0], server_ip[1], server_ip[2], server_ip[3]);

    const TIMEOUT_MS: u64 = 5_000;
    let start = crate::plat::uptime_ms();
    loop {
        if crate::interrupted() { break; }
        let spent = crate::plat::uptime_ms().wrapping_sub(start);
        if spent >= TIMEOUT_MS { break; }

        let mut buf = [0u8; 256];
        let (n, src_ip, src_port) = match sock.recv_from(&mut buf, TIMEOUT_MS - spent) {
            Some(v) => v,
            None => continue,
        };
        // An unconnected socket accepts datagrams from anyone, and this one
        // sets the clock
        if src_ip != *server_ip || src_port != NTP_PORT { continue; }
        if let Some(result) = parse_ntp_response(&buf[..n]) {
            netlog!("ntp: got time unix={} stratum={}", result.unix_secs, result.stratum);
            return Some(result);
        }
    }

    netlog_err!("ntp: timeout");
    None
}


