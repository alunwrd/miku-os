#![no_std]
#![no_main]

//! msh: the ring-3 shell
//!
//! The kernel keeps commands needing ring 0 (`nvidia`, the block layer, `mkfs`,
//! selftests). An unknown command is looked up in `/bin`.
//!
//! Jobs: every command runs in its own process group, which gets the terminal's
//! keys while it runs (`setpgid`, `tty::foreground`), so Ctrl-C reaches it.
//!
//! Line editing is in `edit.rs`.
//!
//! Grammar, after `'`, `"`, and `\` resolve into words:
//!
//! ```text
//! line     := list ( (';' | '&') list )* [';' | '&']
//! list     := pipeline ( ('&&' | '||') pipeline )*
//! pipeline := command ( '|' command )*
//! command  := word+ ( '<' w | '>' w | '>>' w | '2>' w | '2>>' w
//!                   | [n]'>&'m )*        n, m single digits; n is 1 if absent
//! ```
//!
//! No variables, `$(...)`, globbing, functions, `if`, or `for`.

extern crate alloc;

pub use miku;

mod edit;

use miku::{eprintln, exit, print, println};

/// The longest line the terminal will hand over in one go
pub const LINE_MAX: usize = 1024;
/// The most words one command may have, argv[0] included
const MAX_ARGS: usize = 16;
/// The longest a single word may be, with room for its terminator
const WORD_MAX: usize = 128;
const PATH_MAX: usize = 256;
/// The most commands one pipeline may have
const MAX_CMDS: usize = 4;
/// The most words and operators one line may be made of
const MAX_TOKS: usize = 64;
/// The most jobs that may be running in the background at once
const MAX_JOBS: usize = 8;
/// The most `n>&m` one command may carry
const MAX_DUPS: usize = 4;

// Open flags, as `kernel/fs/vfs/types.rs` spells them
const O_READ: u32 = miku_abi::fcntl::O_RDONLY as u32;
const O_WRITE: u32 = miku_abi::fcntl::O_WRONLY as u32;
const O_APPEND: u32 = miku_abi::fcntl::O_APPEND as u32;
const O_CREATE: u32 = miku_abi::fcntl::O_CREAT as u32;
const O_TRUNC: u32 = miku_abi::fcntl::O_TRUNC as u32;

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);

    // `msh -c 'line'` and `msh script` print no banner or prompt and exit with
    // the last status. `-l` and `-i` are accepted and ignored
    let mut first = 1;
    while first < args.len() && matches!(args[first], "-l" | "-i" | "-li" | "-il" | "--login") {
        first += 1;
    }
    if args.len() > first && args[first] == "-c" {
        if args.len() < first + 2 {
            eprintln("msh: -c wants a command after it");
            exit(2);
        }
        exit(run_line(args[first + 1]));
    }
    if args.len() > first {
        exit(run_script(args[first]));
    }

    // Banner first, before anything can fail, so a window shows the program ran
    println("");
    print("MikuOS - tty ");
    miku::print_hex(miku::tty::current() as u64);
    print(", job ");
    miku::print_hex(miku::proc::getpgid(0));
    println("");

    // Parts that read files
    print("welcome, ");
    print(who());
    println(" - 'help' for what this shell knows :)");

    let mut line = [0u8; LINE_MAX];
    loop {
        // Announces finished background jobs between prompts and reaps them
        jobs::reap();
        // The editor prints the prompt, redrawing it each keystroke
        let n = edit::read_line(&mut line, prompt);
        if n < 0 {
            // The terminal is gone
            eprintln("msh: the terminal has gone");
            exit(0);
        }
        if n == 0 {
            // End of input: Ctrl-D, or a terminal whose keys belong to another
            // job. Reported rather than silent
            let fg = miku::tty::foreground_of().unwrap_or(0);
            let mine = miku::proc::getpgid(0);
            if fg != mine {
                print("msh: this terminal's keys belong to job ");
                miku::print_hex(fg);
                print(", not to mine (");
                miku::print_hex(mine);
                println(") - nothing to read, ending");
            } else {
                println("");
            }
            exit(0);
        }
        let text = core::str::from_utf8(&line[..n as usize]).unwrap_or("");
        let _ = run_line(text.trim_end_matches(['\n', '\r']));
    }
}

/// `miku@os:/ $` in the kernel console's colors. Returns its width in columns,
/// since the escapes take no cell
fn prompt() -> usize {
    let mut cols = 0usize;
    // Text and its screen width
    let mut text = |s: &str| {
        print(s);
        cols += s.chars().count();
    };
    let esc = |s: &str| print(s);

    let uid = unsafe { miku::miku_getuid() };
    // Red for root, blue otherwise
    if uid == 0 {
        esc("\x1b[38;2;255;120;120m");
    } else {
        esc("\x1b[38;2;100;160;255m");
    }
    text(who());
    esc("\x1b[38;2;150;160;170m");
    text("@");
    esc("\x1b[38;2;255;255;255m");
    text("os");
    esc("\x1b[38;2;150;160;170m");
    text(":");

    esc("\x1b[38;2;57;197;187m");
    let mut cwd = [0u8; PATH_MAX];
    text(getcwd(&mut cwd).unwrap_or("?"));

    // `#` for root, `$` for everybody else. Thirty years of muscle memory
    esc("\x1b[38;2;255;255;255m");
    text(if uid == 0 { " # " } else { " $ " });
    // Restores the terminal's colors
    esc("\x1b[0m");
    cols
}

/// User name through `mikuauth::db`, falling back to the uid
fn who() -> &'static str {
    use core::sync::atomic::{AtomicBool, Ordering};
    static LOOKED: AtomicBool = AtomicBool::new(false);
    static mut NAME: [u8; 34] = [0; 34];
    static mut LEN: usize = 0;

    unsafe {
        let slot = &mut *core::ptr::addr_of_mut!(NAME);
        let len = &mut *core::ptr::addr_of_mut!(LEN);
        if !LOOKED.swap(true, Ordering::Relaxed) {
            let uid = miku::miku_getuid() as u16;
            let text = miku::read_file_string(mikuauth::PASSWD_PATH).unwrap_or_default();
            let name = match mikuauth::db::find_uid(&text, uid) {
                Some(p) => alloc::string::String::from(p.name),
                None => {
                    let mut n = alloc::string::String::new();
                    let mut v = uid;
                    let mut digits = [0u8; 5];
                    let mut i = 5;
                    loop {
                        i -= 1;
                        digits[i] = b'0' + (v % 10) as u8;
                        v /= 10;
                        if v == 0 {
                            break;
                        }
                    }
                    for b in &digits[i..] {
                        n.push(*b as char);
                    }
                    n
                }
            };
            let bytes = name.as_bytes();
            *len = bytes.len().min(slot.len());
            slot[..*len].copy_from_slice(&bytes[..*len]);
        }
        core::str::from_utf8(&slot[..*len]).unwrap_or("?")
    }
}

/// Runs a line. Returns the last status, used by `&&`, `msh -c`, and scripts
fn run_line(line: &str) -> i64 {
    let line = line.trim();
    if line.is_empty() {
        return 0;
    }

    // Word storage for the line's duration; nothing is allocated before a
    // program starts
    let mut arena = [0u8; LINE_MAX];
    let mut raw = [Raw::Op(Op::Semi); MAX_TOKS];
    let count = match tokenize(line, &mut arena, &mut raw) {
        Ok(n) => n,
        Err(msg) => {
            eprintln(msg);
            return 2;
        }
    };
    if count == 0 {
        return 0;
    }

    // Offsets become slices once the arena stops moving
    let mut toks = [Tok::Op(Op::Semi); MAX_TOKS];
    for i in 0..count {
        toks[i] = match raw[i] {
            Raw::Word(off, len) => {
                Tok::Word(core::str::from_utf8(&arena[off..off + len]).unwrap_or(""))
            }
            Raw::Op(o) => Tok::Op(o),
        };
    }

    run_sequence(&toks[..count])
}

/// Runs each line of a file. `#!` is a comment here
fn run_script(path: &str) -> i64 {
    let Some(text) = miku::read_file_string(path) else {
        print("msh: ");
        print(path);
        println(": cannot read");
        return 127;
    };
    let mut status = 0i64;
    for line in text.lines() {
        status = run_line(line);
    }
    status
}

// Words

/// What a line is made of, once the quotes have been taken off
#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Pipe,
    AndAnd,
    OrOr,
    Semi,
    Amp,
    Lt,
    Gt,
    GtGt,
    ErrGt,
    ErrGtGt,
    /// `n>&m`: descriptor n goes where m goes
    Dup(u8, u8),
}

/// A token before its word becomes a slice
#[derive(Clone, Copy)]
enum Raw {
    Word(usize, usize),
    Op(Op),
}

#[derive(Clone, Copy)]
enum Tok<'a> {
    Word(&'a str),
    Op(Op),
}

/// Operator at `i` and its length. `at_start` separates `2>` as a redirection
/// from two ordinary characters (`x2>f`)
fn op_at(b: &[u8], i: usize, at_start: bool) -> Option<(Op, usize)> {
    let rest = &b[i..];
    let digit = |at: usize| {
        rest.get(at)
            .filter(|c| c.is_ascii_digit())
            .map(|c| c - b'0')
    };
    if at_start {
        if let (Some(n), Some(b'>'), Some(b'&'), Some(m)) =
            (digit(0), rest.get(1), rest.get(2), digit(3))
        {
            return Some((Op::Dup(n, m), 4));
        }
        if rest.starts_with(b"2>>") {
            return Some((Op::ErrGtGt, 3));
        }
        if rest.starts_with(b"2>") {
            return Some((Op::ErrGt, 2));
        }
    }
    if let (Some(b'>'), Some(b'&'), Some(m)) = (rest.first(), rest.get(1), digit(2)) {
        return Some((Op::Dup(1, m), 3));
    }
    if rest.starts_with(b"&&") {
        return Some((Op::AndAnd, 2));
    }
    if rest.starts_with(b"||") {
        return Some((Op::OrOr, 2));
    }
    if rest.starts_with(b">>") {
        return Some((Op::GtGt, 2));
    }
    if rest.starts_with(b">") {
        return Some((Op::Gt, 1));
    }
    if rest.starts_with(b"<") {
        return Some((Op::Lt, 1));
    }
    if rest.starts_with(b"|") {
        return Some((Op::Pipe, 1));
    }
    if rest.starts_with(b"&") {
        return Some((Op::Amp, 1));
    }
    if rest.starts_with(b";") {
        return Some((Op::Semi, 1));
    }
    None
}

/// Splits a line into words and operators, resolving quotes:
/// * `'...'` is literal.
/// * `"..."` is literal except `\` escapes `"`, `\`, `$`, and a backtick.
/// * `\` outside quotes makes the next character ordinary.
/// Quotes are removed. An unclosed quote is refused
fn tokenize(
    line: &str,
    arena: &mut [u8; LINE_MAX],
    out: &mut [Raw; MAX_TOKS],
) -> Result<usize, &'static str> {
    let b = line.as_bytes();
    let mut i = 0usize;
    let mut used = 0usize;
    let mut n = 0usize;

    // Adds a character to the arena, checked though unquoting only shortens
    fn push(arena: &mut [u8; LINE_MAX], used: &mut usize, c: u8) -> Result<(), &'static str> {
        if *used == arena.len() {
            return Err("msh: the line is too long");
        }
        arena[*used] = c;
        *used += 1;
        Ok(())
    }

    while i < b.len() {
        while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        // A `#` at the start of a word begins a comment
        if b[i] == b'#' {
            break;
        }
        if n == out.len() {
            return Err("msh: too many words in one line");
        }

        if let Some((op, len)) = op_at(b, i, true) {
            out[n] = Raw::Op(op);
            n += 1;
            i += len;
            continue;
        }

        let start = used;
        while i < b.len() {
            let c = b[i];
            if c == b' ' || c == b'\t' {
                break;
            }
            if op_at(b, i, false).is_some() {
                break;
            }
            match c {
                b'\'' => {
                    i += 1;
                    loop {
                        if i >= b.len() {
                            return Err("msh: a ' with no closing '");
                        }
                        if b[i] == b'\'' {
                            i += 1;
                            break;
                        }
                        push(arena, &mut used, b[i])?;
                        i += 1;
                    }
                }
                b'"' => {
                    i += 1;
                    loop {
                        if i >= b.len() {
                            return Err("msh: a \" with no closing \"");
                        }
                        if b[i] == b'"' {
                            i += 1;
                            break;
                        }
                        if b[i] == b'\\'
                            && i + 1 < b.len()
                            && matches!(b[i + 1], b'"' | b'\\' | b'$' | b'`')
                        {
                            i += 1;
                        }
                        push(arena, &mut used, b[i])?;
                        i += 1;
                    }
                }
                b'\\' => {
                    i += 1;
                    if i >= b.len() {
                        return Err("msh: a \\ with nothing after it");
                    }
                    push(arena, &mut used, b[i])?;
                    i += 1;
                }
                _ => {
                    push(arena, &mut used, c)?;
                    i += 1;
                }
            }
        }
        out[n] = Raw::Word(start, used - start);
        n += 1;
    }
    Ok(n)
}

// Grammar

/// `a ; b ; c &`: lists in sequence, waited for or not
fn run_sequence(toks: &[Tok]) -> i64 {
    let mut status = 0i64;
    let mut i = 0usize;
    while i <= toks.len() {
        let mut j = i;
        while j < toks.len() && !matches!(toks[j], Tok::Op(Op::Semi) | Tok::Op(Op::Amp)) {
            j += 1;
        }
        let background = matches!(toks.get(j), Some(Tok::Op(Op::Amp)));
        if j > i {
            status = run_list(&toks[i..j], background);
        }
        if j >= toks.len() {
            break;
        }
        i = j + 1;
    }
    status
}

/// `a && b || c`: each pipeline per the last status. A single-pipeline list
/// ending in `&` is backgrounded as itself; a longer one forks a copy of this
/// shell to watch it
fn run_list(toks: &[Tok], background: bool) -> i64 {
    let chained = toks
        .iter()
        .any(|t| matches!(t, Tok::Op(Op::AndAnd) | Tok::Op(Op::OrOr)));

    if background && chained {
        let pid = unsafe { miku::miku_fork() };
        if pid < 0 {
            eprintln("msh: cannot fork");
            return 127;
        }
        if pid == 0 {
            let _ = miku::proc::setpgid(0, 0);
            exit(run_chain(toks));
        }
        let _ = miku::proc::setpgid(pid as u64, pid as u64);
        jobs::add(pid as u64, pid as u64);
        return 0;
    }
    if background {
        return run_pipeline_toks(toks, true);
    }
    run_chain(toks)
}

fn run_chain(toks: &[Tok]) -> i64 {
    let mut status = 0i64;
    let mut start = 0usize;
    let mut pending: Option<Op> = None;
    let mut i = 0usize;
    loop {
        let end = i == toks.len();
        let here = !end && matches!(toks[i], Tok::Op(Op::AndAnd) | Tok::Op(Op::OrOr));
        if end || here {
            // A skipped pipeline leaves the status unchanged
            let skip = match pending {
                Some(Op::AndAnd) => status != 0,
                Some(Op::OrOr) => status == 0,
                _ => false,
            };
            if !skip {
                if toks[start..i].is_empty() {
                    eprintln("msh: nothing between && and ||");
                    return 2;
                }
                status = run_pipeline_toks(&toks[start..i], false);
            }
            if end {
                return status;
            }
            pending = match toks[i] {
                Tok::Op(o) => Some(o),
                _ => None,
            };
            start = i + 1;
        }
        i += 1;
    }
}

/// `a | b | c`: one pipeline's tokens, parsed and run
fn run_pipeline_toks(toks: &[Tok], background: bool) -> i64 {
    let mut cmds = [Cmd::empty(); MAX_CMDS];
    let count = match parse_cmds(toks, &mut cmds) {
        Ok(n) => n,
        Err(msg) => {
            eprintln(msg);
            return 2;
        }
    };
    if count == 0 {
        return 0;
    }

    // `cd`, `exit`, and job builtins run in this shell when the command is
    // alone with no redirection or pipe
    if count == 1 && !background && cmds[0].plain() {
        if let Some(status) = builtin_in_shell(&cmds[0]) {
            return status;
        }
    }

    run_pipeline(&cmds[..count], background)
}

/// One command: its words and redirections
#[derive(Clone, Copy)]
struct Cmd<'a> {
    words: [&'a str; MAX_ARGS],
    n: usize,
    infile: Option<&'a str>,
    outfile: Option<&'a str>,
    append: bool,
    errfile: Option<&'a str>,
    err_append: bool,
    /// `n>&m`, as (n, m), in the order written
    dups: [(u8, u8); MAX_DUPS],
    n_dups: usize,
}

impl<'a> Cmd<'a> {
    const fn empty() -> Self {
        Cmd {
            words: [""; MAX_ARGS],
            n: 0,
            infile: None,
            outfile: None,
            append: false,
            errfile: None,
            err_append: false,
            dups: [(0, 0); MAX_DUPS],
            n_dups: 0,
        }
    }
    /// No redirection, so a builtin may run in the shell
    fn plain(&self) -> bool {
        self.infile.is_none()
            && self.outfile.is_none()
            && self.errfile.is_none()
            && self.n_dups == 0
    }
}

/// Parses one pipeline's tokens into commands. An error runs nothing
fn parse_cmds<'a>(toks: &[Tok<'a>], out: &mut [Cmd<'a>]) -> Result<usize, &'static str> {
    let mut count = 0usize;
    let mut cmd = Cmd::empty();
    let mut anything = false;
    // A redirection that has been read and is waiting for its file
    let mut pending: Option<Op> = None;

    for t in toks {
        match *t {
            Tok::Word(w) => {
                match pending.take() {
                    Some(Op::Lt) => cmd.infile = Some(w),
                    Some(Op::Gt) => {
                        cmd.outfile = Some(w);
                        cmd.append = false;
                    }
                    Some(Op::GtGt) => {
                        cmd.outfile = Some(w);
                        cmd.append = true;
                    }
                    Some(Op::ErrGt) => {
                        cmd.errfile = Some(w);
                        cmd.err_append = false;
                    }
                    Some(Op::ErrGtGt) => {
                        cmd.errfile = Some(w);
                        cmd.err_append = true;
                    }
                    _ => {
                        if cmd.n == MAX_ARGS {
                            return Err("msh: too many arguments");
                        }
                        cmd.words[cmd.n] = w;
                        cmd.n += 1;
                    }
                }
                anything = true;
            }
            Tok::Op(Op::Dup(n, m)) => {
                if cmd.n_dups == MAX_DUPS {
                    return Err("msh: too many descriptor redirections");
                }
                cmd.dups[cmd.n_dups] = (n, m);
                cmd.n_dups += 1;
                anything = true;
            }
            Tok::Op(Op::Pipe) => {
                if pending.is_some() {
                    return Err("msh: a redirection with no file after it");
                }
                if cmd.n == 0 {
                    return Err("msh: an empty command in the pipeline");
                }
                if count == out.len() {
                    return Err("msh: too many commands in one pipeline");
                }
                out[count] = cmd;
                count += 1;
                cmd = Cmd::empty();
                anything = false;
            }
            Tok::Op(o @ (Op::Lt | Op::Gt | Op::GtGt | Op::ErrGt | Op::ErrGtGt)) => {
                if pending.is_some() {
                    return Err("msh: two redirections with no file between them");
                }
                pending = Some(o);
                anything = true;
            }
            // `;`, `&`, `&&`, and `||` were cut earlier, so this is an internal
            // error
            Tok::Op(_) => return Err("msh: a stray ; & && or || in a pipeline"),
        }
    }

    if pending.is_some() {
        return Err("msh: a redirection with no file after it");
    }
    if cmd.n == 0 {
        if anything || count > 0 {
            return Err("msh: an empty command in the pipeline");
        }
        return Ok(count);
    }
    if count == out.len() {
        return Err("msh: too many commands in one pipeline");
    }
    out[count] = cmd;
    count += 1;
    Ok(count)
}

/// Starts every command in one process group, gives it the terminal, and waits.
/// The shell closes every pipe end, or readers would never see EOF.
/// `background` skips the terminal handoff
fn run_pipeline(cmds: &[Cmd], background: bool) -> i64 {
    let n = cmds.len();
    let mut pipes = [[-1i64; 2]; MAX_CMDS];
    for i in 0..n.saturating_sub(1) {
        let mut fds = [0i64; 2];
        if unsafe { miku::miku_pipe(fds.as_mut_ptr()) } < 0 {
            eprintln("msh: cannot make a pipe");
            close_pipes(&mut pipes, n);
            return 127;
        }
        pipes[i] = fds;
    }

    let mut leader: u64 = 0;
    let mut kids = [0u64; MAX_CMDS];
    let mut started = 0usize;

    for i in 0..n {
        let pid = unsafe { miku::miku_fork() };
        if pid < 0 {
            eprintln("msh: cannot fork");
            break;
        }
        if pid == 0 {
            // `leader` is 0 in the first child and its pid in the rest. Set on
            // both sides of the fork to close the window
            let _ = miku::proc::setpgid(0, leader);
            child(&cmds[i], &pipes, i, n);
        }
        if leader == 0 {
            leader = pid as u64;
        }
        let _ = miku::proc::setpgid(pid as u64, leader);
        kids[i] = pid as u64;
        started += 1;
    }

    close_pipes(&mut pipes, n);

    if started == 0 {
        return 127;
    }

    if background {
        jobs::add(leader, kids[started - 1]);
        return 0;
    }

    // The keys belong to the job while it runs
    let _ = miku::tty::foreground(leader);
    let mut status = 0i64;
    for (i, k) in kids.iter().take(started).enumerate() {
        // `WUNTRACED`, so a stopped child returns here and Ctrl-Z works
        match miku::proc::wait_job(*k, true) {
            Some((_, st)) if miku::proc::stopped(st) => {
                // Stopped: takes the terminal back, adds the job to the table,
                // and stops waiting
                let _ = miku::tty::foreground(0);
                jobs::add_stopped(leader, kids[started - 1]);
                return 128 + miku::proc::stop_sig(st);
            }
            Some((_, st)) => {
                // A pipeline's status is its last command's
                if i + 1 == started {
                    status = miku::proc::wait_status_code(st);
                }
            }
            None => {
                if i + 1 == started {
                    status = 127;
                }
            }
        }
    }
    // Takes the terminal back in every case
    let _ = miku::tty::foreground(0);
    status
}

/// Builtins meaningful only in this shell. `None` lets the caller fork. `echo`,
/// `pwd`, `tty`, and `help` run in a child
fn builtin_in_shell(cmd: &Cmd) -> Option<i64> {
    match cmd.words[0] {
        "exit" | "quit" => {
            let code = if cmd.n > 1 {
                parse_i64(cmd.words[1]).unwrap_or(0)
            } else {
                0
            };
            exit(code);
        }
        "cd" => Some(builtin_cd(if cmd.n > 1 { cmd.words[1] } else { "/" })),
        "jobs" => {
            jobs::list();
            Some(0)
        }
        "wait" => {
            jobs::wait_all();
            Some(0)
        }
        // Job number, or the current one. Must run in the shell
        "fg" => {
            let n = if cmd.n > 1 {
                parse_i64(cmd.words[1]).unwrap_or(0).max(0) as usize
            } else {
                0
            };
            Some(jobs::fg(n))
        }
        "bg" => {
            let n = if cmd.n > 1 {
                parse_i64(cmd.words[1]).unwrap_or(0).max(0) as usize
            } else {
                0
            };
            Some(jobs::bg(n))
        }
        _ => None,
    }
}

// Background job table, with `fg` and `bg`
mod jobs {
    use super::{print, println, MAX_JOBS};

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum State {
        Running,
        Stopped,
    }

    #[derive(Clone, Copy)]
    struct Job {
        num: usize,
        pgid: u64,
        /// The last process of the pipeline, whose ending is the job's
        last: u64,
        live: bool,
        state: State,
    }

    static mut TABLE: [Job; MAX_JOBS] = [Job {
        num: 0,
        pgid: 0,
        last: 0,
        live: false,
        state: State::Running,
    }; MAX_JOBS];
    static mut NEXT: usize = 1;
    /// Default job for `fg` and `bg`: the most recent to stop or start
    static mut CURRENT: usize = 0;

    fn table() -> &'static mut [Job; MAX_JOBS] {
        unsafe { &mut *core::ptr::addr_of_mut!(TABLE) }
    }

    /// Adds a started job and reports it
    pub fn add(pgid: u64, last: u64) {
        let num = unsafe {
            let n = &mut *core::ptr::addr_of_mut!(NEXT);
            let v = *n;
            *n += 1;
            v
        };
        for slot in table().iter_mut() {
            if !slot.live {
                *slot = Job {
                    num,
                    pgid,
                    last,
                    live: true,
                    state: State::Running,
                };
                break;
            }
        }
        print("[");
        super::miku::print_hex(num as u64);
        print("] ");
        super::miku::print_hex(pgid);
        println("");
    }

    /// Reaps every finished child without waiting, including pipeline members
    /// outside the table
    pub fn reap() {
        while let Some(pid) = super::miku::proc::try_wait_any() {
            for slot in table().iter_mut() {
                if slot.live && slot.last == pid {
                    slot.live = false;
                    print("[");
                    super::miku::print_hex(slot.num as u64);
                    print("] done  ");
                    super::miku::print_hex(pid);
                    println("");
                }
            }
        }
    }

    /// Adds a stopped job, possibly already in the table
    pub fn add_stopped(pgid: u64, last: u64) {
        for slot in table().iter_mut() {
            if slot.live && slot.pgid == pgid {
                slot.state = State::Stopped;
                set_current(slot.num);
                report(slot.num, pgid, "stopped");
                return;
            }
        }
        let num = next_num();
        for slot in table().iter_mut() {
            if !slot.live {
                *slot = Job {
                    num,
                    pgid,
                    last,
                    live: true,
                    state: State::Stopped,
                };
                break;
            }
        }
        set_current(num);
        report(num, pgid, "stopped");
    }

    fn next_num() -> usize {
        unsafe {
            let n = &mut *core::ptr::addr_of_mut!(NEXT);
            let v = *n;
            *n += 1;
            v
        }
    }

    fn set_current(num: usize) {
        unsafe {
            *core::ptr::addr_of_mut!(CURRENT) = num;
        }
    }

    fn current() -> usize {
        unsafe { *core::ptr::addr_of!(CURRENT) }
    }

    fn report(num: usize, pgid: u64, what: &str) {
        print("[");
        super::miku::print_hex(num as u64);
        print("] ");
        print(what);
        print("  ");
        super::miku::print_hex(pgid);
        println("");
    }

    /// The job a bare `fg` or `bg` means, or the one a number names
    fn find(num: usize) -> Option<(u64, u64, State)> {
        let want = if num == 0 { current() } else { num };
        for slot in table().iter() {
            if slot.live && slot.num == want {
                return Some((slot.pgid, slot.last, slot.state));
            }
        }
        None
    }

    fn mark(pgid: u64, state: State) {
        for slot in table().iter_mut() {
            if slot.live && slot.pgid == pgid {
                slot.state = state;
            }
        }
    }

    fn forget(pgid: u64) {
        for slot in table().iter_mut() {
            if slot.live && slot.pgid == pgid {
                slot.live = false;
            }
        }
    }

    /// Foregrounds a job and waits. The terminal goes first and SIGCONT second,
    /// or a woken job reads from a terminal that is not its own
    pub fn fg(num: usize) -> i64 {
        let Some((pgid, last, _)) = find(num) else {
            super::eprintln("fg: no such job");
            return 1;
        };
        let _ = super::miku::tty::foreground(pgid);
        let _ = super::miku::proc::killpg(pgid, super::miku::proc::SIGCONT);
        mark(pgid, State::Running);
        let status = match super::miku::proc::wait_job(last, true) {
            Some((_, st)) if super::miku::proc::stopped(st) => {
                let _ = super::miku::tty::foreground(0);
                mark(pgid, State::Stopped);
                set_current(num_of(pgid));
                report(num_of(pgid), pgid, "stopped");
                return 128 + super::miku::proc::stop_sig(st);
            }
            Some((_, st)) => super::miku::proc::wait_status_code(st),
            None => 127,
        };
        let _ = super::miku::tty::foreground(0);
        forget(pgid);
        status
    }

    /// Let a stopped job carry on without the terminal
    pub fn bg(num: usize) -> i64 {
        let Some((pgid, _, state)) = find(num) else {
            super::eprintln("bg: no such job");
            return 1;
        };
        if state == State::Running {
            super::eprintln("bg: that job is already running");
            return 1;
        }
        let _ = super::miku::proc::killpg(pgid, super::miku::proc::SIGCONT);
        mark(pgid, State::Running);
        report(num_of(pgid), pgid, "continued");
        0
    }

    fn num_of(pgid: u64) -> usize {
        for slot in table().iter() {
            if slot.live && slot.pgid == pgid {
                return slot.num;
            }
        }
        0
    }

    pub fn list() {
        let mut any = false;
        let cur = current();
        for slot in table().iter() {
            if !slot.live {
                continue;
            }
            any = true;
            print("[");
            super::miku::print_hex(slot.num as u64);
            print(if slot.num == cur { "]+ " } else { "]  " });
            print(match slot.state {
                State::Running => "running  ",
                State::Stopped => "stopped  ",
            });
            super::miku::print_hex(slot.pgid);
            println("");
        }
        if !any {
            println("no jobs in the background");
        }
    }

    /// Waits for every background job
    pub fn wait_all() {
        loop {
            let mut waiting = false;
            for slot in table().iter() {
                if slot.live {
                    waiting = true;
                }
            }
            if !waiting {
                return;
            }
            // Reports stopped jobs rather than waiting on them forever
            for slot in table().iter() {
                if slot.live && slot.state == State::Stopped {
                    println("wait: a job is stopped - 'fg' or 'bg' it first");
                    return;
                }
            }
            let mut status: i64 = 0;
            let pid = unsafe { super::miku::miku_waitpid(u64::MAX, &mut status as *mut i64) };
            if pid <= 0 {
                // Nothing left to wait for, whatever the table thinks
                for slot in table().iter_mut() {
                    slot.live = false;
                }
                return;
            }
            for slot in table().iter_mut() {
                if slot.live && slot.last == pid as u64 {
                    slot.live = false;
                    print("[");
                    super::miku::print_hex(slot.num as u64);
                    print("] done  ");
                    super::miku::print_hex(pid as u64);
                    println("");
                }
            }
        }
    }
}

fn close_pipes(pipes: &mut [[i64; 2]; MAX_CMDS], n: usize) {
    for i in 0..n.saturating_sub(1) {
        if pipes[i][0] >= 0 {
            miku::close(pipes[i][0]);
        }
        if pipes[i][1] >= 0 {
            miku::close(pipes[i][1]);
        }
        pipes[i] = [-1, -1];
    }
}

/// Child side: wires redirections and pipes, then execs. Never returns; any
/// failure ends the process
fn child(cmd: &Cmd, pipes: &[[i64; 2]; MAX_CMDS], i: usize, n: usize) -> ! {
    // Pipeline first, so an explicit `<` or `>` wins
    if i > 0 {
        let _ = unsafe { miku::miku_dup2(pipes[i - 1][0], 0) };
    }
    if i + 1 < n {
        let _ = unsafe { miku::miku_dup2(pipes[i][1], 1) };
    }
    // Closes every pipe copy, or the originals would keep the pipe open
    for j in 0..n.saturating_sub(1) {
        if pipes[j][0] >= 0 {
            miku::close(pipes[j][0]);
        }
        if pipes[j][1] >= 0 {
            miku::close(pipes[j][1]);
        }
    }

    if let Some(path) = cmd.infile {
        let mut buf = [0u8; PATH_MAX];
        let p = match cstr(&mut buf, path) {
            Some(p) => p,
            None => {
                eprintln("msh: path too long");
                exit(1);
            }
        };
        let fd = unsafe { miku::miku_open(p, path.len(), O_READ, 0) };
        if fd < 0 {
            redirect_failed(path);
        }
        let _ = unsafe { miku::miku_dup2(fd, 0) };
        miku::close(fd);
    }
    if let Some(path) = cmd.outfile {
        let mut buf = [0u8; PATH_MAX];
        let p = match cstr(&mut buf, path) {
            Some(p) => p,
            None => {
                eprintln("msh: path too long");
                exit(1);
            }
        };
        let flags = if cmd.append {
            O_WRITE | O_CREATE | O_APPEND
        } else {
            O_WRITE | O_CREATE | O_TRUNC
        };
        let fd = unsafe { miku::miku_open(p, path.len(), flags, 0o644) };
        if fd < 0 {
            redirect_failed(path);
        }
        let _ = unsafe { miku::miku_dup2(fd, 1) };
        miku::close(fd);
    }
    // Redirects the error stream
    if let Some(path) = cmd.errfile {
        let mut buf = [0u8; PATH_MAX];
        let p = match cstr(&mut buf, path) {
            Some(p) => p,
            None => {
                eprintln("msh: path too long");
                exit(1);
            }
        };
        let flags = if cmd.err_append {
            O_WRITE | O_CREATE | O_APPEND
        } else {
            O_WRITE | O_CREATE | O_TRUNC
        };
        let fd = unsafe { miku::miku_open(p, path.len(), flags, 0o644) };
        if fd < 0 {
            redirect_failed(path);
        }
        let _ = unsafe { miku::miku_dup2(fd, 2) };
        miku::close(fd);
    }
    // Duplications last, after files are open. Applied in a fixed order, so
    // `> out 2>&1` and `2>&1 > out` mean the same, unlike POSIX
    for &(n, m) in &cmd.dups[..cmd.n_dups] {
        let _ = unsafe { miku::miku_dup2(m as i64, n as i64) };
    }

    // A pipeline builtin runs in the child
    match cmd.words[0] {
        "echo" => {
            for j in 1..cmd.n {
                if j > 1 {
                    print(" ");
                }
                print(cmd.words[j]);
            }
            println("");
            exit(0);
        }
        "pwd" => {
            let mut cwd = [0u8; PATH_MAX];
            println(getcwd(&mut cwd).unwrap_or("?"));
            exit(0);
        }
        "tty" => {
            report_tty();
            exit(0);
        }
        "help" => {
            help();
            exit(0);
        }
        _ => {}
    }

    exec_or_die(cmd);
}

fn redirect_failed(path: &str) -> ! {
    print("msh: ");
    print(path);
    println(": cannot open");
    exit(1);
}

/// Execs `cmd.words[0]` from `/bin`, or reports why not. There is no PATH
fn exec_or_die(cmd: &Cmd) -> ! {
    // `NAME=value` words before the program go into the child's environment
    let first = cmd.words[..cmd.n]
        .iter()
        .take_while(|w| is_assignment(w))
        .count();
    if first == cmd.n {
        eprintln("msh: NAME=value sets a program's environment, and no program follows");
        exit(1);
    }
    for w in &cmd.words[..first] {
        let (key, val) = w.split_once('=').unwrap_or((w, ""));
        let mut kb = [0u8; WORD_MAX];
        let mut vb = [0u8; WORD_MAX];
        match (cstr(&mut kb, key), cstr(&mut vb, val)) {
            (Some(k), Some(v)) => {
                unsafe { miku::miku_setenv(k, v) };
            }
            _ => {
                eprintln("msh: assignment too long");
                exit(127);
            }
        }
    }
    let name = cmd.words[first];

    let mut path_buf = [0u8; PATH_MAX];
    let path = if name.contains('/') {
        match cstr(&mut path_buf, name) {
            Some(p) => p,
            None => {
                eprintln("msh: path too long");
                exit(127);
            }
        }
    } else {
        let prefix = b"/bin/";
        if prefix.len() + name.len() + 1 > path_buf.len() {
            eprintln("msh: name too long");
            exit(127);
        }
        path_buf[..prefix.len()].copy_from_slice(prefix);
        path_buf[prefix.len()..prefix.len() + name.len()].copy_from_slice(name.as_bytes());
        path_buf[prefix.len() + name.len()] = 0;
        path_buf.as_ptr()
    };

    // argv on the stack, with no allocation
    let mut arg_store = [[0u8; WORD_MAX]; MAX_ARGS];
    let mut argv = [core::ptr::null::<u8>(); MAX_ARGS];
    let mut argc = 0usize;
    for i in first..cmd.n {
        match cstr(&mut arg_store[i], cmd.words[i]) {
            Some(p) => {
                argv[argc] = p;
                argc += 1;
            }
            None => {
                eprintln("msh: argument too long");
                exit(127);
            }
        }
    }

    // With this shell's environment and the assignments
    let _ = unsafe {
        let envp = miku::miku_environ() as *const *const u8;
        miku::miku_execve(path, argv.as_ptr(), argc, envp, miku::miku_env_count())
    };

    // On exec failure, asks the kernel, which runs commands like `nvidia`,
    // `mount`, `sv` and enforces its own privilege. Their output bypasses fd 1,
    // so `nvidia list > file` writes an empty file. Kernel commands take no
    // environment
    if first == 0 && !name.contains('/') {
        let mut line = [0u8; LINE_MAX];
        if let Some(len) = flatten(cmd, &mut line) {
            let rc = unsafe { miku::miku_kcmd(line.as_ptr(), len) };
            // -ENOENT means the kernel does not know the name either
            if rc >= 0 {
                exit(0);
            }
        }
    }

    print("msh: ");
    print(name);
    println(": not found in /bin");
    // Suggests `mikudo` for `sudo` and `doas`
    if name == "sudo" || name == "doas" {
        println("  (on MikuOS it is 'mikudo': mikudo cmd, or mikudo -s for a root shell)");
    }
    exit(127);
}

/// Joins a command's words into one line for the kernel channel. `None` if too
/// long
fn flatten(cmd: &Cmd, out: &mut [u8; LINE_MAX]) -> Option<usize> {
    let mut len = 0usize;
    for i in 0..cmd.n {
        if i > 0 {
            if len == out.len() {
                return None;
            }
            out[len] = b' ';
            len += 1;
        }
        let w = cmd.words[i].as_bytes();
        if len + w.len() > out.len() {
            return None;
        }
        out[len..len + w.len()].copy_from_slice(w);
        len += w.len();
    }
    if len == 0 {
        None
    } else {
        Some(len)
    }
}

fn report_tty() {
    print("tty ");
    miku::print_hex(miku::tty::current() as u64);
    print("  job ");
    miku::print_hex(miku::proc::getpgid(0));
    print("  foreground ");
    miku::print_hex(miku::tty::foreground_of().unwrap_or(0));
    println("");
}

fn help() {
    println("msh - the shell, as a program");
    println("");
    println("  cd [dir]     change directory        (this shell's own)");
    println("  pwd          where you are");
    println("  echo ...     say it back");
    println("  tty          terminal, job, and who has the keys");
    println("  exit [code]  end this shell, and this terminal with it");
    println("");
    println("  jobs         what is running in the background");
    println("  wait         wait for all of it");
    println("");
    println("  cmd | cmd    a pipeline, run as one job");
    println("  cmd > file   send output to a file   (>> appends)");
    println("  cmd < file   take input from a file");
    println("  cmd 2> file  send errors to a file   (2>> appends)");
    println("  cmd > f 2>&1 send both to the same file  (>&2 is the other way)");
    println("  a ; b        one after the other");
    println("  a && b       b only if a succeeded    (|| only if it failed)");
    println("  a &          in the background, and keep the keys here");
    println("  X=1 cmd      cmd with X=1 in its environment, and only its");
    println("");
    println("At the prompt: arrows move and recall, Home/End, Delete,");
    println("  ^A start  ^E end  ^K kill to end  ^U kill the line  ^W a word");
    println("  ^L clear  ^C abandon the line  ^D end of input");
    println("");
    println("  'text'       literal, every character of it");
    println("  \"text\"       literal, but \\\" \\\\ \\$ \\` still escape");
    println("  a\\ b         one word with a space in it");
    println("  # ...        the rest of the line is a comment");
    println("");
    println("Not a terminal: 'msh -c \"line\"' runs one line, 'msh file' runs");
    println("every line of a file, and both exit with the last status.");
    println("");
    println("Anything else is looked for in /bin: ls cat cp mv rm mkdir rmdir");
    println("ln touch stat chmod chown du df tree head tail wc grep sort uniq");
    println("tee ps kill sleep date uptime uname free clear which env, and");
    println("whatever else is installed there.");
    println("");
    println("To run one as somebody else:");
    println("  mikudo cmd     run it as root, on this terminal");
    println("  mikudo -s      a root shell");
    println("  mikudo -l      what you are allowed to run");
    println("");
    println("The machine's own commands - the GPU, the block layer, mkfs,");
    println("mount, sv, the selftests - are the kernel console's. They are not");
    println("programs, so mikudo cannot run them either.");
}

fn builtin_cd(path: &str) -> i64 {
    let mut buf = [0u8; PATH_MAX];
    let p = match cstr(&mut buf, path) {
        Some(p) => p,
        None => {
            eprintln("cd: path too long");
            return 1;
        }
    };
    let r = unsafe { miku::miku_chdir(p) };
    if r < 0 {
        print("cd: ");
        print(path);
        println(": no");
        return 1;
    }
    0
}

// Stack helpers

/// `NAME=value`: a letter or `_`, then letters, digits, and `_`, and an `=`
fn is_assignment(w: &str) -> bool {
    let Some((key, _)) = w.split_once('=') else {
        return false;
    };
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn cstr(buf: &mut [u8], s: &str) -> Option<*const u8> {
    if s.len() + 1 > buf.len() {
        return None;
    }
    buf[..s.len()].copy_from_slice(s.as_bytes());
    buf[s.len()] = 0;
    Some(buf.as_ptr())
}

fn getcwd(buf: &mut [u8]) -> Option<&str> {
    let p = unsafe { miku::miku_getcwd(buf.as_mut_ptr(), buf.len()) };
    if p.is_null() {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    core::str::from_utf8(&buf[..end]).ok()
}

fn parse_i64(s: &str) -> Option<i64> {
    let mut v: i64 = 0;
    if s.is_empty() {
        return None;
    }
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((b - b'0') as i64)?;
    }
    Some(v)
}

/// Exits without printing, since program state is unknown
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
