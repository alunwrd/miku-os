// Shared code for the command-line programs, included with `#[path]`
//
// - Failure messages go to fd 2 as `ls: /nope: no such file`.
// - Exit codes: 1 on failure, 2 on bad usage.
// - Errors do not stop a list: `rm a b c` with an unreadable `b` still removes
//   `c` and exits 1.

#![allow(dead_code)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use miku::MikuStat;

/// File kinds, as in `MikuStat::kind` and `MikuDirent::kind` (kernel's
/// `VNodeKind`)
pub use miku_abi::kstat::KIND_REGULAR;
pub const KIND_DIR:     u8 = 1;
pub use miku_abi::kstat::KIND_SYMLINK;
pub const KIND_CHAR:    u8 = 3;
pub const KIND_BLOCK:   u8 = 4;
pub use miku_abi::kstat::KIND_PIPE;
pub use miku_abi::kstat::KIND_SOCKET;
pub use miku_abi::kstat::KIND_FIFO;

/// Longest path buffer for a syscall
const PATH_MAX: usize = 256;

pub fn out(s: &str) {
    miku::write(1, s.as_bytes());
}

pub fn outln(s: &str) {
    miku::write(1, s.as_bytes());
    miku::write(1, b"\n");
}

pub fn errln(s: &str) {
    miku::write(2, s.as_bytes());
    miku::write(2, b"\n");
}

/// `prog: subject: reason`
pub fn complain(prog: &str, subject: &str, reason: &str) {
    errln(&format!("{prog}: {subject}: {reason}"));
}

/// Text for a negative syscall return (`kernel/syscall/errno.rs`). Unknown
/// numbers print as is
pub fn reason(rc: i64) -> String {
    match -rc {
        1  => String::from("not permitted"),
        2  => String::from("no such file or directory"),
        5  => String::from("input/output error"),
        9  => String::from("bad descriptor"),
        13 => String::from("permission denied"),
        17 => String::from("already exists"),
        20 => String::from("not a directory"),
        21 => String::from("is a directory"),
        22 => String::from("invalid argument"),
        28 => String::from("no space left"),
        30 => String::from("read-only filesystem"),
        36 => String::from("name too long"),
        39 => String::from("directory not empty"),
        _  => format!("error {}", -rc),
    }
}

/// NUL-terminated copy. `None` if too long, rather than truncating
pub fn cpath(s: &str) -> Option<Vec<u8>> {
    if s.len() + 1 > PATH_MAX { return None; }
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    Some(v)
}

/// `dir` and `name` with exactly one slash between
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir == "." {
        String::from(name)
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Last path component
pub fn basename(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((_, name)) if !name.is_empty() => name,
        Some((_, _)) => path,
        None => path,
    }
}

pub fn stat(path: &str) -> Result<MikuStat, i64> {
    let c = match cpath(path) { Some(c) => c, None => return Err(-36) };
    let mut st = MikuStat::default();
    let rc = unsafe { miku::miku_stat(c.as_ptr(), &mut st as *mut MikuStat) };
    if rc < 0 { Err(rc) } else { Ok(st) }
}

pub fn is_dir(st: &MikuStat) -> bool { st.kind == KIND_DIR }

/// `drwxr-xr-x`, the way `ls -l` says it
pub fn mode_string(kind: u8, mode: u32) -> String {
    let t = match kind {
        KIND_DIR     => 'd',
        KIND_SYMLINK => 'l',
        KIND_CHAR    => 'c',
        KIND_BLOCK   => 'b',
        KIND_PIPE | KIND_FIFO => 'p',
        KIND_SOCKET  => 's',
        _            => '-',
    };
    let mut s = String::with_capacity(10);
    s.push(t);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 0o7;
        s.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        s.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        s.push(if bits & 0o1 != 0 { 'x' } else { '-' });
    }
    s
}

// Account names and dates: names from /etc/passwd and /etc/group through
// `mikuauth::db`, dates from `miku_dt_from_timestamp`

/// Account files, read once per program
pub struct Accounts {
    passwd: String,
    group:  String,
}

impl Accounts {
    /// Loads them, or continues with numeric ids if unreadable
    pub fn load() -> Self {
        let text = |p: &str| -> String {
            read_file(p).ok()
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_default()
        };
        Accounts { passwd: text("/etc/passwd"), group: text("/etc/group") }
    }

    /// The owner's name, or the number when there is no such account
    pub fn user(&self, uid: u16) -> String {
        mikuauth::db::find_uid(&self.passwd, uid)
            .map(|p| String::from(p.name))
            .unwrap_or_else(|| format!("{uid}"))
    }

    pub fn group(&self, gid: u16) -> String {
        mikuauth::db::find_gid(&self.group, gid)
            .map(|g| String::from(g.name))
            .unwrap_or_else(|| format!("{gid}"))
    }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                            "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// `Sep 16 06:00`, or `Sep 16  2025` when over six months old. Same width either way
pub fn stamp(when: u64, now: u64) -> String {
    if when == 0 { return String::from("            "); }
    let dt = unsafe { miku::miku_dt_from_timestamp(when as i64) };
    let month = MONTHS.get((dt.month.max(1) - 1) as usize).copied().unwrap_or("???");
    const SIX_MONTHS: u64 = 182 * 24 * 60 * 60;
    let recent = now == 0 || (when + SIX_MONTHS > now && when <= now + 60);
    if recent {
        format!("{month} {:>2} {:02}:{:02}", dt.day, dt.hour, dt.minute)
    } else {
        format!("{month} {:>2}  {:04}", dt.day, dt.year)
    }
}

/// `2026-09-16 06:00:00`, as `stat` prints
pub fn timestamp_full(when: u64) -> String {
    if when == 0 { return String::from("-"); }
    let dt = unsafe { miku::miku_dt_from_timestamp(when as i64) };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second)
}

/// One `key: value` line from a /proc file
pub fn proc_value(path: &str, key: &str) -> Option<u64> {
    let data = read_file(path).ok()?;
    let text = core::str::from_utf8(&data).ok()?;
    for line in text.lines() {
        // Skips lines with no colon (`/proc/stat`'s `cpu` lines)
        let Some((name, value)) = line.split_once(':') else { continue };
        if name.trim() != key { continue; }
        let digits: String = value.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
        return parse_u64(&digits);
    }
    None
}

/// `HH:MM:SS` from a count of scheduler ticks, as `ps` prints CPU time
pub fn cpu_time(ticks: u64, hz: u64) -> String {
    let secs = if hz == 0 { 0 } else { ticks / hz };
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
}

/// `ps` TTY column from a raw `TtyId`: 0 is `?`, `0x0100` the console, others
/// pseudo-terminals by slot
pub fn tty_name(raw: u32) -> String {
    match raw {
        0 => String::from("?"),
        0x0100 => String::from("tty0"),
        other => format!("pts{}", other & 0xff),
    }
}

/// The wall clock in seconds, or 0 when it cannot be had
pub fn now_secs() -> u64 {
    let mut ts = miku::time::Timespec::default();
    if miku::time::clock_gettime(miku::time::CLOCK_REALTIME, &mut ts) < 0 {
        return 0;
    }
    ts.tv_sec.max(0) as u64
}

/// Right-aligned in `width`, for the columns of `ls -l` and `wc`
pub fn pad_left(s: &str, width: usize) -> String {
    let mut out = String::with_capacity(width.max(s.len()));
    for _ in s.len()..width { out.push(' '); }
    out.push_str(s);
    out
}

pub fn pad_right(s: &str, width: usize) -> String {
    let mut out = String::with_capacity(width.max(s.len()));
    out.push_str(s);
    for _ in s.len()..width { out.push(' '); }
    out
}

/// Reads a whole file, or returns the error
pub fn read_file(path: &str) -> Result<Vec<u8>, i64> {
    let c = match cpath(path) { Some(c) => c, None => return Err(-36) };
    let fd = unsafe { miku::miku_open(c.as_ptr(), path.len(), 0x0001, 0) };
    if fd < 0 { return Err(fd); }
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = unsafe { miku::miku_read(fd as u64, buf.as_mut_ptr(), buf.len()) };
        if n < 0 { miku::close(fd); return Err(n); }
        if n == 0 { break; }
        data.extend_from_slice(&buf[..n as usize]);
    }
    miku::close(fd);
    Ok(data)
}

// Open flags (`kernel/fs/vfs/types.rs`)
pub const O_READ: u32 = miku_abi::fcntl::O_RDONLY as u32;
pub const O_WRITE: u32 = miku_abi::fcntl::O_WRONLY as u32;
pub const O_APPEND: u32 = miku_abi::fcntl::O_APPEND as u32;
pub const O_CREATE: u32 = miku_abi::fcntl::O_CREAT as u32;
pub const O_TRUNC: u32 = miku_abi::fcntl::O_TRUNC as u32;

pub fn open(path: &str, flags: u32, mode: u32) -> Result<i64, i64> {
    let c = match cpath(path) { Some(c) => c, None => return Err(-36) };
    let fd = unsafe { miku::miku_open(c.as_ptr(), path.len(), flags, mode) };
    if fd < 0 { Err(fd) } else { Ok(fd) }
}

/// Writes the whole slice, retrying short writes
pub fn write_all(fd: i64, data: &[u8]) -> Result<(), i64> {
    let mut off = 0usize;
    while off < data.len() {
        let n = unsafe { miku::miku_write(fd as u64, data[off..].as_ptr(), data.len() - off) };
        if n < 0 { return Err(n); }
        if n == 0 { return Err(-5); }
        off += n as usize;
    }
    Ok(())
}

/// Parses plain decimal. `None` for anything else, including a partial parse
pub fn parse_u64(s: &str) -> Option<u64> {
    if s.is_empty() { return None; }
    let mut v: u64 = 0;
    for b in s.bytes() {
        if !b.is_ascii_digit() { return None; }
        v = v.checked_mul(10)?.checked_add((b - b'0') as u64)?;
    }
    Some(v)
}

/// A whole file, or standard input for `-`
pub fn slurp(path: &str) -> Result<Vec<u8>, i64> {
    if path != "-" { return read_file(path); }
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = miku::read(0, &mut buf);
        if n < 0 { return Err(n); }
        if n == 0 { break; }
        data.extend_from_slice(&buf[..n as usize]);
    }
    Ok(data)
}

/// Parse an octal file mode, as `chmod 644` means it
pub fn parse_octal(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 4 { return None; }
    let mut v: u32 = 0;
    for b in s.bytes() {
        if !(b'0'..=b'7').contains(&b) { return None; }
        v = v * 8 + (b - b'0') as u32;
    }
    Some(v)
}

/// Every entry of a directory, sorted by name, looping over `getdents`
pub fn read_dir(path: &str) -> Result<Vec<(String, u8)>, i64> {
    let c = match cpath(path) { Some(c) => c, None => return Err(-36) };
    // O_READ | O_DIRECTORY
    let fd = unsafe { miku::miku_open(c.as_ptr(), c.len() - 1, 0x0001 | 0x0040, 0) };
    if fd < 0 { return Err(fd); }

    const BATCH: usize = 64;
    let mut ents: Vec<miku::MikuDirent> = Vec::with_capacity(BATCH);
    for _ in 0..BATCH {
        ents.push(miku::MikuDirent {
            name: [0; 256], inode_id: 0, kind: 0, name_len: 0, _reserved: [0; 6],
        });
    }
    let mut all: Vec<miku::MikuDirent> = Vec::new();
    let mut offset = 0usize;
    loop {
        let n = unsafe { miku::miku_getdents(fd, ents.as_mut_ptr(), BATCH, offset) };
        if n < 0 { unsafe { miku::miku_close(fd) }; return Err(n); }
        if n == 0 { break; }
        for e in ents.iter().take(n as usize) { all.push(*e); }
        offset += n as usize;
    }
    unsafe { miku::miku_close(fd) };
    let ents = all;
    let n = ents.len();
    let mut out: Vec<(String, u8)> = Vec::new();
    for e in ents.iter().take(n) {
        let len = (e.name_len as usize).min(e.name.len() - 1);
        let name = core::str::from_utf8(&e.name[..len]).unwrap_or("");
        if name.is_empty() { continue; }
        out.push((String::from(name), e.kind));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}
