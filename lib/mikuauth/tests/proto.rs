// Wire format against hostile messages. The daemon reads these bytes as root
// from a socket open to all users

use mikuauth::proto::{frame_len, Op, Reader, Reply, Writer, MAX_MESSAGE, MAX_STR};

#[test]
fn a_message_survives_a_round_trip() {
    let mut buf = [0u8; 4096];
    let framed = {
        let mut w = Writer::new(&mut buf);
        w.u8(1).u8(Op::Run as u8).u16(3).str("root").str("/bin/id").u16(2).str("-u").str("-n");
        w.finish().expect("should fit")
    };
    let n = frame_len([framed[0], framed[1], framed[2], framed[3]]).unwrap();
    assert_eq!(n, framed.len() - 4);

    let mut r = Reader::new(&framed[4..]);
    assert_eq!(r.u8(), Some(1));
    assert_eq!(Op::from_u8(r.u8().unwrap()), Some(Op::Run));
    assert_eq!(r.u16(), Some(3));
    assert_eq!(r.str(), Some("root"));
    assert_eq!(r.str(), Some("/bin/id"));
    assert_eq!(r.u16(), Some(2));
    assert_eq!(r.str(), Some("-u"));
    assert_eq!(r.str(), Some("-n"));
    assert_eq!(r.remaining(), 0);
}

#[test]
fn a_reader_never_walks_off_the_end() {
    // Every truncated prefix must read as None
    let full = {
        let mut buf = [0u8; 256];
        let mut w = Writer::new(&mut buf);
        w.u8(1).str("root").u32(7);
        w.finish().unwrap().to_vec()
    };
    for cut in 0..full.len() - 4 {
        let body = &full[4..4 + cut];
        let mut r = Reader::new(body);
        // Must not panic or claim more than it has
        let _ = r.u8();
        let _ = r.str();
        let _ = r.u32();
        assert!(r.remaining() <= body.len());
    }
}

#[test]
fn a_string_longer_than_the_message_is_refused() {
    // A length field longer than the body: 0xFFFF over four bytes
    let body = [0xffu8, 0xff, b'h', b'i'];
    let mut r = Reader::new(&body);
    assert_eq!(r.bytes(), None);

    // And one that is within the message but past the per-string cap
    let mut over = vec![0u8; 2 + MAX_STR + 1];
    over[..2].copy_from_slice(&((MAX_STR + 1) as u16).to_le_bytes());
    let mut r = Reader::new(&over);
    assert_eq!(r.bytes(), None, "a string past MAX_STR must be refused");
}

#[test]
fn a_frame_length_that_is_absurd_is_rejected_before_any_buffer_is_touched() {
    assert_eq!(frame_len([0, 0, 0, 0]), None, "an empty message");
    assert_eq!(frame_len(0xffff_ffffu32.to_le_bytes()), None, "four gigabytes");
    assert_eq!(frame_len(((MAX_MESSAGE + 1) as u32).to_le_bytes()), None);
    assert_eq!(frame_len((MAX_MESSAGE as u32).to_le_bytes()), Some(MAX_MESSAGE));
}

#[test]
fn a_string_that_is_not_utf8_is_refused_rather_than_repaired() {
    // A lossy conversion would exec a path other than the one checked against
    // the rule
    let mut body = vec![3u8, 0];
    body.extend_from_slice(&[0xff, 0xfe, 0x41]);
    let mut r = Reader::new(&body);
    assert_eq!(r.str(), None);

    // Any bytes are valid as bytes, for passwords
    let mut r = Reader::new(&body);
    assert_eq!(r.bytes(), Some(&[0xffu8, 0xfe, 0x41][..]));
}

#[test]
fn a_writer_refuses_rather_than_truncating() {
    // A message that does not fit is not sent; a truncated argv would be a
    // different request
    let mut small = [0u8; 16];
    let mut w = Writer::new(&mut small);
    w.str("a string comfortably longer than sixteen bytes");
    assert!(w.finish().is_none());

    let mut buf = [0u8; 4096];
    let mut w = Writer::new(&mut buf);
    w.str(&"x".repeat(MAX_STR + 1));
    assert!(w.finish().is_none(), "a string past MAX_STR must not be written");
}

#[test]
fn unknown_opcodes_and_replies_are_not_guessed_at() {
    // 6, 7, 8, and 9 are OpenSession, CloseSession, Sessions, and Power, tested
    // below
    for v in [0u8, 10, 99, 255] {
        assert!(Op::from_u8(v).is_none(), "op {v}");
    }
    for v in [0u8, 8, 99, 255] {
        assert!(Reply::from_u8(v).is_none(), "reply {v}");
    }
    assert_eq!(Reply::from_u8(7), Some(Reply::NeedPassword));
    assert_eq!(Op::from_u8(1), Some(Op::Run));
    assert_eq!(Reply::from_u8(2), Some(Reply::Denied));
    assert_eq!(Op::from_u8(6), Some(Op::OpenSession));
    assert_eq!(Op::from_u8(7), Some(Op::CloseSession));
    assert_eq!(Op::from_u8(8), Some(Op::Sessions));
    assert_eq!(Op::from_u8(9), Some(Op::Power));
}

/// An unknown byte must be refused, not read as "off"; this op can be granted
/// without a password
#[test]
fn a_power_action_is_one_of_two_things() {
    use mikuauth::proto::power_action;
    assert!(power_action::valid(power_action::OFF));
    assert!(power_action::valid(power_action::REBOOT));
    for v in [2u8, 3, 200, 255] {
        assert!(!power_action::valid(v), "action {v}");
    }
    assert_eq!(power_action::name(power_action::OFF), "power off");
    assert_eq!(power_action::name(power_action::REBOOT), "reboot");
    assert_eq!(power_action::name(200), "?");
}

#[test]
fn a_session_kind_is_one_of_two_things() {
    use mikuauth::proto::session_kind;

    assert!(session_kind::valid(session_kind::CONSOLE));
    assert!(session_kind::valid(session_kind::GRAPHICAL));
    // An unknown kind is refused, not treated as a console session
    for k in [2u8, 3, 99, 255] {
        assert!(!session_kind::valid(k), "kind {k}");
        assert_eq!(session_kind::name(k), "?");
    }
    assert_eq!(session_kind::name(session_kind::CONSOLE), "console");
    assert_eq!(session_kind::name(session_kind::GRAPHICAL), "graphical");
}

#[test]
fn an_open_session_request_round_trips() {
    // Exact bytes from login and maygreet: `user`, kind, seat. No uid, program,
    // or credential
    let mut buf = [0u8; 256];
    let mut w = Writer::new(&mut buf);
    w.u8(mikuauth::proto::VERSION)
        .u8(Op::OpenSession as u8)
        .u16(0)
        .str("miku")
        .u8(mikuauth::proto::session_kind::GRAPHICAL)
        .u8(0);
    let framed = w.finish().unwrap();

    let mut r = Reader::new(&framed[4..]);
    assert_eq!(r.u8(), Some(mikuauth::proto::VERSION));
    assert_eq!(r.u8().and_then(Op::from_u8), Some(Op::OpenSession));
    assert_eq!(r.u16(), Some(0));
    assert_eq!(r.str(), Some("miku"));
    assert_eq!(r.u8(), Some(mikuauth::proto::session_kind::GRAPHICAL));
    assert_eq!(r.u8(), Some(0));
    // Reading past the end returns None
    assert_eq!(r.u8(), None);
}

#[test]
fn an_empty_string_round_trips_and_is_not_a_failure() {
    // An empty argument differs from a missing one
    let mut buf = [0u8; 64];
    let mut w = Writer::new(&mut buf);
    w.str("").str("x");
    let framed = w.finish().unwrap();
    let mut r = Reader::new(&framed[4..]);
    assert_eq!(r.str(), Some(""));
    assert_eq!(r.str(), Some("x"));
    assert_eq!(r.str(), None, "and then there is nothing left");
}
