#![no_std]
#![no_main]

//! chmod: changes permission bits
//!
//! Usage: chmod [-R] mode file...
//!
//! chmod 644 f            octal
//! chmod u+x f            symbolic
//! chmod go-w,a+r f       several clauses, left to right
//!
//! Grammar: `[ugoa]*[+-=][rwx]*`, comma-separated. `s`, `t`, and `X` are
//! rejected: MikuOS has no setuid bit or sticky directories.

extern crate alloc;

pub use miku;

use alloc::format;
use alloc::vec::Vec;

#[path = "common.rs"]
mod common;
use common::*;

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);
    let mut recurse = false;
    let mut rest: Vec<&str> = Vec::new();

    for a in args.iter().skip(1) {
        match *a {
            "-R" | "-r" => recurse = true,
            "-h" | "--help" => { outln("usage: chmod [-R] mode file..."); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 && rest.is_empty() => {
                errln(&format!("chmod: unknown option {other}"));
                miku::exit(2);
            }
            other => rest.push(other),
        }
    }
    if rest.len() < 2 {
        errln("usage: chmod [-R] mode file...");
        miku::exit(2);
    }

    let spec = rest[0];
    // Octal is absolute; symbolic is computed per file below
    let octal = parse_octal(spec);
    if octal.is_none() && parse_clauses(spec).is_none() {
        errln(&format!("chmod: {spec}: neither an octal mode nor [ugoa][+-=][rwx]"));
        miku::exit(2);
    }

    let mut bad = false;
    for f in &rest[1..] {
        if let Err(rc) = apply(f, spec, octal, recurse) {
            complain("chmod", f, &reason(rc));
            bad = true;
        }
    }
    miku::exit(if bad { 1 } else { 0 });
}

fn apply(path: &str, spec: &str, octal: Option<u32>, recurse: bool) -> Result<(), i64> {
    let st = stat(path)?;
    let mode = match octal {
        Some(m) => m,
        // Read, change, write, per file
        None => match parse_clauses(spec) {
            Some(clauses) => rewrite(st.mode & 0o777, &clauses),
            None => return Err(-22),
        },
    };

    let c = cpath(path).ok_or(-36i64)?;
    let rc = unsafe { miku::miku_chmod(c.as_ptr(), mode) };
    if rc < 0 { return Err(rc); }

    if !recurse || !is_dir(&st) { return Ok(()); }
    for (name, _) in read_dir(path)? {
        if name == "." || name == ".." { continue; }
        apply(&join(path, &name), spec, octal, true)?;
    }
    Ok(())
}

/// One `[ugoa]*[+-=][rwx]*` clause, as parsed
#[derive(Clone, Copy)]
struct Clause {
    /// Which of owner, group and other it touches, as a 3-bit mask
    who: u8,
    op:  u8,
    /// The permission bits it names, in the low three bits
    what: u8,
}

const WHO_USER:  u8 = 1 << 2;
const WHO_GROUP: u8 = 1 << 1;
const WHO_OTHER: u8 = 1;

/// Splits `go-w,a+r` into clauses. `None` if any part is invalid, so a partly
/// understood spec changes nothing
fn parse_clauses(spec: &str) -> Option<alloc::vec::Vec<Clause>> {
    let mut out = alloc::vec::Vec::new();
    for part in spec.split(',') {
        if part.is_empty() { return None; }
        let mut who = 0u8;
        let mut bytes = part.bytes().peekable();
        while let Some(&b) = bytes.peek() {
            match b {
                b'u' => who |= WHO_USER,
                b'g' => who |= WHO_GROUP,
                b'o' => who |= WHO_OTHER,
                b'a' => who |= WHO_USER | WHO_GROUP | WHO_OTHER,
                _ => break,
            }
            bytes.next();
        }
        // No `who` means `a`. The umask is not applied
        if who == 0 { who = WHO_USER | WHO_GROUP | WHO_OTHER; }

        let op = match bytes.next() {
            Some(c @ (b'+' | b'-' | b'=')) => c,
            _ => return None,
        };

        let mut what = 0u8;
        for b in bytes {
            match b {
                b'r' => what |= 0o4,
                b'w' => what |= 0o2,
                b'x' => what |= 0o1,
                b's' | b't' | b'X' => return None,   // see the note at the top
                _ => return None,
            }
        }
        out.push(Clause { who, op, what });
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Apply the clauses to a mode, left to right
fn rewrite(mut mode: u32, clauses: &[Clause]) -> u32 {
    for c in clauses {
        for (bit, shift) in [(WHO_USER, 6), (WHO_GROUP, 3), (WHO_OTHER, 0)] {
            if c.who & bit == 0 { continue; }
            let field = (c.what as u32) << shift;
            let mask = 0o7u32 << shift;
            mode = match c.op {
                b'+' => mode | field,
                b'-' => mode & !field,
                _    => (mode & !mask) | field,
            };
        }
    }
    mode & 0o777
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
