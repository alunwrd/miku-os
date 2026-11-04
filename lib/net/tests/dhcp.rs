// DHCP replies, including malformed ones. A machine reads these before it has
// an address or anything authenticated

use mikunet::stack::dhcp::{self, Lease, PACKET_BUF};

const MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
const OTHER_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0xAA, 0xBB, 0xCC];
const XID: u32 = 0xDEAD_BEEF;

/// A server reply, assembled the way a server would
fn reply(msg_type: u8, xid: u32, mac: &[u8; 6], opts: &[(u8, &[u8])]) -> Vec<u8> {
    let mut buf = vec![0u8; 240];
    buf[0] = 2; // BOOTREPLY
    buf[1] = 1;
    buf[2] = 6;
    buf[4..8].copy_from_slice(&xid.to_be_bytes());
    buf[16..20].copy_from_slice(&[10, 0, 2, 15]); // yiaddr
    buf[28..34].copy_from_slice(mac);
    buf[236..240].copy_from_slice(&[99, 130, 83, 99]);

    buf.extend_from_slice(&[53, 1, msg_type]);
    for (kind, data) in opts {
        buf.push(*kind);
        buf.push(data.len() as u8);
        buf.extend_from_slice(data);
    }
    buf.push(255);
    buf
}

fn full_offer() -> Vec<u8> {
    reply(
        dhcp::OFFER,
        XID,
        &MAC,
        &[
            (1, &[255, 255, 255, 0]),
            (3, &[10, 0, 2, 2]),
            (6, &[10, 0, 2, 3]),
            (54, &[10, 0, 2, 2]),
            (51, &[0, 0, 14, 16]), // 3600 s
        ],
    )
}

// what a good exchange looks like

#[test]
fn an_offer_yields_every_field_the_machine_needs() {
    let l = dhcp::parse_reply(&full_offer(), XID, &MAC, dhcp::OFFER).expect("a valid offer");
    assert_eq!(
        l,
        Lease {
            ip: [10, 0, 2, 15],
            mask: [255, 255, 255, 0],
            gw: [10, 0, 2, 2],
            dns: [10, 0, 2, 3],
            server: [10, 0, 2, 2],
            lease_secs: 3600,
            nak: false,
        }
    );
}

#[test]
fn a_packet_we_built_has_the_shape_a_server_expects() {
    let mut out = [0u8; PACKET_BUF];
    let n = dhcp::build_packet(dhcp::DISCOVER, XID, &MAC, &[0; 4], None, None, &mut out);
    assert_eq!(n, 300, "padded to the legacy BOOTP minimum");

    assert_eq!(out[0], 1, "BOOTREQUEST");
    assert_eq!(out[1..4], [1, 6, 0]);
    assert_eq!(u32::from_be_bytes([out[4], out[5], out[6], out[7]]), XID);
    assert_eq!(out[10] & 0x80, 0x80, "the broadcast bit, or the reply is unicast to nobody");
    assert_eq!(out[28..34], MAC);
    assert_eq!(out[236..240], [99, 130, 83, 99], "the magic cookie");
    assert_eq!(out[240..243], [53, 1, dhcp::DISCOVER]);
}

#[test]
fn a_request_names_the_server_and_the_address_it_offered() {
    // RFC 2131 4.3.2: without both, another server does not know its offer was
    // declined and holds the address
    let mut out = [0u8; PACKET_BUF];
    let srv = [10, 0, 2, 2];
    let want = [10, 0, 2, 15];
    dhcp::build_packet(dhcp::REQUEST, XID, &MAC, &[0; 4], Some(&srv), Some(&want), &mut out);

    let opts = &out[243..300];
    assert!(
        opts.windows(6).any(|w| w[0] == 54 && w[1] == 4 && w[2..6] == srv),
        "option 54, the server identifier"
    );
    assert!(
        opts.windows(6).any(|w| w[0] == 50 && w[1] == 4 && w[2..6] == want),
        "option 50, the requested address"
    );
}

// Replies that are not ours

#[test]
fn a_reply_to_another_machines_transaction_is_refused() {
    // A wrong transaction id: another client's OFFER or ACK
    let pkt = reply(dhcp::OFFER, 0x1234_5678, &MAC, &[]);
    assert!(dhcp::parse_reply(&pkt, XID, &MAC, dhcp::OFFER).is_none());
}

#[test]
fn a_reply_addressed_to_another_card_is_refused() {
    let pkt = reply(dhcp::OFFER, XID, &OTHER_MAC, &[]);
    assert!(dhcp::parse_reply(&pkt, XID, &MAC, dhcp::OFFER).is_none());
}

#[test]
fn an_offer_is_not_an_acknowledgement() {
    // REQUEST must not accept an OFFER as the granted lease
    let pkt = full_offer();
    assert!(dhcp::parse_reply(&pkt, XID, &MAC, dhcp::OFFER).is_some());
    assert!(
        dhcp::parse_reply(&pkt, XID, &MAC, dhcp::ACK).is_none(),
        "an OFFER must not satisfy a wait for an ACK"
    );
}

#[test]
fn a_client_broadcast_coming_back_to_us_is_not_a_reply() {
    let mut pkt = full_offer();
    pkt[0] = 1; // BOOTREQUEST
    assert!(dhcp::parse_reply(&pkt, XID, &MAC, dhcp::OFFER).is_none());
}

#[test]
fn a_refusal_is_reported_rather_than_waited_out() {
    // A NAK is an answer, not silence, so the client need not wait out the
    // timeout
    let pkt = reply(dhcp::NAK, XID, &MAC, &[(54, &[10, 0, 2, 2])]);
    let l = dhcp::parse_reply(&pkt, XID, &MAC, dhcp::ACK).expect("a NAK is an answer");
    assert!(l.nak);
    assert_eq!(l.ip, [0, 0, 0, 0], "and carries no address to be used by mistake");
}

// replies that are malformed

#[test]
fn walking_the_options_terminates_on_anything() {
    let base = full_offer();

    // A length running past the end of the packet
    let mut lying = base.clone();
    let at = lying.len() - 4;
    lying[at] = 6;
    lying[at + 1] = 200;
    assert!(dhcp::parse_reply(&lying, XID, &MAC, dhcp::OFFER).is_some(), "stops, does not panic");

    // An option with a kind byte and nothing after it
    let mut truncated = base.clone();
    truncated.truncate(241);
    truncated[240] = 53;
    assert!(dhcp::parse_reply(&truncated, XID, &MAC, dhcp::OFFER).is_none());

    // Every prefix of a valid packet
    for cut in 0..base.len() {
        let _ = dhcp::parse_reply(&base[..cut], XID, &MAC, dhcp::OFFER);
    }
}

#[test]
fn a_zero_length_option_does_not_stall_the_walk() {
    // Option 0 is pad and advances by one; every other option advances by at
    // least two
    let mut pkt = reply(dhcp::OFFER, XID, &MAC, &[(1, &[255, 255, 255, 0])]);
    let at = pkt.len() - 1;
    pkt.splice(at..at, [0u8, 0, 0, 0, 12, 0]); // four pads then a zero-length option 12
    let l = dhcp::parse_reply(&pkt, XID, &MAC, dhcp::OFFER).expect("the walk got past them");
    assert_eq!(l.mask, [255, 255, 255, 0]);
}

#[test]
fn options_after_the_end_marker_are_not_read() {
    // Anything after the end of the option list is ignored
    let mut pkt = reply(dhcp::OFFER, XID, &MAC, &[(3, &[10, 0, 2, 2])]);
    pkt.extend_from_slice(&[3, 4, 66, 66, 66, 66]); // a second router option
    let l = dhcp::parse_reply(&pkt, XID, &MAC, dhcp::OFFER).unwrap();
    assert_eq!(l.gw, [10, 0, 2, 2], "the one before the end marker wins");
}

#[test]
fn a_reply_without_the_magic_cookie_is_not_dhcp() {
    let mut pkt = full_offer();
    pkt[238] = 0;
    assert!(dhcp::parse_reply(&pkt, XID, &MAC, dhcp::OFFER).is_none());
}

#[test]
fn a_reply_with_no_options_at_all_is_not_the_message_we_wanted() {
    let mut pkt = vec![0u8; 240];
    pkt[0] = 2;
    pkt[4..8].copy_from_slice(&XID.to_be_bytes());
    pkt[28..34].copy_from_slice(&MAC);
    pkt[236..240].copy_from_slice(&[99, 130, 83, 99]);
    assert!(
        dhcp::parse_reply(&pkt, XID, &MAC, dhcp::OFFER).is_none(),
        "no message type means no message"
    );
}

// the frame it goes out in

#[test]
fn a_discover_leaves_broadcast_from_an_address_we_do_not_have_yet() {
    let mut pkt = [0u8; PACKET_BUF];
    let n = dhcp::build_packet(dhcp::DISCOVER, XID, &MAC, &[0; 4], None, None, &mut pkt);

    let mut out = [0u8; 700];
    let len = dhcp::frame(&pkt[..n], &MAC, &[0; 4], &[255; 4], &[0xFF; 6], &mut out);
    assert!(len > 0);

    let f = mikunet::stack::eth::EthFrame::parse(&out[..len]).unwrap();
    assert_eq!(f.dst, [0xFF; 6], "nobody knows who the server is yet");
    assert_eq!(f.src, MAC);

    let ip = mikunet::ipv4::Ipv4Header::parse(f.payload).unwrap();
    assert_eq!(ip.src, [0, 0, 0, 0], "we have no address to claim");
    assert_eq!(ip.dst, [255, 255, 255, 255]);
    assert!(mikunet::ipv4::verify_header(&f.payload[..20]));

    let udp_bytes = ip.payload(f.payload);
    let udp = mikunet::stack::udp::UdpHeader::parse(udp_bytes).unwrap();
    assert_eq!(udp.src_port, dhcp::CLIENT_PORT);
    assert_eq!(udp.dst_port, dhcp::SERVER_PORT);
    assert_eq!(udp.payload(udp_bytes).len(), n);
}
