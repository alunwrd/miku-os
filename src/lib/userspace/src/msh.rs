//! msh - MikuOS userspace shell.
//!
//! The system shell has always lived inside the kernel: 8.5k lines running in
//! ring 0, calling `fs::`, `block::` and `sched::` internals directly. That
//! makes every parsing bug a kernel bug, and it means the syscall ABI has
//! essentially no users, so nothing ever exercises it.
//!
//! This is the first ring-3 shell. It only uses the syscall surface that
//! already exists, so it is deliberately smaller than the in-kernel one - the
//! commands that still need kernel internals (nvidia, block, fs.*) stay
//! there for now, and mikuD keeps the in-kernel shell as a fallback. Every
//! command moved here is one less thing parsing user input at ring 0.

#![no_std]
#![no_main]

#[path = "miku.rs"]
mod miku;

use miku::*;

const LINE_MAX: usize = 512;
const PATH_MAX: usize = 256;
const DIRENTS_MAX: usize = 128;

// Small helpers - no allocator, everything on the stack

/// Copy `s` into `buf` and NUL-terminate. Returns the C string pointer.
/// Truncates rather than overflowing
fn cstr<'a>(buf: &'a mut [u8], s: &str) -> *const u8 {
    let n = if s.len() < buf.len() - 1 { s.len() } else { buf.len() - 1 };
    buf[..n].copy_from_slice(&s.as_bytes()[..n]);
    buf[n] = 0;
    buf.as_ptr()
}

fn as_str(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap_or("")
}

fn split_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(' ') {
        Some(i) => (&s[..i], s[i + 1..].trim_start()),
        None => (s, ""),
    }
}

fn print_u64(v: u64) {
    print_int(v as i64);
}

// Commands

fn cmd_pwd() {
    let mut buf = [0u8; PATH_MAX];
    unsafe {
        if miku_getcwd(buf.as_mut_ptr(), buf.len()).is_null() {
            eprintln("pwd: failed");
            return;
        }
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    println(as_str(&buf[..end]));
}

fn cmd_cd(arg: &str) {
    let target = if arg.is_empty() { "/" } else { arg };
    let mut buf = [0u8; PATH_MAX];
    let p = cstr(&mut buf, target);
    if unsafe { miku_chdir(p) } < 0 {
        eprint("cd: cannot change to ");
        eprintln(target);
    }
}

fn cmd_ls(arg: &str) {
    let path = if arg.is_empty() { "." } else { arg };
    let mut pbuf = [0u8; PATH_MAX];
    let p = cstr(&mut pbuf, path);

    let mut ents: [MikuDirent; DIRENTS_MAX] = unsafe { core::mem::zeroed() };
    let n = unsafe { miku_readdir(p, ents.as_mut_ptr(), DIRENTS_MAX) };
    if n < 0 {
        eprint("ls: cannot read ");
        eprintln(path);
        return;
    }
    for i in 0..n as usize {
        let e = &ents[i];
        let len = e.name_len as usize;
        let len = if len <= e.name.len() { len } else { e.name.len() };
        // kind 2 is a directory in the MikuOS dirent ABI
        if e.kind == 2 {
            print(as_str(&e.name[..len]));
            println("/");
        } else {
            println(as_str(&e.name[..len]));
        }
    }
}

fn cmd_cat(arg: &str) {
    if arg.is_empty() {
        eprintln("usage: cat <file>");
        return;
    }
    let mut pbuf = [0u8; PATH_MAX];
    let p = cstr(&mut pbuf, arg);
    let fd = unsafe { miku_open_cstr(p) };
    if fd < 0 {
        eprint("cat: cannot open ");
        eprintln(arg);
        return;
    }
    let mut buf = [0u8; 1024];
    loop {
        let n = unsafe { miku_read_fd(fd, buf.as_mut_ptr(), buf.len()) };
        if n <= 0 {
            break;
        }
        unsafe { miku_write(1, buf.as_ptr(), n as usize) };
    }
    unsafe { miku_close(fd) };
}

fn cmd_stat(arg: &str) {
    if arg.is_empty() {
        eprintln("usage: stat <path>");
        return;
    }
    let mut pbuf = [0u8; PATH_MAX];
    let p = cstr(&mut pbuf, arg);
    let mut st: MikuStat = unsafe { core::mem::zeroed() };
    if unsafe { miku_stat(p, &mut st) } < 0 {
        eprint("stat: cannot stat ");
        eprintln(arg);
        return;
    }
    print("  size:  ");
    print_u64(st.size);
    println(" bytes");
    print("  mode:  ");
    print_hex(st.mode as u64);
    println("");
    print("  inode: ");
    print_u64(st.inode_id);
    println("");
    print("  links: ");
    print_u64(st.nlinks as u64);
    println("");
}

fn cmd_mkdir(arg: &str) {
    if arg.is_empty() {
        eprintln("usage: mkdir <dir>");
        return;
    }
    let mut pbuf = [0u8; PATH_MAX];
    let p = cstr(&mut pbuf, arg);
    if unsafe { miku_mkdir(p, 0o755) } < 0 {
        eprint("mkdir: failed: ");
        eprintln(arg);
    }
}

fn cmd_rm(arg: &str) {
    if arg.is_empty() {
        eprintln("usage: rm <path>");
        return;
    }
    let mut pbuf = [0u8; PATH_MAX];
    let p = cstr(&mut pbuf, arg);
    let rc = if unsafe { miku_isdir(p) } {
        unsafe { miku_rmdir(p) }
    } else {
        unsafe { miku_unlink(p) }
    };
    if rc < 0 {
        eprint("rm: failed: ");
        eprintln(arg);
    }
}

fn cmd_write(rest: &str) {
    let (path, text) = split_word(rest);
    if path.is_empty() {
        eprintln("usage: write <file> <text>");
        return;
    }
    let mut pbuf = [0u8; PATH_MAX];
    let p = cstr(&mut pbuf, path);
    let mut tbuf = [0u8; LINE_MAX];
    let t = cstr(&mut tbuf, text);
    if unsafe { miku_write_file_cstr(p, t) } < 0 {
        eprint("write: failed: ");
        eprintln(path);
    }
}


// Filters. These exist so a pipeline has something to pipe into; each also
// works standalone on a file argument.

/// Read all of `fd` through `f`, a line at a time
fn for_each_line<F: FnMut(&str)>(fd: i64, mut f: F) {
    let mut buf = [0u8; 4096];
    let mut used = 0usize;
    loop {
        if used == buf.len() {
            // Overlong line: flush what we have and carry on
            f(as_str(&buf[..used]));
            used = 0;
        }
        let n = unsafe { miku_read_fd(fd, buf.as_mut_ptr().add(used), buf.len() - used) };
        if n <= 0 { break; }
        used += n as usize;

        let mut start = 0usize;
        let mut i = 0usize;
        while i < used {
            if buf[i] == b'\n' {
                f(as_str(&buf[start..i]));
                start = i + 1;
            }
            i += 1;
        }
        if start > 0 {
            buf.copy_within(start..used, 0);
            used -= start;
        }
    }
    if used > 0 {
        f(as_str(&buf[..used]));
    }
}

/// Open an argument as a file, or fall back to stdin when there is none.
/// Returns (fd, needs_close)
fn input_fd(arg: &str) -> Option<(i64, bool)> {
    if arg.is_empty() {
        return Some((0, false));
    }
    let mut pbuf = [0u8; PATH_MAX];
    let p = cstr(&mut pbuf, arg);
    let fd = unsafe { miku_open_cstr(p) };
    if fd < 0 {
        eprint("cannot open ");
        eprintln(arg);
        return None;
    }
    Some((fd, true))
}

fn cmd_wc(arg: &str) {
    let Some((fd, close)) = input_fd(arg) else { return };
    let mut lines = 0u64;
    let mut words = 0u64;
    let mut bytes = 0u64;
    for_each_line(fd, |l| {
        lines += 1;
        bytes += l.len() as u64 + 1;
        words += l.split_whitespace().count() as u64;
    });
    if close { unsafe { miku_close(fd) }; }
    print_u64(lines); print(" ");
    print_u64(words); print(" ");
    print_u64(bytes); println("");
}

fn cmd_head(rest: &str) {
    let (first, second) = split_word(rest);
    // head [-n N] [file]
    let (count, path) = if first == "-n" {
        let (n, p) = split_word(second);
        (n.parse::<u64>().unwrap_or(10), p)
    } else {
        (10u64, first)
    };
    let Some((fd, close)) = input_fd(path) else { return };
    let mut seen = 0u64;
    for_each_line(fd, |l| {
        if seen < count {
            println(l);
            seen += 1;
        }
    });
    if close { unsafe { miku_close(fd) }; }
}

fn cmd_grep(rest: &str) {
    let (pattern, path) = split_word(rest);
    if pattern.is_empty() {
        eprintln("usage: grep <pattern> [file]");
        return;
    }
    let Some((fd, close)) = input_fd(path) else { return };
    for_each_line(fd, |l| {
        if l.contains(pattern) {
            println(l);
        }
    });
    if close { unsafe { miku_close(fd) }; }
}

fn cmd_echo(rest: &str) {
    println(rest);
}


/// Raw syscall. The miku libraries do not expose clock_gettime yet (they are
/// built from a separate tree), so the shell issues it directly - the ABI is
/// the point of this program, and a two-argument syscall needs no runtime
#[inline(always)]
unsafe fn syscall2(nr: u64, a1: u64, a2: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") nr => ret,
        in("rdi") a1,
        in("rsi") a2,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    ret
}

const SYS_CLOCK_GETTIME: u64 = 70;
const CLOCK_REALTIME: u64 = 0;

/// Days-to-civil, same shifted-year algorithm the kernel uses
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn print_pad2(v: u64) {
    if v < 10 {
        print("0");
    }
    print_u64(v);
}


// Terminal control through ioctl(2). Proves the tty layer end to end:
// read the current settings, flip them, read them back
const SYS_IOCTL: u64 = 71;
const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;
const TIOCGWINSZ: u64 = 0x5413;

#[repr(C)]
#[derive(Clone, Copy)]
struct Termios {
    icanon: u8,
    echo: u8,
    isig: u8,
    _pad: u8,
}

#[inline(always)]
unsafe fn syscall3(nr: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") nr => ret,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    ret
}

fn tcgets() -> Option<Termios> {
    let mut t = Termios { icanon: 0, echo: 0, isig: 0, _pad: 0 };
    let rc = unsafe { syscall3(SYS_IOCTL, 0, TCGETS, &mut t as *mut _ as u64) };
    if rc > u64::MAX - 4096 { None } else { Some(t) }
}

fn tcsets(t: &Termios) -> bool {
    let rc = unsafe { syscall3(SYS_IOCTL, 0, TCSETS, t as *const _ as u64) };
    rc <= u64::MAX - 4096
}

fn cmd_stty(arg: &str) {
    let mut t = match tcgets() {
        Some(t) => t,
        None => { eprintln("stty: not a terminal"); return; }
    };
    match arg.trim() {
        "" => {
            print("icanon=");  print_u64(t.icanon as u64);
            print(" echo=");   print_u64(t.echo as u64);
            print(" isig=");   print_u64(t.isig as u64);
            println("");
            let mut ws = [0u16; 2];
            if unsafe { syscall3(SYS_IOCTL, 0, TIOCGWINSZ, ws.as_mut_ptr() as u64) } <= u64::MAX - 4096 {
                print("size ");
                print_u64(ws[0] as u64);
                print("x");
                print_u64(ws[1] as u64);
                println(" (rows x cols)");
            }
        }
        "raw"    => { t.icanon = 0; t.echo = 0; if tcsets(&t) { println("raw mode"); } }
        "cooked" => { t.icanon = 1; t.echo = 1; if tcsets(&t) { println("cooked mode"); } }
        "-echo"  => { t.echo = 0; if tcsets(&t) { println("echo off"); } }
        "echo"   => { t.echo = 1; if tcsets(&t) { println("echo on"); } }
        _ => eprintln("usage: stty [raw|cooked|echo|-echo]"),
    }
}

fn cmd_date() {
    let mut ts = [0i64; 2];
    let rc = unsafe { syscall2(SYS_CLOCK_GETTIME, CLOCK_REALTIME, ts.as_mut_ptr() as u64) };
    // errno comes back as a small negative value in two's complement
    if rc > u64::MAX - 4096 {
        eprintln("date: clock not set");
        return;
    }
    let secs = ts[0] as u64;
    let (y, mo, d) = civil_from_days((secs / 86400) as i64);
    let rem = secs % 86400;
    print_u64(y as u64);
    print("-");
    print_pad2(mo as u64);
    print("-");
    print_pad2(d as u64);
    print(" ");
    print_pad2(rem / 3600);
    print(":");
    print_pad2((rem % 3600) / 60);
    print(":");
    print_pad2(rem % 60);
    println(" UTC");
}

fn cmd_uptime() {
    print("up ");
    print_u64(uptime_ms() / 1000);
    println(" s");
}

fn cmd_help() {
    println("msh - MikuOS userspace shell");
    println("  pwd                 print working directory");
    println("  cd [dir]            change directory");
    println("  ls [dir]            list directory");
    println("  cat <file>          print a file");
    println("  stat <path>         file metadata");
    println("  mkdir <dir>         create a directory");
    println("  rm <path>           remove a file or empty directory");
    println("  write <file> <txt>  write text to a file");
    println("  echo <text>         print text");
    println("  wc [file]           count lines, words, bytes");
    println("  head [-n N] [file]  first N lines");
    println("  grep <pat> [file]   lines containing pat");
    println("  uptime              time since boot");
    println("  date                wall-clock date and time (UTC)");
    println("  stty [raw|cooked]   show or change terminal settings");
    println("  pid                 shell pid");
    println("  exit                leave the shell");
}


/// Startup script. Runs before the first prompt, so a machine can be brought
/// up with a fixed set of commands and - just as usefully - a headless boot
/// can be checked without anyone typing
const RC_PATH: &str = "/etc/msh.rc";

fn run_rc() {
    let mut pbuf = [0u8; PATH_MAX];
    let p = cstr(&mut pbuf, RC_PATH);
    let fd = unsafe { miku_open_cstr(p) };
    if fd < 0 {
        return;
    }
    println("msh: running /etc/msh.rc");

    let mut buf = [0u8; 2048];
    let n = unsafe { miku_read_fd(fd, buf.as_mut_ptr(), buf.len()) };
    unsafe { miku_close(fd) };
    if n <= 0 {
        return;
    }

    for line in as_str(&buf[..n as usize]).split('\n') {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        print("+ ");
        println(line);
        if !run_pipeline(line) {
            return;
        }
    }
}


// Redirection and pipelines
//
// `dispatch` runs one simple command. Everything below arranges the file
// descriptors around it: `>` `>>` `<` rewire the shell's own stdio and put it
// back afterwards, and `|` forks a child per stage so each side of the pipe
// gets its own descriptors. Builtins run in the child directly - no exec is
// needed when the code is already in the image.

// Must match kernel OpenFlags (fs/vfs/types.rs)
const O_READ:     u32 = 0x0001;
const O_WRITE:    u32 = 0x0002;
const O_APPEND:   u32 = 0x0004;
const O_CREATE:   u32 = 0x0008;
const O_TRUNCATE: u32 = 0x0020;

fn open_for(path: &str, flags: u32) -> i64 {
    let mut pbuf = [0u8; PATH_MAX];
    let bytes = path.as_bytes();
    let n = if bytes.len() < pbuf.len() { bytes.len() } else { pbuf.len() };
    pbuf[..n].copy_from_slice(&bytes[..n]);
    unsafe { miku_open(pbuf.as_ptr(), n, flags, 0o644) }
}

/// Strip trailing `> file`, `>> file`, `< file` off a command and apply them.
/// Returns the bare command plus the descriptors saved for restoration
fn apply_redirects(cmd: &str) -> Option<(usize, Redir, Redir)> {
    let mut out = Redir::untouched();
    let mut inp = Redir::untouched();
    let mut end = cmd.len();

    // Scan left to right, remembering where the command text stops
    let bytes = cmd.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let (op, oplen) = if bytes[i] == b'>' && i + 1 < bytes.len() && bytes[i + 1] == b'>' {
            (2u8, 2usize)
        } else if bytes[i] == b'>' {
            (1u8, 1usize)
        } else if bytes[i] == b'<' {
            (3u8, 1usize)
        } else {
            i += 1;
            continue;
        };
        if end > i { end = i; }
        let target = cmd[i + oplen..].trim();
        let target = split_word(target).0;
        if target.is_empty() {
            eprintln("msh: missing redirection target");
            return None;
        }
        let (flags, dst) = match op {
            1 => (O_WRITE | O_CREATE | O_TRUNCATE, 1i64),
            2 => (O_WRITE | O_CREATE | O_APPEND, 1i64),
            _ => (O_READ, 0i64),
        };
        let fd = open_for(target, flags);
        if fd < 0 {
            eprint("msh: cannot open ");
            eprintln(target);
            return None;
        }
        // Remember the previous binding so the shell's own stdio survives.
        // dup() fails when the descriptor is still the bare terminal (it has
        // no fd-table entry yet); that is fine - closing it afterwards is
        // what restores the terminal
        if dst == 1 { out = Redir { applied: true, saved: unsafe { miku_dup(1) } }; }
        else        { inp = Redir { applied: true, saved: unsafe { miku_dup(0) } }; }
        unsafe {
            miku_dup2(fd, dst);
            miku_close(fd);
        }
        i += oplen + 1;
    }
    Some((end, out, inp))
}

/// State of one standard stream around a redirection
#[derive(Clone, Copy)]
struct Redir {
    /// True when this run actually pointed the stream somewhere else
    applied: bool,
    /// Descriptor holding the previous binding, or -1 when there was none
    /// because the stream was still the plain terminal
    saved: i64,
}

impl Redir {
    fn untouched() -> Self { Self { applied: false, saved: -1 } }
}

fn restore_one(r: Redir, fd: i64) {
    if !r.applied {
        return;                     // never touched, leave it alone
    }
    unsafe {
        if r.saved >= 0 {
            miku_dup2(r.saved, fd);
            miku_close(r.saved);
        } else {
            // Dropping the redirected entry falls back to the terminal
            miku_close(fd);
        }
    }
}

fn restore_redirects(out: Redir, inp: Redir) {
    restore_one(out, 1);
    restore_one(inp, 0);
}

/// One command plus its redirections
fn run_simple(cmd: &str) -> bool {
    let Some((end, out, inp)) = apply_redirects(cmd) else { return true };
    let keep_going = dispatch(cmd[..end].trim());
    restore_redirects(out, inp);
    keep_going
}

/// A `|`-separated pipeline. Each stage runs in its own process so both ends
/// can hold the pipe open at once
fn run_pipeline(line: &str) -> bool {
    if !line.contains('|') {
        return run_simple(line);
    }

    let mut prev_read: i64 = -1;
    let mut children = [0i64; 8];
    let mut nchildren = 0usize;

    let mut rest = line;
    loop {
        let (stage, tail) = match rest.find('|') {
            Some(i) => (rest[..i].trim(), Some(rest[i + 1..].trim())),
            None => (rest.trim(), None),
        };

        let mut pipe_fds = [0i64; 2];
        let have_next = tail.is_some();
        if have_next && unsafe { miku_pipe(pipe_fds.as_mut_ptr()) } < 0 {
            eprintln("msh: pipe failed");
            return true;
        }

        let pid = unsafe { miku_fork() };
        if pid == 0 {
            // Child: wire up its ends, then run the stage and leave
            unsafe {
                if prev_read >= 0 { miku_dup2(prev_read, 0); miku_close(prev_read); }
                if have_next {
                    miku_close(pipe_fds[0]);
                    miku_dup2(pipe_fds[1], 1);
                    miku_close(pipe_fds[1]);
                }
            }
            run_simple(stage);
            exit(0);
        }

        // Parent: hand the read end to the next stage, drop the rest
        unsafe {
            if prev_read >= 0 { miku_close(prev_read); }
            if have_next {
                miku_close(pipe_fds[1]);
                prev_read = pipe_fds[0];
            } else {
                prev_read = -1;
            }
        }
        if pid > 0 && nchildren < children.len() {
            children[nchildren] = pid;
            nchildren += 1;
        }

        match tail {
            Some(t) => rest = t,
            None => break,
        }
    }

    for i in 0..nchildren {
        let mut status: i64 = 0;
        unsafe { miku_waitpid(children[i] as u64, &mut status) };
    }
    true
}

fn prompt() {
    let mut buf = [0u8; PATH_MAX];
    unsafe {
        if !miku_getcwd(buf.as_mut_ptr(), buf.len()).is_null() {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            print(as_str(&buf[..end]));
        }
    }
    print(" $ ");
}

fn dispatch(line: &str) -> bool {
    let (cmd, rest) = split_word(line);
    match cmd {
        "" => {}
        "help" | "?" => cmd_help(),
        "pwd" => cmd_pwd(),
        "cd" => cmd_cd(rest),
        "ls" => cmd_ls(rest),
        "cat" => cmd_cat(rest),
        "stat" => cmd_stat(rest),
        "mkdir" => cmd_mkdir(rest),
        "rm" => cmd_rm(rest),
        "write" => cmd_write(rest),
        "echo" => cmd_echo(rest),
        "wc" => cmd_wc(rest),
        "head" => cmd_head(rest),
        "grep" => cmd_grep(rest),
        "uptime" => cmd_uptime(),
        "date" => cmd_date(),
        "stty" => cmd_stty(rest),
        "pid" => {
            print_u64(getpid());
            println("");
        }
        "exit" | "quit" => return false,
        _ => {
            eprint("msh: unknown command: ");
            eprintln(cmd);
            eprintln("try 'help'");
        }
    }
    true
}

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    println("msh: MikuOS userspace shell (ring 3). 'help' for commands.");
    run_rc();

    let mut line = [0u8; LINE_MAX];
    loop {
        prompt();
        let n = unsafe { miku_readline(line.as_mut_ptr(), line.len()) };
        if n < 0 {
            // stdin closed - nothing more to do
            break;
        }
        let text = as_str(&line[..n as usize]).trim();
        if !run_pipeline(text) {
            break;
        }
    }

    println("msh: exit");
    exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    eprintln("msh: panic");
    exit(1);
}
