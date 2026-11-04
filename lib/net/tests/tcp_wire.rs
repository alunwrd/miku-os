// TCP segments and sequence arithmetic. A comparison wrong at the wrap discards
// live data as an old duplicate

use mikunet::ipv4;
use mikunet::stack::tcp::{self, TcpSegment, FLAG_ACK, FLAG_FIN, FLAG_PSH, FLAG_SYN};

const SRC: [u8; 4] = [10, 0, 2, 15];
const DST: [u8; 4] = [93, 184, 216, 34];

// the sequence space

#[test]
fn ordering_holds_across_the_wrap() {
    // Two numbers 100 apart, straddling zero; plain `>` gets every one
    // backwards
    let before = u32::MAX - 50;
    let after = 49u32; // 100 further on, having wrapped

    assert!(tcp::seq_gt(after, before), "after the wrap still comes after");
    assert!(!tcp::seq_gt(before, after));
    assert!(tcp::seq_lt(before, after));
    assert!(
        after < before,
        "and plain comparison disagrees, which is the entire point"
    );
}

#[test]
fn a_number_is_not_after_itself_but_is_at_least_itself() {
    for n in [0u32, 1, u32::MAX, u32::MAX / 2, 1 << 31] {
        assert!(!tcp::seq_gt(n, n));
        assert!(tcp::seq_ge(n, n));
        assert!(tcp::seq_le(n, n));
        assert!(!tcp::seq_lt(n, n));
    }
}

#[test]
fn the_comparison_is_a_half_space() {
    // TCP orders numbers within 2^31 of each other; the boundary lands where
    // RFC 1982 says
    let base = 1_000_000u32;
    assert!(tcp::seq_gt(base.wrapping_add(1), base));
    assert!(tcp::seq_gt(base.wrapping_add((1 << 31) - 1), base));
    assert!(
        !tcp::seq_gt(base.wrapping_add(1 << 31), base),
        "exactly half the space away is not ordered, and must not claim to be"
    );
    assert!(tcp::seq_lt(base.wrapping_sub(1), base));
}

#[test]
fn every_pair_agrees_with_itself() {
    // The four predicates must be consistent: exactly one of gt, lt, equal,
    // with ge and le the unions
    let samples = [0u32, 1, 2, 1 << 30, (1 << 31) - 1, 1 << 31, u32::MAX - 1, u32::MAX];
    for &a in &samples {
        for &b in &samples {
            let gt = tcp::seq_gt(a, b);
            let lt = tcp::seq_lt(a, b);
            let eq = a == b;
            assert!(!(gt && lt), "{a} cannot be both before and after {b}");
            assert!(!(eq && (gt || lt)), "{a} == {b} but also ordered");
            assert_eq!(tcp::seq_ge(a, b), gt || eq, "ge disagrees for {a},{b}");
            assert_eq!(tcp::seq_le(a, b), lt || eq, "le disagrees for {a},{b}");
        }
    }
}

// segments

#[test]
fn a_segment_we_built_parses_back_and_checks_out() {
    let body = b"GET / HTTP/1.1\r\n";
    let mut out = [0u8; 128];
    let n = tcp::build(
        49152, 443, 0x1234_5678, 0x8765_4321,
        FLAG_ACK | FLAG_PSH, 64240, body, &SRC, &DST, &mut out,
    );
    assert_eq!(n, 20 + body.len());

    let s = TcpSegment::parse(&out[..n]).unwrap();
    assert_eq!(s.src_port, 49152);
    assert_eq!(s.dst_port, 443);
    assert_eq!(s.seq, 0x1234_5678);
    assert_eq!(s.ack, 0x8765_4321);
    assert_eq!(s.flags, FLAG_ACK | FLAG_PSH);
    assert_eq!(s.window, 64240);
    assert_eq!(s.data_offset, 20);
    assert_eq!(s.payload(&out[..n]), body);
    assert!(s.options(&out[..n]).is_empty());

    assert!(tcp::verify(&SRC, &DST, &out[..n]));
}

#[test]
fn the_checksum_is_bound_to_the_addresses_but_not_to_their_order() {
    let mut out = [0u8; 64];
    let n = tcp::build(1, 2, 0, 0, FLAG_ACK, 100, b"hi", &SRC, &DST, &mut out);
    assert!(tcp::verify(&SRC, &DST, &out[..n]));

    // A different address on either end fails, the pseudo-header's purpose
    assert!(!tcp::verify(&SRC, &[1, 2, 3, 4], &out[..n]));
    assert!(!tcp::verify(&[1, 2, 3, 4], &DST, &out[..n]));

    // Swapping the two passes: the checksum adds 16-bit words in any order.
    // Ports and the connection table tell inbound from outbound
    assert!(
        tcp::verify(&DST, &SRC, &out[..n]),
        "the sum is commutative; if this ever fails the arithmetic changed"
    );
}

#[test]
fn one_flipped_bit_anywhere_fails_the_segment_check() {
    let mut out = [0u8; 64];
    let n = tcp::build(80, 80, 7, 7, FLAG_ACK, 1, b"abcd", &SRC, &DST, &mut out);
    for byte in 0..n {
        for bit in 0..8 {
            let mut bad = out;
            bad[byte] ^= 1 << bit;
            assert!(
                !tcp::verify(&SRC, &DST, &bad[..n]),
                "a flip at byte {byte} bit {bit} went unnoticed"
            );
        }
    }
}

#[test]
fn a_header_longer_than_the_segment_is_refused() {
    // `data_offset` says where the payload starts; a 60-byte header claimed in
    // 20 bytes is rejected at the parse
    let mut out = [0u8; 64];
    let n = tcp::build(1, 2, 0, 0, FLAG_ACK, 0, b"", &SRC, &DST, &mut out);
    assert_eq!(n, 20);

    let mut lying = out;
    lying[12] = 0xF0; // 15 words = 60 bytes
    assert!(
        TcpSegment::parse(&lying[..n]).is_none(),
        "a 60-byte header inside a 20-byte segment"
    );

    // Below the minimum, the payload would be inside the header
    let mut tiny = out;
    tiny[12] = 0x40; // 4 words = 16 bytes
    assert!(TcpSegment::parse(&tiny[..n]).is_none());

    assert!(TcpSegment::parse(&out[..19]).is_none(), "shorter than a header");
}

#[test]
fn options_are_carried_and_found() {
    let opts = tcp::mss_opts();
    let mut out = [0u8; 64];
    let n = tcp::build_opts(
        1234, 80, 100, 0, FLAG_SYN, 64240, &opts, b"", &SRC, &DST, &mut out,
    );
    assert_eq!(n, 24, "twenty bytes of header and four of options");

    let s = TcpSegment::parse(&out[..n]).unwrap();
    assert_eq!(s.data_offset, 24);
    assert_eq!(s.options(&out[..n]), &opts);
    assert!(s.payload(&out[..n]).is_empty());
    assert_eq!(tcp::parse_mss(s.options(&out[..n])), Some(tcp::OUR_MSS));
    assert!(tcp::verify(&SRC, &DST, &out[..n]), "options are covered by the checksum");
}

#[test]
fn an_option_block_that_is_not_a_multiple_of_four_is_refused() {
    // The data offset counts 32-bit words, so a three-byte option block cannot
    // be described
    let mut out = [0u8; 64];
    assert_eq!(
        tcp::build_opts(1, 2, 0, 0, FLAG_SYN, 0, &[2, 4, 5], b"", &SRC, &DST, &mut out),
        0
    );
    assert_eq!(
        tcp::build_opts(1, 2, 0, 0, FLAG_SYN, 0, &[1u8; 44], b"", &SRC, &DST, &mut out),
        0,
        "more options than the header can describe"
    );
}

#[test]
fn walking_the_options_terminates_on_anything() {
    // A zero length byte would loop forever; a length past the block would read
    // other bytes
    assert_eq!(tcp::parse_mss(&[2, 0, 5, 0]), None, "zero length");
    assert_eq!(tcp::parse_mss(&[2, 40, 5, 0]), None, "length past the end");
    assert_eq!(tcp::parse_mss(&[2, 1, 5, 0]), None, "length below the minimum");
    assert_eq!(tcp::parse_mss(&[5]), None, "an option with no length byte");
    assert_eq!(tcp::parse_mss(&[]), None);

    // No-ops are skipped, end-of-list stops, and MSS is found wherever it is
    assert_eq!(tcp::parse_mss(&[1, 1, 2, 4, 0x05, 0xB4]), Some(1460));
    assert_eq!(tcp::parse_mss(&[4, 2, 2, 4, 0x05, 0xB4]), Some(1460), "after SACK-permitted");
    assert_eq!(tcp::parse_mss(&[0, 2, 4, 0x05, 0xB4]), None, "past the end of the list");
}

#[test]
fn the_handshake_flags_each_take_a_number_of_their_own() {
    // An empty SYN is acknowledged with seq+1
    let mut out = [0u8; 64];
    let n = tcp::build(1, 2, 0, 0, FLAG_SYN, 0, b"", &SRC, &DST, &mut out);
    let syn = TcpSegment::parse(&out[..n]).unwrap();
    assert_eq!(syn.seq_len(0), 1);

    let n = tcp::build(1, 2, 0, 0, FLAG_FIN | FLAG_ACK, 0, b"bye", &SRC, &DST, &mut out);
    let fin = TcpSegment::parse(&out[..n]).unwrap();
    assert_eq!(fin.seq_len(3), 4, "three bytes of data and the FIN");

    let n = tcp::build(1, 2, 0, 0, FLAG_ACK, 0, b"data", &SRC, &DST, &mut out);
    let ack = TcpSegment::parse(&out[..n]).unwrap();
    assert_eq!(ack.seq_len(4), 4, "a plain ACK occupies only its data");
    assert!(ack.has(FLAG_ACK));
    assert!(!ack.has(FLAG_SYN));
}

#[test]
fn a_short_buffer_builds_nothing() {
    let mut tiny = [0u8; 16];
    assert_eq!(tcp::build(1, 2, 0, 0, FLAG_ACK, 0, b"", &SRC, &DST, &mut tiny), 0);
}

#[test]
fn the_segment_checksum_is_the_l4_one() {
    // Same pseudo-header arithmetic as UDP, with another protocol number
    let mut out = [0u8; 64];
    let n = tcp::build(1, 2, 0, 0, FLAG_ACK, 0, b"x", &SRC, &DST, &mut out);
    let mut zeroed = out;
    zeroed[16] = 0;
    zeroed[17] = 0;
    assert_eq!(
        tcp::checksum(&SRC, &DST, &zeroed[..n]),
        ipv4::l4_checksum(&SRC, &DST, ipv4::PROTO_TCP, &zeroed[..n])
    );
}
