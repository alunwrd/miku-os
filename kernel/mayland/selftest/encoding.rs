// The encoding, and the handshake on top of it
//
// Every argument type out and back, a message split across two writes, and a
// client asking the registry what is there. The client encoder produces the
// bytes, the server consumes exactly those, and the client decoder reads back
// what the server sent, so a wire format wrong in one direction shows here.

use super::*;

/// Every argument type out and back again
pub(super) fn test_wire(t: &mut Runner) {
    let mut w = Writer::new();
    let array: [u8; 5] = [1, 2, 3, 4, 5];
    let _ = w.message(0x1234, 7, &[
        Arg::Int(-42),
        Arg::Uint(0xdead_beef),
        Arg::Fixed(wire::fixed_from_int(-3)),
        Arg::Object(9),
        Arg::NewId(10),
        Arg::Str(Some("may")),
        Arg::Str(None),
        Arg::Array(&array),
    ]);
    let bytes = w.take();

    // A four-character string is 4 bytes of length + "may\0" = 8; a 5-byte
    // array is 4 + 8 with padding. Header 8 + 5 words + 8 + 4 + 12 = 52
    let expect_len = 8 + 5 * 4 + 8 + 4 + 12;
    if bytes.len() != expect_len {
        t.check("wire: encoded length", false,
                format!("{} bytes, expected {}", bytes.len(), expect_len));
        return;
    }

    let h = match wire::peek_header(&bytes) {
        Ok(Some(h)) => h,
        other => {
            t.check("wire: header", false, format!("{:?}", other.is_ok()));
            return;
        }
    };
    let mut r = Reader::new(&bytes[wire::HEADER_LEN..h.size]);
    let ok = h.sender == 0x1234
        && h.opcode == 7
        && h.size == expect_len
        && r.int()    == Ok(-42)
        && r.uint()   == Ok(0xdead_beef)
        && r.fixed()  == Ok(wire::fixed_from_int(-3))
        && r.object() == Ok(9)
        && r.new_id() == Ok(10)
        && r.string() == Ok(Some("may"))
        && r.string() == Ok(None)
        && r.array()  == Ok(&array[..]);

    t.check("wire: every argument type survives a round trip", ok && r.is_empty(),
            format!("{} bytes, sender {}, opcode {}", bytes.len(), h.sender, h.opcode));
}

/// A stream that stops mid-message must be held, not guessed at
pub(super) fn test_partial(t: &mut Runner) {
    let mut w = Writer::new();
    let _ = w.message(1, 0, &[Arg::Uint(7), Arg::Uint(8)]);
    let bytes = w.take();

    let mut held = true;
    for cut in 1..bytes.len() {
        if !matches!(wire::peek_header(&bytes[..cut]), Ok(None)) {
            held = false;
            break;
        }
    }
    let whole = matches!(wire::peek_header(&bytes), Ok(Some(_)));

    // A size field that does not reach the header is refused, rather than
    // producing a zero-length message the reader would spin on
    let mut bad = bytes.clone();
    bad[4..8].copy_from_slice(&(4u32 << 16).to_le_bytes());
    let refused = wire::peek_header(&bad).is_err();

    t.check("wire: a truncated stream is held, a bad size is refused",
            held && whole && refused,
            format!("{} prefixes held, impossible size {}",
                    bytes.len() - 1, if refused { "refused" } else { "ACCEPTED" }));
}

/// get_registry lists the globals, and binding one produces its state
pub(super) fn test_handshake(t: &mut Runner) {
    let cl = server::connect(0);
    let mut conn = ClientConn::new();

    let registry = conn.get_registry();
    let events = exchange(&mut conn, cl);

    let mut names: Vec<(u32, String, u32)> = Vec::new();
    for e in &events {
        if let Event::Global { name, interface, version } = e {
            names.push((*name, interface.clone(), *version));
        }
    }
    let expected = server::globals().len();
    t.check("registry: every global is advertised", names.len() == expected,
            format!("{} of {} - {}", names.len(), expected,
                    names.iter().map(|(_, i, _)| i.as_str())
                         .collect::<Vec<_>>().join(", ")));

    // Bind all four and check each one's opening state
    let shm_name = names.iter().find(|(_, i, _)| i == "may_shm").map(|(n, _, _)| *n);
    let seat_name = names.iter().find(|(_, i, _)| i == "may_seat").map(|(n, _, _)| *n);
    let out_name = names.iter().find(|(_, i, _)| i == "may_output").map(|(n, _, _)| *n);

    if let (Some(sn), Some(en), Some(on)) = (shm_name, seat_name, out_name) {
        conn.bind(registry, sn, Interface::Shm, 1);
        conn.bind(registry, en, Interface::Seat, 1);
        conn.bind(registry, on, Interface::Output, 1);
        let events = exchange(&mut conn, cl);

        let formats: Vec<u32> = events.iter().filter_map(|e| match e {
            Event::ShmFormat { format } => Some(*format), _ => None }).collect();
        let caps = events.iter().find_map(|e| match e {
            Event::SeatCapabilities { caps } => Some(*caps), _ => None });
        let mode = events.iter().find_map(|e| match e {
            Event::OutputMode { width, height, .. } => Some((*width, *height)),
            _ => None });
        let done = events.iter().any(|e| matches!(e, Event::OutputDone));

        t.check("bind: a global sends its state at once",
                formats.contains(&protocol::shm::FORMAT_XRGB8888)
                    && caps == Some(protocol::seat::CAP_KEYBOARD | protocol::seat::CAP_POINTER)
                    && mode.is_some() && done,
                format!("{} formats, caps {:?}, mode {:?}", formats.len(), caps, mode));
    } else {
        t.check("bind: a global sends its state at once", false,
                "a global is missing from the registry".to_string());
    }

    // sync proves ordering: it fires only after everything before it ran
    let cb = conn.sync();
    let events = exchange(&mut conn, cl);
    let fired = events.iter().any(|e| matches!(e, Event::Done { id, .. } if *id == cb));
    let freed = events.iter().any(|e| matches!(e, Event::DeleteId { id } if *id == cb));
    t.check("display: sync fires and frees its callback id", fired && freed,
            format!("done {}, delete_id {}", fired, freed));

    server::disconnect(cl);
}

/// An opcode no interface defines is fatal, and says why
pub(super) fn test_bad_opcode(t: &mut Runner) {
    let cl = server::connect(0);
    let mut conn = ClientConn::new();
    conn.raw(protocol::DISPLAY_ID, 99, &[]);
    let events = exchange(&mut conn, cl);

    let err = first_error(&events);
    let dead = !server::is_alive(cl);
    t.check("errors: an unknown request kills the connection",
            err.is_some() && dead,
            match &err {
                Some((_, why)) => why.clone(),
                None => "no error event".to_string(),
            });
    server::disconnect(cl);
}
