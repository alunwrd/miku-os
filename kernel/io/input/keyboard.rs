// The keyboard, from a scancode to a terminal
//
// Beside `layout.rs`, which says what a key means, and `stdin.rs`, the ring the
// interrupt fills.
//
// The loop wakes every tick, 4 ms, for `console::flush()` at the bottom: an
// `sfence` plus the pushed-display hook, on a cadence and not on a keystroke,
// since a program printing for a long time draws from another thread and would
// otherwise not reach the screen until it finished.
//
// `sched::wait::WaitQueue` does not fit. Its producer would be `stdin::push`,
// which runs in the keyboard interrupt with interrupts off;
// `WaitQueue::wake_all` takes a spin `Mutex` and allocates, where taking a lock
// a thread on this core may hold is a deadlock. Without an early wake a queue
// is a `sleep`.

use core::sync::atomic::Ordering;

use pc_keyboard::DecodedKey;

use crate::shell::{handle_keypress, CMD_POLL_TICKS, KBD_POLL_TICKS};

/// Shift+PageUp / Shift+PageDown, which move the window through the history
///
/// True when the key was handled here and must not reach the program. The rows
/// moved are half a screen, as a page key does on terminals, leaving some
/// common text
fn scrolled(tty: crate::io::tty::TtyId, code: pc_keyboard::KeyCode) -> bool {
    use pc_keyboard::KeyCode as K;
    if !super::mods::shift() {
        return false;
    }
    let step = (crate::io::tty::size(tty).1 / 2).max(1);
    let at = crate::io::tty::view(tty);
    match code {
        K::PageUp   => { crate::io::tty::set_view(tty, at.saturating_add(step)); true }
        K::PageDown => { crate::io::tty::set_view(tty, at.saturating_sub(step)); true }
        _ => false,
    }
}

/// What a non-character key sends, in the encoding everything expects
///
/// The `CSI` forms every terminal emits and every readline parses: `ESC [ A`
/// for up and its neighbours, the `~` family for the editing keys. A key not
/// listed sends nothing, as an unrecognised sequence would be characters in the
/// middle of a line
fn key_sequence(code: pc_keyboard::KeyCode) -> Option<&'static str> {
    use pc_keyboard::KeyCode as K;
    Some(match code {
        K::ArrowUp    => "\x1b[A",
        K::ArrowDown  => "\x1b[B",
        K::ArrowRight => "\x1b[C",
        K::ArrowLeft  => "\x1b[D",
        K::Home       => "\x1b[H",
        K::End        => "\x1b[F",
        K::PageUp     => "\x1b[5~",
        K::PageDown   => "\x1b[6~",
        K::Delete     => "\x1b[3~",
        K::Insert     => "\x1b[2~",
        _ => return None,
    })
}

pub fn kbd_thread() -> ! {
    use pc_keyboard::{HandleControl, Keyboard, ScancodeSet1};
    // This machine's layout, not the crate's: the crate's US table does not map
    // the keycode its own scancode set produces for `\` and `|`. See
    // `io::input::layout`
    let mut keyboard = Keyboard::new(
        ScancodeSet1::new(),
        crate::io::input::layout::MikuUs104,
        HandleControl::MapLettersToUnicode,
    );
    crate::serial_println!("[kbd] thread started");
    loop {
        if !crate::kcore::boot_state::is_done() {
            crate::sched::sleep(CMD_POLL_TICKS);
            continue;
        }
        let mut got = false;
        while let Some(sc) = crate::io::input::stdin::pop() {
            got = true;
            if let Ok(Some(ev)) = keyboard.add_byte(sc) {
                // Which terminal these keys belong to: the one with the
                // keyboard. Only the console's no-foreground case is the kernel
                // shell's own line editor. A pty's keys are its own whether or
                // not anything is reading: otherwise a terminal loses what was
                // typed before its program read
                super::mods::observe(&ev);
                if let Some(key) = keyboard.process_keyevent(ev) {
                    // What a character means is the line discipline's to say,
                    // including Ctrl-C, which it turns into a signal for that
                    // terminal's foreground
                    let tty = crate::io::tty::active();
                    if tty != crate::io::tty::SYSTEM
                        || crate::io::tty::foreground(tty) != 0
                    {
                        // A key that is not a character is a sequence, needed
                        // for arrow keys, Home, Delete and shell history.
                        // `feed_key` passes them only in raw mode; see the note
                        // there
                        match key {
                            DecodedKey::Unicode(c) => crate::io::tty::feed_char(tty, c),
                            // Shift+PageUp/PageDown scrolls the terminal and
                            // does not reach the program. The same keycode
                            // either way, so the layout forwards the modifier
                            // state; unshifted the key is the program's,
                            // shifted it is the window's
                            DecodedKey::RawKey(code) => {
                                // Not only after a keypress: a command running
                                // for a while, and any userspace program in the
                                // foreground, print from the other thread and
                                // would otherwise not reach a pushed display
                                // until they finished. Costs nothing when
                                // nothing has been drawn: the console reports
                                // no damage and the hook returns immediately
                                if !scrolled(tty, code) {
                                    if let Some(seq) = key_sequence(code) {
                                        crate::io::tty::feed_key(tty, seq);
                                    }
                                }
                            }
                        }
                    } else {
                        match key {
                            DecodedKey::Unicode('\u{0003}') => {
                                crate::net::CTRL_C.store(true, Ordering::SeqCst);
                                crate::println!("^C");
                            }
                            other => handle_keypress(other),
                        }
                    }
                }
            }
        }
        // Not only after a keypress: a command that runs for a while, and any
        // userspace program in the foreground, print from the *other* thread
        // and would otherwise not reach a pushed display until they finished.
        // Costs nothing when nothing has been drawn - the console reports no
        // damage and the hook returns immediately
        crate::io::console::flush();
        if !got {
            crate::sched::sleep(KBD_POLL_TICKS);
        }
    }
}