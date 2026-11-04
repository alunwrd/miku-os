// Asking the network for an address
//
// The packets are `mikunet::stack::dhcp`, where `cargo test` can hand the
// parser replies nobody would send on purpose: a length running off the buffer,
// options after the end marker, an OFFER pretending to be an ACK, another
// machine's transaction. All of that arrives before the machine has an address
// or anything is authenticated.
//
// What is left here is the conversation: four messages, retries, a timeout, and
// a socket to hold the client port.

use mikunet::stack::dhcp;
use mikunet::stack::eth::BROADCAST_MAC;

pub use mikunet::stack::dhcp::Lease as DhcpResult;

/// How long one message waits for its answer (ticks, 4 ms each)
const REPLY_TIMEOUT_TICKS: u64 = 1000; // 4 s
/// Attempts per message. On real hardware the PHY may have only just finished
/// auto-negotiating, and the first frame can be lost
const ATTEMPTS: u32 = 4;

pub fn do_dhcp() -> Option<DhcpResult> {
    // Claims the client port for the whole exchange; replies are routed to this
    // queue by the RX demux
    let udp_h = match super::rx::register_udp(dhcp::CLIENT_PORT) {
        Some(h) => h,
        None => {
            crate::serial_println!("[dhcp] udp port 68 busy");
            return None;
        }
    };
    let r = exchange(udp_h);
    super::rx::unregister_udp(udp_h);
    r
}

fn exchange(udp_h: usize) -> Option<DhcpResult> {
    let mac = super::get_mac();

    // A fresh transaction id per exchange, so two machines on a segment do not
    // accept each other's OFFER and ACK. The MAC is mixed in so they cannot
    // collide on the same draw
    let xid: u32 = (crate::kcore::random::random_u64() as u32)
        ^ u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]]);

    let zero_ip = [0u8; 4];
    let bcast_ip = [255u8; 4];

    crate::serial_println!(
        "[dhcp] start xid={:#x} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        xid, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    let mut pkt = [0u8; dhcp::PACKET_BUF];
    let len = dhcp::build_packet(dhcp::DISCOVER, xid, &mac, &zero_ip, None, None, &mut pkt);

    let mut offer = None;
    for attempt in 1..=ATTEMPTS {
        crate::serial_println!("[dhcp] DISCOVER {}/{}", attempt, ATTEMPTS);
        if let Some(o) = send_and_wait(udp_h, &pkt[..len], &mac, &bcast_ip, xid, dhcp::OFFER) {
            offer = Some(o);
            break;
        }
        crate::serial_println!("[dhcp] discover {}/{} timeout", attempt, ATTEMPTS);
    }
    let offer = offer?;

    crate::serial_println!(
        "[dhcp] offer ip={}.{}.{}.{} server={}.{}.{}.{}",
        offer.ip[0], offer.ip[1], offer.ip[2], offer.ip[3],
        offer.server[0], offer.server[1], offer.server[2], offer.server[3]
    );

    let len = dhcp::build_packet(
        dhcp::REQUEST, xid, &mac, &zero_ip,
        Some(&offer.server), Some(&offer.ip), &mut pkt,
    );

    let mut ack = None;
    for attempt in 1..=ATTEMPTS {
        crate::serial_println!("[dhcp] request {}/{}", attempt, ATTEMPTS);
        if let Some(a) = send_and_wait(udp_h, &pkt[..len], &mac, &bcast_ip, xid, dhcp::ACK) {
            ack = Some(a);
            break;
        }
    }
    let ack = ack?;

    if ack.nak {
        // Refused: the offered address is not ours after all. Reports failure
        // so the caller starts a new transaction
        crate::serial_println!("[dhcp] REQUEST was refused");
        return None;
    }
    crate::serial_println!(
        "[dhcp] ACK ip={}.{}.{}.{}", ack.ip[0], ack.ip[1], ack.ip[2], ack.ip[3]
    );
    Some(ack)
}

fn send_and_wait(
    udp_h: usize,
    payload: &[u8],
    mac: &[u8; 6],
    dst_ip: &[u8; 4],
    xid: u32,
    want: u8,
) -> Option<DhcpResult> {
    let mut frame = [0u8; 700];
    // Source 0.0.0.0: there is no address yet. Broadcast at both layers,
    // because nothing on the segment knows this machine yet
    let n = dhcp::frame(payload, mac, &[0; 4], dst_ip, &BROADCAST_MAC, &mut frame);
    if n == 0 {
        return None;
    }
    if !super::tx_frame(&frame[..n]) {
        crate::serial_println!("[dhcp] driver send returned false (TX queue busy?)");
    }

    let start = crate::fs::procfs::uptime_ticks();
    x86_64::instructions::interrupts::enable();
    loop {
        if crate::fs::procfs::uptime_ticks().wrapping_sub(start) > REPLY_TIMEOUT_TICKS {
            return None;
        }
        super::rx::pump();
        while let Some(dgram) = super::rx::udp_take(udp_h) {
            if let Some(r) = dhcp::parse_reply(&dgram.data, xid, mac, want) {
                return Some(r);
            }
        }
        crate::sched::sleep(1); // a tick, not a spin: see sched::wait
    }
}
