// The terminal table, checked without a screen, a keyboard or a disk
//
// Text in and cells out: the part of a terminal that can be wrong in an
// interesting way has no pixels in it. Run with `tty test`, or `run=tty_test`
// on the boot line.
//
// The property checked is that a `TtyId` names one terminal for ever. A slot
// freed and reused comes back with a higher generation, so an id captured
// before the hangup answers "no such terminal" and does not address another's
// text. Linux has `vhangup` and `revoke` for this.

use core::sync::atomic::Ordering;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::{MAX_TTY, SYSTEM, TtyId, clear_foreground, exists, feed_char, foreground, hangup,
            list, master_owner, open_pty, readable, release, set_foreground, with_grid};
use crate::process::signal::{self, DISP_CATCH, DISP_DEFAULT, SIGINT, SIGKILL, SIGTERM};

/// A cell's size, from the one place that defines it
const CELL: usize = crate::mayland::protocol::console::CELL_BYTES;

struct Case {
    name: &'static str,
    ok: bool,
    detail: String,
}

struct Runner {
    cases: Vec<Case>,
}

impl Runner {
    fn check(&mut self, name: &'static str, ok: bool, detail: String) {
        if ok {
            crate::serial_println!("[tty-test] ok   {} - {}", name, detail);
        } else {
            crate::serial_println!("[tty-test] FAIL {} - {}", name, detail);
        }
        self.cases.push(Case { name, ok, detail });
    }
}

pub fn run() -> (usize, usize) {
    let mut t = Runner { cases: Vec::new() };

    // The console is always there and is never a pty. The first is what `fd 1`
    // falls back to, the second stops a client claiming the machine's own text
    // by pretending to own it
    t.check("tty: the console always exists",
            exists(SYSTEM),
            format!("tty0 present, owner {:?}", master_owner(SYSTEM)));
    t.check("tty: the console has no master",
            master_owner(SYSTEM).is_none(),
            String::from("nobody owns tty0, so nobody can claim it as their pty"));

    // A pty opens, is owned by whoever asked, and has the shape it asked for
    let a = open_pty(4242, 80, 24);
    let Some(a) = a else {
        t.check("tty: a pty opens", false, String::from("open_pty refused"));
        return tally(&t);
    };
    t.check("tty: a pty opens",
            exists(a) && a != SYSTEM,
            format!("id {:#x}, index {}, generation {}", a.raw(), a.index(), a.generation()));
    t.check("tty: a pty belongs to whoever opened it",
            master_owner(a) == Some(4242),
            format!("owner {:?}", master_owner(a)));
    let shape = with_grid(a, |g| g.size());
    t.check("tty: a pty has the shape it asked for",
            shape == Some((80, 24)),
            format!("{:?}", shape));

    // Two of them are two, and neither is the console
    let b = open_pty(4243, 100, 30).expect("second pty");
    t.check("tty: two terminals are two",
            a != b && a.index() != b.index() && exists(a) && exists(b),
            format!("{:#x} and {:#x}", a.raw(), b.raw()));

    // The text of one is not the text of the other: no cross-talk
    with_grid(a, |g| g.touch_rows(0, 1));
    let da = with_grid(a, |g| g.take_row_damage()).flatten();
    let db = with_grid(b, |g| g.take_row_damage()).flatten();
    t.check("tty: two terminals are damaged independently",
            da == Some((0, 1)) && db.is_none(),
            format!("a {:?}, b {:?}", da, db));

    // Text goes in and comes out as cells. The pty writes through the same
    // `TextGrid` as the console
    with_grid(b, |g| g.write_str("hello", [1, 2, 3]));
    let mut buf = alloc::vec![0u8; 100 * 30 * CELL];
    let got = with_grid(b, |g| g.copy_cells(&mut buf)).flatten();
    let first = &buf[..5 * CELL];
    t.check("tty: text written to a terminal is in its cells",
            got == Some((100, 30))
                && (0..5).map(|i| cell_char(&buf, i)).collect::<String>() == "hello",
            format!("{:?}, read {:?}", got,
                    (0..5).map(|i| cell_char(&buf, i)).collect::<String>()));

    // Nor did it land in the other one
    let mut buf_a = alloc::vec![0u8; 80 * 24 * CELL];
    let clean = with_grid(a, |g| g.copy_cells(&mut buf_a)).flatten();
    t.check("tty: writing to one terminal leaves the other alone",
            clean == Some((80, 24)) && buf_a[..4] == [b' ', 0, 0, 0],
            format!("a's first cell {:?}", &buf_a[..4]));

    // Scrolling feeds the history ring, indexed at `MAX_COLS` whatever the
    // terminal's own width. A grid allocated at its own width would be walked
    // off the end on the first scrolled row
    with_grid(b, |g| {
        for i in 0..40 {
            g.write_str("x", [1, 2, 3]);
            let _ = i;
            g.write_str("\n", [1, 2, 3]);
        }
    });
    let after = with_grid(b, |g| g.cursor());
    t.check("tty: a terminal scrolls without walking off its history",
            after == Some((0, 29)),
            format!("pen at {:?} after forty lines on a thirty-row terminal", after));

    // A resize must not cost the text
    //
    // A window opens, the shell prints its banner and prompt, and the first
    // configure changing the character count must not blank the cells or send
    // the pen home.
    //
    // `mayland/selftest/console.rs` covers a refused size; this covers an
    // accepted one
    let rs = open_pty(4248, 40, 10).expect("resize pty");
    with_grid(rs, |g| g.write_str("keep me", super::super::console::COLOR_MIKU));

    // Sized for the shapes this case uses (60x14 is 3360 bytes), not for the
    // ceiling
    let read_first = |id: TtyId, n: usize| -> String {
        let mut buf = alloc::vec![0u8; 64 * 16 * CELL];
        match with_grid(id, |g| g.copy_cells(&mut buf)).flatten() {
            // Four bytes a cell, the character first
            Some(_) => (0..n).map(|i| cell_char(&buf, i)).collect(),
            None => String::new(),
        }
    };

    let wider = crate::io::tty::set_size(rs, 60, 14);
    let after_wider = read_first(rs, 7);
    t.check("tty: widening a terminal keeps its text",
            wider && after_wider == "keep me",
            format!("read {:?} after 40x10 -> 60x14", after_wider));

    let narrower = crate::io::tty::set_size(rs, 24, 6);
    let after_narrower = read_first(rs, 7);
    t.check("tty: narrowing a terminal keeps what still fits",
            narrower && after_narrower == "keep me",
            format!("read {:?} after 60x14 -> 24x6", after_narrower));

    // The pen comes with it: a shell that resized would otherwise print its
    // next line over its own prompt
    let pen = with_grid(rs, |g| g.cursor());
    t.check("tty: a resize keeps the pen inside the new shape",
            matches!(pen, Some((c, r)) if c < 24 && r < 6),
            format!("pen at {:?} on a 24x6 terminal", pen));
    release(rs);

    // `print!` follows the task, not a parameter. Nothing in `kernel/shell`
    // mentions a terminal, so this working means the shell works on any of
    // them. A terminal of its own: `b` has been scrolled by the case above, and
    // this is about where the text lands
    let r = open_pty(4246, 40, 8).expect("routing pty");
    let saved = crate::sched::current_tty();
    let mine = crate::sched::current_pid();
    crate::sched::set_ctty(mine, r.raw());
    crate::print!("routed");
    crate::sched::set_ctty(mine, saved);
    let mut rbuf = alloc::vec![0u8; 40 * 8 * CELL];
    let _ = with_grid(r, |g| g.copy_cells(&mut rbuf));
    let routed: String = (0..6).map(|i| cell_char(&rbuf, i)).collect();
    t.check("tty: print follows the task's terminal, not the console",
            routed == "routed",
            format!("first cells read {:?}", routed));
    release(r);

    // /dev/console is the console, /dev/tty is yours
    //
    // `/dev/console` names tty0; `/dev/tty` is how a program with redirected
    // stdout reaches the person at the keyboard.
    //
    // The half checkable without a descriptor: writing to a named terminal goes
    // there and nowhere else, whoever writes
    let dev = open_pty(4247, 40, 8).expect("device pty");
    let saved2 = crate::sched::current_tty();
    crate::sched::set_ctty(mine, dev.raw());
    // What `write(fd, ...)` on /dev/console does, from a task on a pty. A real
    // marker: the question is where the bytes went. It lands on the console,
    // with the rest of this suite's output
    crate::io::tty::write_text(SYSTEM, "[tty-test] this line belongs on the console\n");
    let leaked = read_first(dev, 9);
    // What /dev/tty does, resolving to this task's own
    crate::io::tty::write_text(dev, "mine");
    let landed = read_first(dev, 4);
    crate::sched::set_ctty(mine, saved2);
    t.check("tty: writing the console does not land in the writer's window",
            leaked == "         ",
            format!("the pty's first row reads {:?} after a console write", leaked));
    t.check("tty: writing a named terminal lands in that terminal",
            landed == "mine",
            format!("read {:?} back from the terminal that was named", landed));

    // A terminal that has gone takes the write with it, no bytes counted, which
    // `sys_write` turns into EIO
    release(dev);
    t.check("tty: writing a terminal that has gone fails",
            !crate::io::tty::write_text(dev, "x"),
            String::from("write_text refused a released id"));

    // The two device nodes are distinct, with Linux's numbers: 5:0 is /dev/tty
    // and 5:1 is /dev/console. `classify` routes on this, and a mix-up writes
    // to the wrong screen
    use crate::fs::devfs::{DevType, dev_type_from_node};
    t.check("tty: /dev/tty and /dev/console are different devices",
            DevType::Tty.major() == 5 && DevType::Tty.minor() == 0
                && DevType::Console.major() == 5 && DevType::Console.minor() == 1
                && dev_type_from_node(5, 0) == Some(DevType::Tty)
                && dev_type_from_node(5, 1) == Some(DevType::Console),
            String::from("5:0 is /dev/tty, 5:1 is /dev/console, both ways round"));

    // Non-ASCII text in cells
    //
    // `'П'` is one cell holding the whole codepoint
    let ru = open_pty(4250, 40, 4).expect("cyrillic pty");
    with_grid(ru, |g| g.write_str("Привет", super::super::console::COLOR_MIKU));
    let read_back = {
        let mut buf = alloc::vec![0u8; 64 * 16 * CELL];
        match with_grid(ru, |g| g.copy_cells(&mut buf)).flatten() {
            Some(_) => (0..6).map(|i| cell_char(&buf, i)).collect::<String>(),
            None => String::new(),
        }
    };
    t.check("cell: a Cyrillic line survives the grid it was printed into",
            read_back == "Привет",
            format!("read {:?} back", read_back));

    // One cell per character, not per byte. `Привет` is twelve bytes and six
    // columns; a grid advancing by bytes would misplace everything after it
    t.check("cell: a two-byte character still occupies one column",
            with_grid(ru, |g| g.cursor()) == Some((6, 0)),
            format!("pen at {:?} after six characters", with_grid(ru, |g| g.cursor())));

    // And it comes back out of a resize, which reads every cell
    crate::io::tty::set_size(ru, 60, 8);
    let after = {
        let mut buf = alloc::vec![0u8; 64 * 16 * CELL];
        match with_grid(ru, |g| g.copy_cells(&mut buf)).flatten() {
            Some(_) => (0..6).map(|i| cell_char(&buf, i)).collect::<String>(),
            None => String::new(),
        }
    };
    t.check("cell: and survives a resize",
            after == "Привет",
            format!("read {:?} after 40x4 -> 60x8", after));
    release(ru);

    // The pen a program may move
    //
    // Cursor addressing on a real terminal, read back as cells. The parser is
    // checked exhaustively on the host (a transcription of `Vt` and this grid);
    // these prove the transcription is of this code
    let cur = open_pty(4249, 20, 5).expect("cursor pty");
    let cells = |id: TtyId, from: usize, n: usize| -> String {
        let mut buf = alloc::vec![0u8; 64 * 16 * CELL];
        match with_grid(id, |g| g.copy_cells(&mut buf)).flatten() {
            Some(_) => (from..from + n).map(|i| cell_char(&buf, i)).collect(),
            None => String::new(),
        }
    };

    with_grid(cur, |g| g.write_str("\x1b[5;3H", super::super::console::COLOR_MIKU));
    t.check("vt: CUP puts the pen where it says",
            with_grid(cur, |g| g.cursor()) == Some((2, 4)),
            format!("pen at {:?} after ESC[5;3H", with_grid(cur, |g| g.cursor())));

    with_grid(cur, |g| g.write_str("\x1b[99;99H", super::super::console::COLOR_MIKU));
    t.check("vt: a position past the end lands inside the grid",
            with_grid(cur, |g| g.cursor()) == Some((19, 4)),
            format!("pen at {:?} on a 20x5 terminal", with_grid(cur, |g| g.cursor())));

    with_grid(cur, |g| g.write_str("\x1b[H\x1b[A\x1b[D", super::super::console::COLOR_MIKU));
    t.check("vt: moving off the top-left corner stays in the corner",
            with_grid(cur, |g| g.cursor()) == Some((0, 0)),
            String::from("up and left at (0,0) is still (0,0)"));

    // Erase leaves the pen alone. `ESC[2J` and `ESC[H` are two sequences and
    // every program sends both
    with_grid(cur, |g| g.write_str("\x1b[H\x1b[2Jabcdef\x1b[4G\x1b[K",
                                    super::super::console::COLOR_MIKU));
    let erased = cells(cur, 0, 6);
    t.check("vt: erase-in-line stops at the pen and leaves it there",
            erased == "abc   " && with_grid(cur, |g| g.cursor()) == Some((3, 0)),
            format!("row reads {:?}, pen {:?}", erased, with_grid(cur, |g| g.cursor())));

    // A terminal cleared with an escape must not have its text come back at the
    // first size change, as it would if the history ring kept what was on
    // screen
    with_grid(cur, |g| {
        g.write_str("one\ntwo\nthree\nfour\nfive\nsix\nseven\n",
                    super::super::console::COLOR_MIKU);
        g.write_str("\x1b[2J", super::super::console::COLOR_MIKU);
    });
    let wiped = cells(cur, 0, 20 * 5).trim().len();
    crate::io::tty::set_size(cur, 40, 10);
    let after_widen = cells(cur, 0, 40 * 10).trim().len();
    t.check("vt: text cleared with an escape does not come back on a resize",
            wiped == 0 && after_widen == 0,
            format!("{} cells after the clear, {} after widening", wiped, after_widen));

    // An escape that never ends, and one interrupted. Neither may eat the
    // terminal: the parser's state is on the grid, so a program dying
    // mid-sequence would otherwise silence every program after it
    with_grid(cur, |g| {
        g.write_str("\x1b[2J\x1b[H", super::super::console::COLOR_MIKU);
        g.write_str("\x1b[1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;1;", super::super::console::COLOR_MIKU);
        g.write_str("ok", super::super::console::COLOR_MIKU);
    });
    // The runaway is abandoned at `SEQ_MAX` and its remainder printed as text,
    // so the row holds the leftover parameters and then "ok". The check is that
    // "ok" arrived at all
    let survived = cells(cur, 0, 40);
    t.check("vt: a sequence that never ends gives the terminal back",
            survived.contains("ok"),
            format!("row reads {:?} after 40 parameters and no final byte",
                    survived.trim_end()));

    // The window title ported programs set: introducer and payload are both
    // consumed
    with_grid(cur, |g| {
        g.write_str("\x1b[2J\x1b[H", super::super::console::COLOR_MIKU);
        g.write_str("\x1b]0;a title\x07hi", super::super::console::COLOR_MIKU);
    });
    let titled = cells(cur, 0, 2);
    t.check("vt: a window title does not land on the screen",
            titled == "hi",
            format!("read {:?} after an OSC", titled));
    release(cur);

    // What has left the screen is still there
    //
    // The history ring is readable, and the window stays the same size: the
    // pool it is drawn from is allocated once
    let sb = open_pty(4251, 20, 3).expect("scrollback pty");
    for i in 1..=8 {
        with_grid(sb, |g| {
            let mut line = String::new();
            line.push_str("line");
            line.push((b'0' + i as u8) as char);
            line.push('\n');
            g.write_str(&line, super::super::console::COLOR_MIKU);
        });
    }
    let top = |id: TtyId| -> String {
        let mut buf = alloc::vec![0u8; 64 * 16 * CELL];
        match with_grid(id, |g| g.copy_cells(&mut buf)).flatten() {
            Some(_) => (0..5).map(|i| cell_char(&buf, i)).collect(),
            None => String::new(),
        }
    };
    let depth = with_grid(sb, |g| g.history_depth()).unwrap_or(0);
    t.check("scroll: rows that left the screen were kept",
            depth >= 5,
            format!("{} rows of history on a 3-row terminal after 8 lines", depth));

    let live = top(sb);
    let back2 = { crate::io::tty::set_view(sb, 2); top(sb) };
    t.check("scroll: going back shows an older row",
            back2 != live && back2.starts_with("line"),
            format!("top row was {:?}, two back it is {:?}", live, back2));

    let clamped = crate::io::tty::set_view(sb, 9999);
    t.check("scroll: asking for more than there is is clamped, not refused",
            clamped == depth,
            format!("asked for 9999, landed at {} with {} in the ring", clamped, depth));

    // Output does not move the window, as a busy log needs
    let held = crate::io::tty::view(sb);
    with_grid(sb, |g| g.write_str("more\n", super::super::console::COLOR_MIKU));
    t.check("scroll: new output does not drag the window back to the bottom",
            crate::io::tty::view(sb) == held,
            format!("view {} before printing, {} after", held, crate::io::tty::view(sb)));

    // Typing does
    feed_char(sb, 'x');
    t.check("scroll: typing returns to the live screen",
            crate::io::tty::view(sb) == 0,
            format!("view is {} after a keystroke", crate::io::tty::view(sb)));
    release(sb);

    // The control characters are settings
    let cc = open_pty(4252, 40, 4).expect("termios pty");
    // A job nobody is in, as in the job cases below: ^A raises a real SIGINT at
    // the foreground, and this task's own job there would kill the task running
    // the suite
    set_foreground(cc, 999_999);
    let mut t_alt = crate::io::tty::termios(cc);
    t_alt.c_cc[super::ldisc::VINTR] = 0x01;                 // stty intr ^A
    t_alt.c_cc[super::ldisc::VERASE] = 0x08;
    crate::io::tty::set_termios(cc, t_alt);

    // ^C is an ordinary character here and reaches nobody as a signal; ^A is
    // the interrupt. The line being thrown away is the visible half
    for ch in "ab".chars() { feed_char(cc, ch); }
    feed_char(cc, '\u{0001}');
    for ch in "cd\n".chars() { feed_char(cc, ch); }
    let mut line = [0u8; 32];
    let took = super::read_input(cc, &mut line, 32);
    let text = took.map(|n| core::str::from_utf8(&line[..n]).unwrap_or("?").trim_end());
    t.check("termios: the interrupt character can be moved",
            text == Some("cd"),
            format!("read {:?} after 'ab', ^A, 'cd'", text));

    // The old one is data, not a signal and not silence
    crate::io::tty::set_termios(cc, crate::io::tty::ldisc::Termios::cooked());
    t.check("termios: the defaults are the characters that used to be literals",
            crate::io::tty::termios(cc).c_cc[super::ldisc::VSUSP] == 0x1a
                && crate::io::tty::termios(cc).c_cc[super::ldisc::VINTR] == 0x03,
            String::from("VSUSP is Ctrl-Z and VINTR is Ctrl-C again"));
    clear_foreground(cc);
    release(cc);

    // A process that stops, and comes back
    //
    // With a real thread: this feature is what the scheduler does with one, and
    // a model on the host cannot say whether the kernel agrees. Bounded: if
    // `cont` fails the thread stays stopped without hanging the suite, and it
    // is killed either way at the end
    {
        let spinner = crate::sched::spawn_named(stop_spinner, "tty-test-stop", 10);

        // Wait for the counter to move, bounded by a deadline: on a four-CPU
        // machine the spinner may not be picked up within three ticks, and a
        // stop is applied at the target's next scheduling point
        let mut moving = false;
        let mut before = STOP_TICKS.load(Ordering::Relaxed);
        for _ in 0..50 {
            crate::sched::sleep(1);
            let now = STOP_TICKS.load(Ordering::Relaxed);
            if now > before { moving = true; before = now; break; }
        }
        // Once more, so `before` is a reading taken while it ran. A wait for
        // the condition too: a priority-10 spinner does not always get the
        // processor twice in two ticks
        let mut again = false;
        if moving {
            for _ in 0..50 {
                crate::sched::sleep(1);
                if STOP_TICKS.load(Ordering::Relaxed) > before { again = true; break; }
            }
        }
        let moving = moving && again;

        crate::process::signal::send_signal(spinner, crate::process::signal::SIGSTOP);
        // It stops at its next scheduling point; see `signal::request_stop`
        let mut stopped_state = false;
        for _ in 0..50 {
            crate::sched::sleep(1);
            if crate::sched::is_stopped(spinner) { stopped_state = true; break; }
        }
        let at_stop = STOP_TICKS.load(Ordering::Relaxed);
        crate::sched::sleep(5);
        let frozen = STOP_TICKS.load(Ordering::Relaxed) == at_stop;

        t.check("stop: SIGSTOP takes a thread off the processor",
                stopped_state && moving && frozen,
                format!("was moving {}, state stopped {}, counter frozen {}",
                        moving, stopped_state, frozen));

        // The one thing that must not lift it: any other signal, as
        // `send_signal` wakes its target after every send
        crate::process::signal::send_signal(spinner, crate::process::signal::SIGWINCH);
        crate::sched::sleep(3);
        t.check("stop: another signal does not undo it",
                crate::sched::is_stopped(spinner)
                    && STOP_TICKS.load(Ordering::Relaxed) == at_stop,
                String::from("still stopped and still frozen after a SIGWINCH"));

        crate::process::signal::send_signal(spinner, crate::process::signal::SIGCONT);
        crate::sched::sleep(5);
        let running_again = STOP_TICKS.load(Ordering::Relaxed) > at_stop;
        t.check("stop: SIGCONT is the way back, and it works",
                running_again && !crate::sched::is_stopped(spinner),
                format!("counter moved from {} to {}",
                        at_stop, STOP_TICKS.load(Ordering::Relaxed)));

        // A stopped thread must still be killable, or a stop could make
        // something unkillable
        crate::process::signal::send_signal(spinner, crate::process::signal::SIGSTOP);
        crate::sched::sleep(5);
        crate::sched::kill_with_code(spinner, 0);
        crate::sched::sleep(3);
        t.check("stop: a stopped thread can still be killed",
                !crate::sched::process_alive(spinner),
                format!("pid {} is gone", spinner));
    }

    // The master end
    //
    // The ring and the grid are fed from one place in one order. The host model
    // proves the property over random interleavings; these prove the plumbing
    {
        let Some(slot) = crate::io::tty::open_master() else {
            t.check("ptmx: a master opens", false, String::from("no slot"));
            return tally(&t);
        };
        let m = crate::io::tty::master_tty(slot).expect("master tty");
        // The reference `open` makes on the master node, which counts a master
        // (`VNode::inc_ref` -> `master_ref`)
        crate::io::tty::master_ref(slot);
        t.check("ptmx: opening a master makes a terminal",
                exists(m) && crate::io::tty::master_count(m) == 1,
                format!("{:#x}, {} master(s)", m.raw(), crate::io::tty::master_count(m)));

        // What a program prints, the master reads
        with_grid(m, |g| g.write_str("hi", super::super::console::COLOR_MIKU));
        let mut buf = [0u8; 32];
        let n = crate::io::tty::master_read(m, &mut buf);
        t.check("ptmx: the master reads what the program wrote",
                &buf[..n] == b"hi",
                format!("read {:?}", core::str::from_utf8(&buf[..n]).unwrap_or("?")));

        // ...and the grid has the same text, from the same write
        let seen = {
            // Sized for the terminal `open_master` makes, 80x24
            let mut c = alloc::vec![0u8; 128 * 48 * CELL];
            match with_grid(m, |g| g.copy_cells(&mut c)).flatten() {
                Some(_) => (0..2).map(|i| cell_char(&c, i)).collect::<String>(),
                None => String::new(),
            }
        };
        t.check("ptmx: and the grid got the same bytes, from the same write",
                seen == "hi",
                format!("the grid reads {:?}", seen));

        // Writing the master is the other side's input, through the discipline
        set_foreground(m, crate::sched::pgid_of(crate::sched::current_pid()));
        crate::io::tty::feed_bytes(m, b"typed\n");
        let mut line = [0u8; 32];
        let got = super::read_input(m, &mut line, 32);
        let text = got.map(|k| core::str::from_utf8(&line[..k]).unwrap_or("?").trim_end());
        t.check("ptmx: writing the master is the program's input",
                text == Some("typed"),
                format!("the program read {:?}", text));
        clear_foreground(m);

        // A second master on the same terminal: on a real machine a `dup` or a
        // `fork`, here the count those go through. `open_master` would make a
        // different terminal
        crate::io::tty::master_ref(slot);
        t.check("ptmx: a second master on the same terminal is counted",
                crate::io::tty::master_count(m) == 2,
                format!("{} masters", crate::io::tty::master_count(m)));
        crate::io::tty::master_unref(slot);
        crate::io::tty::run_hangups();
        t.check("ptmx: closing one of two does not hang the terminal up",
                exists(m) && crate::io::tty::master_count(m) == 1,
                String::from("still there, one master left"));

        // The last one takes it with it
        crate::io::tty::master_unref(slot);
        crate::io::tty::run_hangups();
        t.check("ptmx: closing the last one hangs it up and frees the slot",
                !exists(m),
                format!("{:#x} is gone", m.raw()));
    }

    // It went back to the console when the task's terminal was cleared
    crate::print!("");
    t.check("tty: clearing a task's terminal puts its output back",
            crate::sched::current_tty() == saved,
            format!("current_tty {} again", crate::sched::current_tty()));

    // Listing sees the console and both
    let all = list();
    t.check("tty: the table lists the console and every pty",
            all.len() == 3 && all[0] == SYSTEM && all.contains(&a) && all.contains(&b),
            format!("{} terminals", all.len()));

    // Who the terminal belongs to
    //
    // The job. A keystroke's signal goes to a group, a set of processes.
    //
    // Against a group nobody is in: a real SIGINT at a real foreground would
    // kill the task running the test
    let me = crate::sched::current_pid();
    // The suite can run from a task with no pid of its own, as on the boot
    // path. "Which job am I in" has no answer then, and comparing 0 with 0
    // would pass without meaning
    if me != 0 {
        t.check("job: a process is in a job from the moment it exists",
                crate::sched::pgid_of(me) == me,
                format!("pid {} leads job {}", me, crate::sched::pgid_of(me)));

        let mut members = [0u64; 8];
        let (n, _) = crate::sched::group_members(crate::sched::pgid_of(me), 0, &mut members);
        t.check("job: a job knows its members",
                n >= 1 && members[..n].contains(&me),
                format!("{} member(s), first {}", n, members[0]));
    }

    const NOBODY: u64 = 999_999;
    t.check("job: a signal to an empty job reaches nobody",
            signal::send_to_group(NOBODY, SIGINT) == 0,
            String::from("no process has that group, so nothing was signalled"));

    // SIGKILL cannot be trapped
    t.check("job: SIGKILL cannot be caught",
            !signal::set_disposition(me, SIGKILL, DISP_CATCH)
                && signal::disposition(me, SIGKILL) == DISP_DEFAULT,
            String::from("refused, and left at its default"));

    // A table and not a flag: SIGTERM is catchable without making every other
    // signal so
    let term_before = signal::disposition(me, SIGTERM);
    signal::set_disposition(me, SIGTERM, DISP_CATCH);
    t.check("job: a disposition is per signal",
            signal::disposition(me, SIGTERM) == DISP_CATCH
                && signal::disposition(me, SIGINT) == DISP_DEFAULT,
            String::from("SIGTERM caught, SIGINT still at its default"));
    signal::set_disposition(me, SIGTERM, term_before);

    // A terminal's keys belong to a job, and giving them away is one call
    let j = open_pty(4247, 40, 8).expect("job pty");
    set_foreground(j, NOBODY);
    t.check("job: a terminal's keys belong to a job",
            foreground(j) == NOBODY,
            format!("foreground of {:#x} is {}", j.raw(), foreground(j)));

    // A resize tells the job, and does not kill it
    //
    // SIGWINCH is ignored by default; every signal `default_is_fatal` does not
    // name is fatal, so the foreground here, this very task, must survive to
    // reach the next line
    set_foreground(j, crate::sched::pgid_of(me));
    let reshaped = crate::io::tty::set_size(j, 52, 12);
    t.check("tty: a resize signals the job instead of killing it",
            reshaped && crate::sched::process_alive(me)
                && with_grid(j, |g| g.size()) == Some((52, 12)),
            format!("still running after {:#x} became 52x12", j.raw()));

    // A resize to the shape it has is not a resize. The compositor asks for a
    // size every frame, almost always the one it gave last time
    let quiet = crate::io::tty::set_size(j, 52, 12);
    t.check("tty: asking for the shape it already has changes nothing",
            quiet && with_grid(j, |g| g.size()) == Some((52, 12)),
            String::from("accepted, and the same shape it was"));
    set_foreground(j, NOBODY);

    // Ctrl-C is a signal for that job and not data for whoever is reading. The
    // line is thrown away: what was typed before the interrupt is not delivered
    // afterwards
    feed_char(j, 'a');
    feed_char(j, 'b');
    feed_char(j, '\u{0003}');
    feed_char(j, '\n');
    let mut line = [0u8; 16];
    let took = super::read_input(j, &mut line, 16);
    t.check("job: an interrupt is a signal, not input",
            took == Some(1) && line[0] == b'\n',
            format!("read {:?} after 'ab' and Ctrl-C", took));

    // The editing keys of the discipline
    //
    // Ctrl-W and Tab. These check what a `read` returns, which is what a
    // program sees
    for ch in "cd /usr/loca".chars() { feed_char(j, ch); }
    feed_char(j, '\u{0017}');                 // Ctrl-W: erase "/usr/loca"
    for ch in "/etc\n".chars() { feed_char(j, ch); }
    let mut line = [0u8; 32];
    let took = super::read_input(j, &mut line, 32);
    let text = took.map(|n| core::str::from_utf8(&line[..n]).unwrap_or("?").trim_end());
    t.check("tty: Ctrl-W erases a word, not a character",
            text == Some("cd /etc"),
            format!("read {:?}", text));

    feed_char(j, 'a');
    feed_char(j, '\t');
    feed_char(j, 'b');
    feed_char(j, '\n');
    let mut tabbed = [0u8; 16];
    let n = super::read_input(j, &mut tabbed, 16).unwrap_or(0);
    t.check("tty: a tab is data and reaches the program",
            n == 4 && tabbed[1] == b'\t',
            format!("read {:?}", core::str::from_utf8(&tabbed[..n])));

    clear_foreground(j);
    t.check("job: a terminal with no job in front has no keys to give",
            foreground(j) == 0 && !readable(j),
            String::from("nobody holds it, and nothing is waiting in it"));
    release(j);

    // A hangup takes the owner away but leaves the terminal addressable, so a
    // client still drawing it is told, not reading freed memory
    hangup(a);
    t.check("tty: a hung-up terminal has no owner",
            exists(a) && master_owner(a).is_none(),
            String::from("still addressable, nobody's"));

    // The slot comes back only when the last reference is dropped
    release(a);
    t.check("tty: releasing the last reference frees the slot",
            !exists(a),
            format!("{:#x} is gone", a.raw()));

    // A slot reused under a new id must not answer to the old one: a TtyId
    // names one terminal for ever
    let c = open_pty(4244, 80, 24).expect("third pty");
    t.check("tty: a reused slot does not answer to the old id",
            c.index() == a.index() && c != a && !exists(a) && exists(c),
            format!("slot {} was {:#x}, is now {:#x}", c.index(), a.raw(), c.raw()));

    // A stale id reaches nothing, not whoever took the slot
    t.check("tty: a stale id reads no text",
            with_grid(a, |g| g.size()).is_none()
                && with_grid(c, |g| g.size()) == Some((80, 24)),
            String::from("the old id is refused, the new one answers"));

    // The table is bounded, and running out is a refusal, not a panic
    let mut extra: Vec<TtyId> = Vec::new();
    while let Some(id) = open_pty(4245, 80, 24) {
        extra.push(id);
        if extra.len() > MAX_TTY + 2 { break; }
    }
    t.check("tty: the table is bounded",
            list().len() <= MAX_TTY,
            format!("{} terminals with the table full, ceiling {}", list().len(), MAX_TTY));

    // An empty shape is refused
    for id in extra.drain(..) { release(id); }
    release(b);
    release(c);
    t.check("tty: a terminal with no rows is refused",
            open_pty(1, 0, 24).is_none() && open_pty(1, 80, 0).is_none(),
            String::from("zero columns and zero rows both refused"));

    // The table is back to just the console, so a second run says the same as
    // the first
    let left = list();
    t.check("tty: the run leaves the table as it found it",
            left.len() == 1 && left[0] == SYSTEM,
            format!("{} terminal(s) left", left.len()));

    tally(&t)
}

/// The character out of cell `i` of a copied grid
///
/// A little-endian `u32` then the colour; `protocol::console::CELL_BYTES` says
/// how far apart they are
fn cell_char(buf: &[u8], i: usize) -> char {
    let at = i * CELL;
    if at + 4 > buf.len() { return '?'; }
    let cp = u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]);
    char::from_u32(cp).unwrap_or('?')
}

/// A thread that shows it is running
///
/// The counter tells "stopped" from "running but idle" from outside: a state
/// byte says what the scheduler believes, this says what the processor did
static STOP_TICKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn stop_spinner() -> ! {
    loop {
        STOP_TICKS.fetch_add(1, Ordering::Relaxed);
        crate::sched::yield_now();
    }
}

fn tally(t: &Runner) -> (usize, usize) {
    let pass = t.cases.iter().filter(|c| c.ok).count();
    (pass, t.cases.len() - pass)
}

/// `tty test`
pub fn cmd_selftest() {
    let (pass, fail) = run();
    crate::serial_println!("[tty-test] {} passed, {} failed", pass, fail);
    if fail == 0 {
        crate::print_success!("tty: {} passed", pass);
    } else {
        crate::print_error!("tty: {} passed, {} FAILED", pass, fail);
    }
}
