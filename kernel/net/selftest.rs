// Network selftest: what needs the machine
//
// Pure functions of bytes live in `mikunet` and are checked by
// `cd lib/net && cargo test`: checksums, fragment detection, sequence
// wraparound, ethernet framing, and a DHCP parser fed replies nobody would send
// on purpose.
//
// This is what a host build cannot answer: is there a card, does the link carry
// a frame, does a real server answer a DISCOVER, does the gateway answer ARP,
// does anything come back from a ping.

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::kcore::selftest::{Report, Runner};
use super::dhcp;

pub fn run() -> Report {
    let mut t = Runner::new();

    if !super::is_ready() {
        t.skip("net/live", String::from("skipped: no network adapter on this machine"));
        return t.report();
    }

    check_dhcp_lease(&mut t);
    check_arp_gateway(&mut t);
    check_ping(&mut t);
    check_drop_counters(&mut t);

    t.report()
}

fn check_dhcp_lease(t: &mut Runner) {
    let ip = super::get_ip();
    if ip != [0, 0, 0, 0] {
        t.check("net/dhcp-lease", true,
            format!("already leased {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]));
        return;
    }
    match dhcp::do_dhcp() {
        Some(r) => {
            super::set_ip(r.ip, r.gw, r.mask);
            super::set_dns(r.dns);
            let ok = r.ip != [0, 0, 0, 0] && r.gw != [0, 0, 0, 0];
            t.check("net/dhcp-lease", ok,
                format!("ip={}.{}.{}.{} gw={}.{}.{}.{} lease={}s",
                    r.ip[0], r.ip[1], r.ip[2], r.ip[3],
                    r.gw[0], r.gw[1], r.gw[2], r.gw[3], r.lease_secs));
        }
        None => t.check("net/dhcp-lease", false, String::from("no reply to DISCOVER")),
    }
}

fn check_arp_gateway(t: &mut Runner) {
    let (ip, gw) = { let s = super::NET.lock(); (s.ip, s.gw) };
    if gw == [0, 0, 0, 0] {
        t.skip("net/arp-gateway", String::from("skipped: no gateway configured"));
        return;
    }
    match super::resolve_arp(&gw, &ip, &super::get_mac()) {
        Some(m) => t.check("net/arp-gateway", true,
            format!("{}.{}.{}.{} is {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                gw[0], gw[1], gw[2], gw[3], m[0], m[1], m[2], m[3], m[4], m[5])),
        None => t.check("net/arp-gateway", false, String::from("gateway did not answer ARP")),
    }
}

fn check_ping(t: &mut Runner) {
    let (ip, gw) = { let s = super::NET.lock(); (s.ip, s.gw) };
    if gw == [0, 0, 0, 0] {
        t.skip("net/ping", String::from("skipped: no gateway to ping"));
        return;
    }
    let s = super::ping_probe(&gw, &ip, 4, false);
    let (sent, got) = (s.sent, s.received);
    t.check("net/ping", sent > 0 && got == sent,
        format!("{}/{} replies from the gateway, rtt min/avg/max {}/{}/{} us",
            got, sent, s.rtt_min_us, s.rtt_avg_us, s.rtt_max_us));
}

fn check_drop_counters(t: &mut Runner) {
    let s = super::NET.lock();
    let (c, f, a) = (s.drop_csum, s.drop_frag, s.drop_addr);
    // Frames addressed elsewhere are normal on a shared segment; a bad
    // checksum or a fragment we cannot reassemble is worth knowing about
    t.check("net/drop-counters", c == 0 && f == 0,
        format!("checksum={} fragment={} not-for-us={}", c, f, a));
}

fn assert_ne(t: &mut Runner, ok: bool) {
    if !ok {
        t.check("selftest/setup", false, String::from("a fixture failed to build"));
    }
}
