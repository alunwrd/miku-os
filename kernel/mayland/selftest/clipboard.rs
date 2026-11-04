// may_clipboard: what one program hands to another
//
// Checks that the bytes arrive, and the two boundaries: a program that was not
// typed at may not put anything in the selection, and a program that is not
// focused may not read it. Both are refused with a reason, and the cases check
// the reason rather than only the refusal

use super::*;
use crate::mayland::server::selection::{self, Which};

/// Puts `text` in the pool and returns a serial the compositor will accept
///
/// A copy that answers no key is refused, so a test has to press something
/// first. It also shows the serial reaches the client
fn typed_serial(conn: &mut ClientConn, cl: u32) -> u32 {
    crate::mayland::server::key(46, true, 0);
    let events = drain_events(conn, cl);
    events.iter().rev().find_map(|e| match e {
        Event::Key { serial, .. } => Some(*serial), _ => None,
    }).unwrap_or(0)
}

/// A copy in one program and a paste in another
pub(super) fn test_clipboard_round_trip(t: &mut Runner) {
    const TEXT: &[u8] = b"the selection travels in a pool";
    let stride = 64 * 4;

    // The source. It needs a window, because setting the selection needs a
    // keystroke and a keystroke needs the keyboard
    let Some((src, mut a, comp_a, xdg_a, ha, buf_a)) =
        client_with_pool((stride * 64) as usize, 64, 64, stride)
    else {
        t.check("clipboard: a copy in one program is a paste in another",
                false, "no source client".into());
        return;
    };
    let (clip_a, _) = bind_clipboard(&mut a, src);
    let _ = open_window(&mut a, src, comp_a, xdg_a, "source", buf_a, 64, 64);
    super::pointer::bind_keyboard(&mut a, src);
    let serial_a = typed_serial(&mut a, src);
    // Written straight into the pool: this in-kernel client has no mapping of
    // its own and the compositor's view is the only one
    let _ = shm::with_bytes_mut(ha, 0, TEXT.len(), |b| b.copy_from_slice(TEXT));
    a.clipboard_set(clip_a, serial_a, protocol::clipboard::CLIPBOARD,
                    "text/plain", ha, 0, TEXT.len() as u32);
    let events = exchange(&mut a, src);
    let copied = first_error(&events).is_none();

    // The reader, which now has the keyboard because its window opened last
    let Some((dst, mut b, comp_b, xdg_b, hb, buf_b)) =
        client_with_pool((stride * 64) as usize, 64, 64, stride)
    else {
        t.check("clipboard: a copy in one program is a paste in another",
                false, "no reading client".into());
        server::disconnect(src);
        let _ = shm::destroy(ha);
        return;
    };
    // Told at the bind: a program starting after the copy has to know there is
    // something to paste without asking
    let (clip_b, offered) = bind_clipboard(&mut b, dst);
    let _ = open_window(&mut b, dst, comp_b, xdg_b, "reader", buf_b, 64, 64);
    super::pointer::bind_keyboard(&mut b, dst);

    let serial_b = typed_serial(&mut b, dst);
    b.clipboard_get(clip_b, serial_b, protocol::clipboard::CLIPBOARD, "text/plain");
    let events = exchange(&mut b, dst);
    let got = events.iter().find_map(|e| match e {
        Event::ClipboardData { handle, size, .. } => Some((*handle, *size)),
        _ => None,
    });
    // The pool is the reader's own: one pool cannot be mapped into two address
    // spaces, which is why the content is copied twice
    let same = match got {
        Some((h, n)) => h != ha
            && n as usize == TEXT.len()
            && shm::with_bytes(h, 0, n as usize, |x| x == TEXT).unwrap_or(false),
        None => false,
    };

    // Asking again does not accumulate pools: a program pasting in a loop must
    // not use up its share
    let mut still = true;
    for _ in 0..12 {
        let s = typed_serial(&mut b, dst);
        b.clipboard_get(clip_b, s, protocol::clipboard::CLIPBOARD, "text/plain");
        let e = exchange(&mut b, dst);
        if first_error(&e).is_some() {
            still = false;
            break;
        }
    }

    t.check("clipboard: a copy in one program is a paste in another",
            copied && offered && same && still,
            format!("the copy was accepted {}, the reader was told there is \
                     something {}, the bytes came back in a pool of its own {}, \
                     twelve pastes in a row still work {}",
                    copied, offered, same, still));

    server::disconnect(dst);
    server::disconnect(src);
    let _ = shm::destroy(ha);
    let _ = shm::destroy(hb);
}

/// The two boundaries, each refused for its own stated reason
pub(super) fn test_clipboard_boundaries(t: &mut Runner) {
    const TEXT: &[u8] = b"secret";
    let stride = 64 * 4;
    let Some((src, mut a, comp_a, xdg_a, ha, buf_a)) =
        client_with_pool((stride * 64) as usize, 64, 64, stride)
    else {
        t.check("clipboard: only what was typed at may copy, only what is \
                 focused may paste", false, "no client".into());
        return;
    };
    let (clip_a, _) = bind_clipboard(&mut a, src);
    let _ = open_window(&mut a, src, comp_a, xdg_a, "source", buf_a, 64, 64);
    let _ = shm::with_bytes_mut(ha, 0, TEXT.len(), |b| b.copy_from_slice(TEXT));

    // A copy that answers nothing. Serial 1 has been issued at some point, but
    // it is not an input serial
    a.clipboard_set(clip_a, 1, protocol::clipboard::CLIPBOARD,
                    "text/plain", ha, 0, TEXT.len() as u32);
    let events = exchange(&mut a, src);
    let no_serial = matches!(first_error(&events),
        Some((_, ref why)) if why.contains("answers no key and no button"));
    server::disconnect(src);

    // A range outside the pool it names, caught by `with_bytes`. Shows the
    // compositor uses it rather than trusting the client's arithmetic
    let Some((src2, mut c, comp_c, xdg_c, hc, buf_c)) =
        client_with_pool((stride * 64) as usize, 64, 64, stride)
    else {
        t.check("clipboard: only what was typed at may copy, only what is \
                 focused may paste", false, "no second client".into());
        let _ = shm::destroy(ha);
        return;
    };
    let (clip_c, _) = bind_clipboard(&mut c, src2);
    let _ = open_window(&mut c, src2, comp_c, xdg_c, "source", buf_c, 64, 64);
    super::pointer::bind_keyboard(&mut c, src2);
    let s = typed_serial(&mut c, src2);
    // Past the end of a 16 KiB pool, but under the 64 KiB ceiling: a length
    // over the ceiling is refused by a different check, and this one would not
    // run
    c.clipboard_set(clip_c, s, protocol::clipboard::CLIPBOARD,
                    "text/plain", hc, 0, 20_000);
    let events = exchange(&mut c, src2);
    let past_the_end = matches!(first_error(&events),
        Some((_, ref why)) if why.contains("not inside that pool"));
    server::disconnect(src2);

    // A read by a program that is not the one being typed at
    let Some((src3, mut d, comp_d, xdg_d, hd, buf_d)) =
        client_with_pool((stride * 64) as usize, 64, 64, stride)
    else {
        t.check("clipboard: only what was typed at may copy, only what is \
                 focused may paste", false, "no third client".into());
        let _ = shm::destroy(ha);
        let _ = shm::destroy(hc);
        return;
    };
    let (clip_d, _) = bind_clipboard(&mut d, src3);
    let _ = open_window(&mut d, src3, comp_d, xdg_d, "owner", buf_d, 64, 64);
    super::pointer::bind_keyboard(&mut d, src3);
    let s = typed_serial(&mut d, src3);
    let _ = shm::with_bytes_mut(hd, 0, TEXT.len(), |b| b.copy_from_slice(TEXT));
    d.clipboard_set(clip_d, s, protocol::clipboard::CLIPBOARD,
                    "text/plain", hd, 0, TEXT.len() as u32);
    let _ = exchange(&mut d, src3);

    // A second program with no window, so it can never have the keyboard: a
    // background process
    let Some((spy, mut e, _comp_e, _xdg_e, he, _buf_e)) =
        client_with_pool((stride * 64) as usize, 64, 64, stride)
    else {
        t.check("clipboard: only what was typed at may copy, only what is \
                 focused may paste", false, "no background client".into());
        server::disconnect(src3);
        let _ = shm::destroy(ha);
        let _ = shm::destroy(hc);
        let _ = shm::destroy(hd);
        return;
    };
    let (clip_e, _) = bind_clipboard(&mut e, spy);
    e.clipboard_get(clip_e, 1, protocol::clipboard::CLIPBOARD, "text/plain");
    let events = exchange(&mut e, spy);
    let refused_read = matches!(first_error(&events),
        Some((_, ref why)) if why.contains("only the window being typed at"));

    // And the selection goes when the program that set it does
    server::disconnect(src3);
    let empty = {
        let s = server::lock();
        selection::peek(&s, Which::Clipboard).is_none()
    };

    t.check("clipboard: only what was typed at may copy, only what is focused \
             may paste",
            no_serial && past_the_end && refused_read && empty,
            format!("a copy answering nothing is refused {}, a range past the \
                     pool is refused {}, a background program may not read {}, \
                     the selection dies with its owner {}",
                    no_serial, past_the_end, refused_read, empty));

    server::disconnect(spy);
    let _ = shm::destroy(ha);
    let _ = shm::destroy(hc);
    let _ = shm::destroy(hd);
    let _ = shm::destroy(he);
}

/// Binds `may_clipboard` and says whether the bind itself brought an offer
///
/// One call because they are one exchange: the compositor answers a bind with
/// what is already in the selection
fn bind_clipboard(conn: &mut ClientConn, cl: u32) -> (u32, bool) {
    let registry = 2;
    let Some(name) = server::globals().iter()
        .find(|g| g.iface == Interface::Clipboard).map(|g| g.name)
    else { return (0, false) };
    let id = conn.bind(registry, name, Interface::Clipboard, 1);
    let events = exchange(conn, cl);
    let offered = events.iter().any(|e| matches!(e, Event::ClipboardOffer { .. }));
    (id, offered)
}
