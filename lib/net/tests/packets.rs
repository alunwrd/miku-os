// Ethernet, IPv4, and UDP on the host, especially malformed input

use mikunet::ipv4;
use mikunet::stack::eth::{self, EthFrame};
use mikunet::stack::udp::{self, UdpHeader};

const ME: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
const THEM: [u8; 6] = [0x52, 0x54, 0x00, 0xAB, 0xCD, 0xEF];
const SRC: [u8; 4] = [10, 0, 2, 15];
const DST: [u8; 4] = [10, 0, 2, 3];

// the checksum itself

#[test]
fn the_rfc_1071_worked_example_comes_out_right() {
    // RFC 1071 section 3: the octets 00 01 f2 03 f4 f5 f6 f7 sum to 0xddf2 and
    // check as 0x220d
    let data = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
    assert_eq!(ipv4::sum16(&[&data]), 0xddf2);
    assert_eq!(ipv4::checksum(&data), 0x220d);
}

#[test]
fn an_odd_length_pads_on_the_right() {
    // The last byte is the high half of a 16-bit word
    assert_eq!(ipv4::sum16(&[&[0x12u8][..]]), 0x1200);
    assert_eq!(ipv4::sum16(&[&[0x12u8, 0x00][..]]), 0x1200);
}

#[test]
fn split_buffers_sum_the_same_as_one() {
    let whole = [1u8, 2, 3, 4, 5, 6, 7, 8];
    assert_eq!(
        ipv4::sum16(&[&whole]),
        ipv4::sum16(&[&whole[..2], &whole[2..5], &whole[5..]]),
        "the pseudo-header is summed as a separate chunk, so this had better hold"
    );
}

#[test]
fn a_computed_zero_is_sent_as_all_ones() {
    // UDP reserves 0 for "no checksum computed", so a real zero goes out as
    // 0xFFFF; they are the same value in one's complement. Searched, since the
    // input depends on the pseudo-header's addresses
    let mut hit = None;
    for n in 0u32..=0xFFFF {
        let body = [(n >> 8) as u8, n as u8, 0, 0];
        let c = ipv4::l4_checksum(&SRC, &DST, ipv4::PROTO_UDP, &body);
        assert_ne!(c, 0, "l4_checksum must never return a bare zero");
        if c == 0xFFFF {
            hit = Some(body);
        }
    }
    let mut body = hit.expect("no input in 16 bits folded to zero - the search is wrong");

    // A segment carrying that checksum still verifies
    body[2] = 0xFF;
    body[3] = 0xFF;
    assert!(ipv4::verify_l4(&SRC, &DST, ipv4::PROTO_UDP, &body));
}

// IPv4 headers

#[test]
fn a_header_we_built_verifies() {
    let mut out = [0u8; 64];
    let n = ipv4::build(&SRC, &DST, ipv4::PROTO_UDP, b"hello", &mut out);
    assert_eq!(n, 25);
    assert!(ipv4::verify_header(&out[..20]));

    let h = ipv4::Ipv4Header::parse(&out[..n]).expect("our own header must parse");
    assert_eq!(h.src, SRC);
    assert_eq!(h.dst, DST);
    assert_eq!(h.proto, ipv4::PROTO_UDP);
    assert_eq!(h.ttl, ipv4::DEFAULT_TTL);
    assert_eq!(h.ihl, 20);
    assert_eq!(h.total_len, 25);
    assert!(!h.fragmented);
    assert_eq!(h.payload(&out[..n]), b"hello");
}

#[test]
fn one_flipped_bit_anywhere_fails_the_header_check() {
    let mut out = [0u8; 64];
    ipv4::build(&SRC, &DST, ipv4::PROTO_TCP, b"x", &mut out);
    for byte in 0..20 {
        for bit in 0..8 {
            let mut bad = out;
            bad[byte] ^= 1 << bit;
            assert!(
                !ipv4::verify_header(&bad[..20]),
                "a flip at byte {byte} bit {bit} went unnoticed"
            );
        }
    }
}

#[test]
fn the_hop_limit_is_the_senders_to_choose() {
    // Probes with a rising TTL, for traceroute
    let mut out = [0u8; 32];
    ipv4::build_ttl(&SRC, &DST, ipv4::PROTO_ICMP, b"", 1, &mut out);
    let h = ipv4::Ipv4Header::parse(&out).unwrap();
    assert_eq!(h.ttl, 1);
    assert!(ipv4::verify_header(&out[..20]), "the checksum must cover the TTL we set");
}

#[test]
fn a_malformed_header_is_refused_rather_than_believed() {
    let mut good = [0u8; 40];
    ipv4::build(&SRC, &DST, ipv4::PROTO_UDP, b"body", &mut good);

    assert!(ipv4::Ipv4Header::parse(&good[..19]).is_none(), "shorter than a header");

    let mut v6 = good;
    v6[0] = 0x65; // version 6, ihl 5
    assert!(ipv4::Ipv4Header::parse(&v6).is_none(), "not version 4");

    let mut tiny_ihl = good;
    tiny_ihl[0] = 0x44; // ihl 4 words = 16 bytes, below the 20-byte minimum
    assert!(ipv4::Ipv4Header::parse(&tiny_ihl).is_none(), "ihl below the minimum");

    let mut huge_ihl = good;
    huge_ihl[0] = 0x4F; // ihl 15 words = 60 bytes, past the end of this buffer
    assert!(
        ipv4::Ipv4Header::parse(&huge_ihl[..40]).is_none(),
        "options that run off the end of the packet"
    );
}

#[test]
fn a_length_the_sender_lied_about_cannot_read_past_the_buffer() {
    // `total_len` is the sender's choice; the buffer is what arrived
    let mut pkt = [0u8; 30];
    ipv4::build(&SRC, &DST, ipv4::PROTO_UDP, b"realbody", &mut pkt);
    pkt[2] = 0xFF;
    pkt[3] = 0xFF; // claims 65535 bytes
    let h = ipv4::Ipv4Header::parse(&pkt).unwrap();
    assert_eq!(h.total_len, 0xFFFF);
    assert_eq!(h.payload(&pkt).len(), 10, "payload must stop at what arrived");

    // A total length shorter than the header
    pkt[2] = 0;
    pkt[3] = 4;
    let h = ipv4::Ipv4Header::parse(&pkt).unwrap();
    assert_eq!(h.payload(&pkt), b"", "a length below the header yields nothing, not a panic");
}

#[test]
fn both_kinds_of_fragment_are_seen() {
    let mut pkt = [0u8; 40];
    ipv4::build(&SRC, &DST, ipv4::PROTO_UDP, b"frag", &mut pkt);
    assert!(!ipv4::Ipv4Header::parse(&pkt).unwrap().fragmented);

    // More-fragments set, offset zero: the first piece
    let mut mf = pkt;
    mf[6] = 0x20;
    assert!(ipv4::Ipv4Header::parse(&mf).unwrap().fragmented, "MF bit");

    // Offset non-zero, more-fragments clear: the last piece
    let mut off = pkt;
    off[6] = 0x00;
    off[7] = 0x10;
    assert!(ipv4::Ipv4Header::parse(&off).unwrap().fragmented, "non-zero offset");

    // The don't-fragment bit is not a fragment, and most packets set it
    let mut df = pkt;
    df[6] = 0x40;
    assert!(!ipv4::Ipv4Header::parse(&df).unwrap().fragmented, "DF is not MF");
}

// UDP

#[test]
fn a_datagram_we_built_parses_and_checks_out() {
    let body = b"the quick brown fox";
    let mut out = [0u8; 64];
    let n = udp::build(4321, 53, body, &SRC, &DST, &mut out);
    assert_eq!(n, 8 + body.len());

    let h = UdpHeader::parse(&out[..n]).unwrap();
    assert_eq!(h.src_port, 4321);
    assert_eq!(h.dst_port, 53);
    assert_eq!(h.length as usize, n);
    assert_eq!(h.payload(&out[..n]), body);

    assert!(ipv4::verify_l4(&SRC, &DST, ipv4::PROTO_UDP, &out[..n]));
}

#[test]
fn the_udp_checksum_covers_the_addresses_it_was_not_sent_with() {
    // A datagram is valid only for the addresses in its pseudo-header
    let mut out = [0u8; 32];
    let n = udp::build(1, 2, b"abc", &SRC, &DST, &mut out);
    assert!(ipv4::verify_l4(&SRC, &DST, ipv4::PROTO_UDP, &out[..n]));
    assert!(
        !ipv4::verify_l4(&[10, 0, 2, 99], &DST, ipv4::PROTO_UDP, &out[..n]),
        "a different source must not verify"
    );
    assert!(
        !ipv4::verify_l4(&SRC, &[10, 0, 2, 99], ipv4::PROTO_UDP, &out[..n]),
        "a different destination must not verify"
    );
}

#[test]
fn a_udp_length_the_sender_lied_about_is_clamped() {
    let mut out = [0u8; 32];
    let n = udp::build(7, 7, b"four", &SRC, &DST, &mut out);
    out[4] = 0xFF;
    out[5] = 0xFF;
    let h = UdpHeader::parse(&out[..n]).unwrap();
    assert_eq!(h.payload(&out[..n]).len(), 4, "clamped to what arrived");

    // A length below the header
    out[4] = 0;
    out[5] = 2;
    let h = UdpHeader::parse(&out[..n]).unwrap();
    assert_eq!(h.payload(&out[..n]), b"");
}

#[test]
fn a_short_buffer_builds_nothing_rather_than_something_partial() {
    let mut tiny = [0u8; 4];
    assert_eq!(udp::build(1, 2, b"toolong", &SRC, &DST, &mut tiny), 0);
    assert_eq!(ipv4::build(&SRC, &DST, ipv4::PROTO_UDP, b"toolong", &mut tiny), 0);
    assert_eq!(EthFrame::build(&THEM, &ME, eth::ETHERTYPE_IP, b"toolong", &mut tiny), 0);
    assert!(UdpHeader::parse(&[0u8; 7]).is_none());
}

// ethernet

#[test]
fn a_frame_survives_the_round_trip() {
    let mut out = [0u8; 64];
    let n = EthFrame::build(&THEM, &ME, eth::ETHERTYPE_IP, b"payload", &mut out);
    assert_eq!(n, 14 + 7);

    let f = EthFrame::parse(&out[..n]).unwrap();
    assert_eq!(f.dst, THEM);
    assert_eq!(f.src, ME);
    assert_eq!(f.ethertype, eth::ETHERTYPE_IP);
    assert_eq!(f.payload, b"payload");
}

#[test]
fn a_frame_shorter_than_its_header_does_not_parse() {
    assert!(EthFrame::parse(&[0u8; 13]).is_none());
    // A header with no body is a real frame with an empty payload
    assert_eq!(EthFrame::parse(&[0u8; 14]).unwrap().payload.len(), 0);
}

#[test]
fn only_frames_meant_for_us_count_as_ours() {
    assert!(eth::addressed_to(&ME, &ME), "our own address");
    assert!(eth::addressed_to(&eth::BROADCAST_MAC, &ME), "broadcast");
    // Low bit of the first octet is the group bit
    assert!(eth::addressed_to(&[0x01, 0x00, 0x5e, 0, 0, 1], &ME), "multicast");
    assert!(
        !eth::addressed_to(&THEM, &ME),
        "somebody else's frame - a mirrored port or a promiscuous card hands \
         these up, and treating them as ours puts a stranger's TCP segment in \
         our demux"
    );
}
