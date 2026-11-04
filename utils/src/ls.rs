#![no_std]
#![no_main]

//! ls: lists a directory
//!
//! Usage: ls [-l] [-a] [-1] [path...]
//!
//! Color only when the output is a terminal, detected by `TIOCGWINSZ`.

extern crate alloc;

pub use miku;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[path = "common.rs"]
mod common;
use common::*;

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);
    let mut long = false;
    let mut all = false;
    let mut one = false;
    let mut paths: Vec<&str> = Vec::new();

    for a in args.iter().skip(1) {
        match *a {
            "-l" => long = true,
            "-a" => all = true,
            "-1" => one = true,
            "-la" | "-al" => { long = true; all = true; }
            "-h" | "--help" => {
                outln("usage: ls [-l] [-a] [-1] [path...]");
                miku::exit(0);
            }
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("ls: unknown option {other}"));
                miku::exit(2);
            }
            other => paths.push(other),
        }
    }
    if paths.is_empty() { paths.push("."); }

    // Several paths each get a heading
    let many = paths.len() > 1;
    let mut bad = false;

    for (i, p) in paths.iter().enumerate() {
        if many {
            if i > 0 { outln(""); }
            outln(&format!("{p}:"));
        }
        if let Err(rc) = list(p, long, all, one) {
            complain("ls", p, &reason(rc));
            bad = true;
        }
    }
    miku::exit(if bad { 1 } else { 0 });
}

fn list(path: &str, long: bool, all: bool, one: bool) -> Result<(), i64> {
    // A non-directory lists itself
    let st = stat(path)?;
    if !is_dir(&st) {
        if long {
            outln(&long_line(path, &st));
        } else {
            outln(path);
        }
        return Ok(());
    }

    let entries = read_dir(path)?;
    let mut names: Vec<(String, u8)> = Vec::new();
    for (name, kind) in entries {
        if name == "." || name == ".." { if !all { continue; } }
        if !all && name.starts_with('.') { continue; }
        names.push((name, kind));
    }

    if long {
        // Two passes, since column widths need every entry
        let accounts = Accounts::load();
        let now = now_secs();
        let mut rows: Vec<Row> = Vec::new();
        let mut total_kb = 0u64;
        for (name, _) in &names {
            let full = join(path, name);
            match stat(&full) {
                Ok(st) => {
                    total_kb += st.size.div_ceil(1024);
                    rows.push(Row {
                        kind:  st.kind,
                        mode:  mode_string(st.kind, st.mode),
                        links: format!("{}", st.nlinks),
                        user:  accounts.user(st.uid),
                        group: accounts.group(st.gid),
                        size:  format!("{}", st.size),
                        when:  stamp(st.mtime, now),
                        name:  name.clone(),
                    });
                }
                // Shows entries `stat` fails on, such as broken links
                Err(rc) => outln(&format!("?????????? ? ? ? ? {} {name}", reason(rc))),
            }
        }
        let w_links = rows.iter().map(|r| r.links.len()).max().unwrap_or(1);
        let w_user  = rows.iter().map(|r| r.user.len()).max().unwrap_or(1);
        let w_group = rows.iter().map(|r| r.group.len()).max().unwrap_or(1);
        let w_size  = rows.iter().map(|r| r.size.len()).max().unwrap_or(1);

        outln(&format!("total {total_kb}"));
        for r in &rows {
            outln(&format!("{} {} {} {} {} {} {}",
                r.mode,
                pad_left(&r.links, w_links),
                pad_right(&r.user, w_user),
                pad_right(&r.group, w_group),
                pad_left(&r.size, w_size),
                r.when,
                paint(&r.name, r.kind)));
        }
        return Ok(());
    }

    if one || names.len() <= 1 {
        for (name, kind) in &names { outln(&paint(name, *kind)); }
        return Ok(());
    }

    // Columns filled across at the terminal's width
    let width = term_width();
    let longest = names.iter().map(|(n, _)| n.len()).max().unwrap_or(1) + 2;
    let per_line = (width / longest).max(1);
    let mut col = 0;
    let mut line = String::new();
    for (name, kind) in &names {
        // Pads the name before coloring, since escape sequences have no screen
        // width
        line.push_str(&paint(&pad_right(name, longest), *kind));
        col += 1;
        if col == per_line {
            outln(line.trim_end());
            line.clear();
            col = 0;
        }
    }
    if !line.is_empty() { outln(line.trim_end()); }
    Ok(())
}

/// One `ls -l` line before column widths are known
struct Row {
    kind:  u8,
    mode:  String,
    links: String,
    user:  String,
    group: String,
    size:  String,
    when:  String,
    name:  String,
}

/// The same line for a single file named on the command line
fn long_line(name: &str, st: &miku::MikuStat) -> String {
    let accounts = Accounts::load();
    format!("{} {} {} {} {} {} {}",
            mode_string(st.kind, st.mode),
            st.nlinks,
            accounts.user(st.uid),
            accounts.group(st.gid),
            st.size,
            stamp(st.mtime, now_secs()),
            name)
}

/// Name colored by kind: directories blue, executables green, links cyan,
/// devices yellow
fn paint(name: &str, kind: u8) -> String {
    if !to_terminal() { return String::from(name); }
    let colour = match kind {
        KIND_DIR     => "\x1b[1;34m",
        KIND_SYMLINK => "\x1b[1;36m",
        KIND_CHAR | KIND_BLOCK => "\x1b[1;33m",
        KIND_PIPE | KIND_FIFO | KIND_SOCKET => "\x1b[35m",
        _ => return String::from(name),
    };
    format!("{colour}{name}\x1b[0m")
}

/// Whether fd 1 is a terminal
fn to_terminal() -> bool {
    const TIOCGWINSZ: u64 = 0x5413;
    let mut ws = [0u16; 4];
    unsafe { miku::miku_ioctl(1, TIOCGWINSZ, ws.as_mut_ptr() as u64) >= 0 }
}

/// Terminal width from `TIOCGWINSZ`, or 80 when redirected
fn term_width() -> usize {
    const TIOCGWINSZ: u64 = 0x5413;
    let mut ws = [0u16; 4]; // rows, cols, xpixel, ypixel
    let rc = unsafe { miku::miku_ioctl(1, TIOCGWINSZ, ws.as_mut_ptr() as u64) };
    if rc < 0 || ws[1] == 0 { 80 } else { ws[1] as usize }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
