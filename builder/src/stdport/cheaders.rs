//! Generated C header constants and declaration checks
//!
//! `generate`: reads `pub const NAME: type = value;` from
//! `lib/rust-libc/libc/src/unix/miku/mod.rs`, what `libc_miku.so` uses, and
//! writes them to `target/miku-c/include/bits/<group>.h`, since MikuOS's values
//! differ from Linux's.
//!
//! `check_declarations`: every function a header declares must be defined by a
//! MikuOS library, checked against `lib/miku`'s link stubs.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::fsx::write_if_changed;

/// Generated headers directory, on `lib/c/miku-cc`'s include path
pub fn generated_dir(root: &Path) -> PathBuf {
    root.join("target/miku-c/include")
}

/// How a constant's name says which header it belongs in
enum Pat {
    /// Starts with this
    Prefix(&'static str),
    /// Is one of these
    Exact(&'static [&'static str]),
    /// This, then one or more of `[A-Z0-9]` (`digits_only`: `[0-9]`)
    Run(&'static str, bool),
}

const TERMIOS_NAMES: &[&str] = &[
    "NCCS", "VINTR", "VQUIT", "VERASE", "VKILL", "VEOF", "VTIME", "VMIN", "VSWTC", "VSTART",
    "VSTOP", "VSUSP", "VEOL", "VREPRINT", "VDISCARD", "VWERASE", "VLNEXT", "VEOL2", "IGNBRK",
    "BRKINT", "IGNPAR", "PARMRK", "INPCK", "ISTRIP", "INLCR", "IGNCR", "ICRNL", "IUCLC", "IXON",
    "IXANY", "IXOFF", "IMAXBEL", "IUTF8", "OPOST", "OLCUC", "ONLCR", "OCRNL", "ONOCR", "ONLRET",
    "OFILL", "OFDEL", "NLDLY", "NL0", "NL1", "CRDLY", "CR0", "CR1", "CR2", "CR3", "TABDLY",
    "TAB0", "TAB1", "TAB2", "TAB3", "XTABS", "BSDLY", "BS0", "BS1", "VTDLY", "VT0", "VT1",
    "FFDLY", "FF0", "FF1", "CBAUD", "CSIZE", "CS5", "CS6", "CS7", "CS8", "CSTOPB", "CREAD",
    "PARENB", "PARODD", "HUPCL", "CLOCAL", "CBAUDEX", "CMSPAR", "CRTSCTS", "ISIG", "ICANON",
    "XCASE", "ECHO", "ECHOE", "ECHOK", "ECHONL", "NOFLSH", "TOSTOP", "ECHOCTL", "ECHOPRT",
    "ECHOKE", "FLUSHO", "PENDIN", "IEXTEN", "EXTPROC", "TCSANOW", "TCSADRAIN", "TCSAFLUSH",
    "TCIFLUSH", "TCOFLUSH", "TCIOFLUSH", "TCOOFF", "TCOON", "TCIOFF", "TCION",
];

use Pat::*;

/// In order: the first matching group is a name's header
const GROUPS: &[(&str, &[Pat])] = &[
    ("termios", &[Exact(TERMIOS_NAMES), Run("B", true)]),
    ("errno", &[Run("E", false)]),
    ("fcntl", &[Prefix("O_"),
        Exact(&["F_DUPFD", "F_GETFD", "F_SETFD", "F_GETFL", "F_SETFL", "F_GETLK", "F_SETLK",
                "F_SETLKW", "F_RDLCK", "F_WRLCK", "F_UNLCK", "F_DUPFD_CLOEXEC", "F_GETPATH"]),
        Exact(&["FD_CLOEXEC"]), Prefix("AT_"), Prefix("LOCK_")]),
    ("stat", &[Prefix("S_I"), Prefix("UTIME_")]),
    ("unistd", &[Prefix("SEEK_"), Exact(&["R_OK", "W_OK", "X_OK", "F_OK"]), Prefix("_SC_"),
        Exact(&["STDIN_FILENO", "STDOUT_FILENO", "STDERR_FILENO"])]),
    ("mman", &[Prefix("PROT_"),
        Exact(&["MAP_SHARED", "MAP_PRIVATE", "MAP_FIXED", "MAP_ANONYMOUS", "MAP_ANON"]),
        Prefix("MADV_"), Prefix("POSIX_MADV_"), Exact(&["MS_ASYNC", "MS_SYNC", "MS_INVALIDATE"])]),
    ("socket", &[Prefix("AF_"), Prefix("PF_"), Prefix("SOCK_"), Prefix("SOL_"), Prefix("SO_"),
        Prefix("MSG_"), Prefix("SHUT_"), Prefix("SCM_"), Exact(&["SOMAXCONN"])]),
    ("in", &[Prefix("IPPROTO_"), Prefix("IP_"), Prefix("IPV6_"), Prefix("TCP_"), Prefix("INADDR_")]),
    ("netdb", &[Prefix("EAI_"), Prefix("AI_")]),
    ("random", &[Prefix("GRND_")]),
    ("time", &[Prefix("CLOCK_")]),
    ("signal", &[Run("SIG", false), Prefix("SA_"), Exact(&["SIG_BLOCK", "SIG_UNBLOCK", "SIG_SETMASK"])]),
    ("resource", &[Prefix("RLIMIT_")]),
    ("poll", &[Prefix("POLL")]),
    ("ioctl", &[Prefix("FIO"), Prefix("TIOC")]),
    ("stdlib", &[Exact(&["EXIT_SUCCESS", "EXIT_FAILURE"])]),
];

fn matches(p: &Pat, name: &str) -> bool {
    match p {
        Prefix(x) => name.starts_with(x),
        Exact(xs) => xs.contains(&name),
        Run(x, digits) => name.strip_prefix(x).is_some_and(|r| !r.is_empty() && r.chars().all(|c|
            c.is_ascii_digit() || (!digits && c.is_ascii_uppercase()))),
    }
}

fn group_of(name: &str) -> Option<&'static str> {
    GROUPS.iter().find(|(_, pats)| pats.iter().any(|p| matches(p, name))).map(|(g, _)| *g)
}

/// `pub const NAME: type = value;` -> `(NAME, value)`
fn parse_const(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim_start().strip_prefix("pub const")?;
    if !rest.starts_with(char::is_whitespace) { return None; }
    let rest = rest.trim_start();
    let name_len = rest.find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))?;
    let name = &rest[..name_len];
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) { return None; }
    let rest = rest[name_len..].trim_start().strip_prefix(':')?;
    let eq = rest.find('=')?;
    if rest[..eq].trim().is_empty() { return None; }
    let value = rest[eq + 1..].trim_end().strip_suffix(';')?.trim();
    if value.is_empty() { return None; }
    Some((name, value))
}

/// A Rust constant expression as C, or `None` if it has no C spelling
fn c_value(expr: &str) -> Option<String> {
    let e = expr.trim();
    if ["as ", "::", "!", "_u32", "_i32", "_u64", "_i64", "\""].iter().any(|t| e.contains(t)) {
        return None;
    }
    // `0o17` is C's `017`
    let mut s = String::new();
    let b = e.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'0' && b.get(i + 1) == Some(&b'o')
            && b.get(i + 2).is_some_and(|c| (b'0'..=b'7').contains(c) || *c == b'_')
        {
            s.push('0');
            i += 2;
            continue;
        }
        s.push(b[i] as char);
        i += 1;
    }
    // `0x8000_0000` loses separators between hex digits
    let c: Vec<char> = s.chars().collect();
    let hex = |ch: char| ch.is_ascii_hexdigit();
    let mut out = String::new();
    for (i, &ch) in c.iter().enumerate() {
        if ch == '_' && i > 0 && i + 1 < c.len() && hex(c[i - 1]) && hex(c[i + 1]) { continue; }
        out.push(ch);
    }
    let ok = out.chars().all(|ch| ch.is_ascii_alphanumeric() || "_()<>+-* |&".contains(ch));
    (ok && !out.is_empty()).then_some(out)
}

/// Writes `bits/<group>.h` for each group, only when changed
pub fn generate(root: &Path) -> Result<(), String> {
    let src = root.join("lib/rust-libc/libc/src/unix/miku/mod.rs");
    let text = fs::read_to_string(&src).map_err(|e| format!("{}: {e}", src.display()))?;
    let mut found: BTreeMap<&str, Vec<(String, String)>> = GROUPS.iter().map(|(g, _)| (*g, Vec::new())).collect();
    for line in text.lines() {
        let Some((name, expr)) = parse_const(line) else { continue };
        let Some(g) = group_of(name) else { continue };
        if let Some(v) = c_value(expr) {
            found.get_mut(g).unwrap().push((name.to_string(), v));
        }
    }
    let bits = generated_dir(root).join("bits");
    for (g, _) in GROUPS {
        let guard = format!("_MIKU_BITS_{}_H", g.to_uppercase());
        let mut t = vec![
            "/* generated by the builder (builder/src/stdport/cheaders.rs) from".to_string(),
            "   lib/rust-libc/libc/src/unix/miku/mod.rs - do not edit */".to_string(),
            format!("#ifndef {guard}"), format!("#define {guard}"), String::new(),
        ];
        for (name, v) in &found[g] {
            t.push(format!("#define {name} ({v})"));
        }
        t.extend(["".to_string(), "#endif".to_string(), String::new()]);
        let path = bits.join(format!("{g}.h"));
        write_if_changed(&path, &t.join("\n")).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

/// Function a prototype line declares: return type, name, `(`, at line start
/// outside a `#define`
fn declared_name(part: &str) -> Option<&str> {
    let open = part.find('(')?;
    let head = &part[..open];
    let first = head.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_') { return None; }
    if !head.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c.is_whitespace() || c == '*') {
        return None;
    }
    let head = head.trim_end();
    let start = head.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
    // A name needs a preceding type and starts lowercase or with `_`
    if start == 0 { return None; }
    let name = &head[start..];
    let lead = name.chars().next()?;
    if !(lead.is_ascii_lowercase() || lead == '_') { return None; }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') { return None; }
    Some(name)
}

fn strip_block_comments(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("/*") {
        // An unterminated comment is left as it is, as a regex would
        let Some(j) = rest[i + 2..].find("*/") else { break };
        out.push_str(&rest[..i]);
        out.push(' ');
        rest = &rest[i + 2 + j + 2..];
    }
    out.push_str(rest);
    out
}

/// Checks every header function against the library definitions; missing ones
/// are errors, one per line
pub fn check_declarations(root: &Path) -> Result<(), String> {
    let include = root.join("lib/c/include");
    let stubs = root.join("lib/miku/stubs");
    let mut have = std::collections::HashSet::new();
    for e in fs::read_dir(&stubs).map_err(|e| format!("{}: {e}", stubs.display()))?.flatten() {
        if e.path().extension().is_some_and(|x| x == "so") {
            let o = Command::new("nm").args(["-D", "--defined-only"]).arg(e.path()).output()
                .map_err(|err| format!("cannot run nm: {err}"))?;
            if !o.status.success() { return Err(format!("nm failed on {}", e.path().display())); }
            for l in String::from_utf8_lossy(&o.stdout).lines() {
                if let Some(n) = l.split_whitespace().last() { have.insert(n.to_string()); }
            }
        }
    }
    let mut declared: BTreeMap<String, String> = BTreeMap::new();
    for f in super::fsx::files_sorted(&include) {
        if f.extension().is_none_or(|x| x != "h") { continue; }
        let rel = f.strip_prefix(&include).unwrap().display().to_string();
        let text = strip_block_comments(&fs::read_to_string(&f).unwrap_or_default());
        for line in text.lines() {
            let s = line.trim();
            // A `static inline` function needs no library definition
            if ["#", "typedef", "struct", "union", "static"].iter().any(|p| s.starts_with(p)) { continue; }
            for part in s.split(';') {
                if let Some(n) = declared_name(part.trim()) {
                    declared.entry(n.to_string()).or_insert_with(|| rel.clone());
                }
            }
        }
    }
    let missing: Vec<String> = declared.iter()
        .filter(|(n, _)| !have.contains(*n))
        .map(|(n, h)| format!("lib/c/include/{h}: declares {n}, which no MikuOS library defines"))
        .collect();
    if missing.is_empty() { Ok(()) } else { Err(missing.join("\n")) }
}
