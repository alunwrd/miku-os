#![no_std]
#![no_main]

//! curl: makes one HTTP request and shows the exchange. Shares its library with
//! `wget`

extern crate alloc;

pub use miku;

use alloc::string::String;
use mikunet::verify::Trust;

fn say(parts: &[&str]) {
    for p in parts {
        miku::print(p);
    }
    miku::println("");
}

fn err(parts: &[&str]) {
    for p in parts {
        miku::eprint(p);
    }
    miku::eprintln("");
}

fn num(v: u64) -> String {
    let mut buf = [0u8; 24];
    let n = miku::fmt_dec(&mut buf, v);
    String::from(core::str::from_utf8(&buf[..n]).unwrap_or("?"))
}

fn usage() -> ! {
    miku::println("usage: curl <url> [-X GET|POST] [-d <data>] [-o <file>] [-I] [--insecure] [-v]");
    miku::println("  -X <method>  GET or POST");
    miku::println("  -d <data>    a body, which implies POST");
    miku::println("  -o <file>    write the body to a file instead of the screen");
    miku::println("  -I           headers only");
    miku::println("  --insecure   connect even when the certificate does not check out");
    miku::exit(2);
}

fn write_file(path: &str, data: &[u8]) -> Result<usize, &'static str> {
    const O_WRITE: u32 = 0x0002;
    const O_CREATE: u32 = 0x0008;
    const O_TRUNCATE: u32 = 0x0020;

    let mut cpath = [0u8; 256];
    let n = path.len().min(cpath.len() - 1);
    cpath[..n].copy_from_slice(&path.as_bytes()[..n]);

    let fd = unsafe { miku::miku_open(cpath.as_ptr(), n, O_WRITE | O_CREATE | O_TRUNCATE, 0o644) };
    if fd < 0 {
        return Err("cannot open for writing");
    }
    let mut off = 0usize;
    while off < data.len() {
        let w = unsafe { miku::miku_write_fd(fd, data[off..].as_ptr(), data.len() - off) };
        if w <= 0 {
            unsafe { miku::miku_close(fd) };
            return Err("write failed");
        }
        off += w as usize;
    }
    unsafe { miku::miku_close(fd) };
    Ok(off)
}

/// Prints a body with unprintable bytes as dots, protecting the terminal
fn print_body(body: &[u8]) {
    const LIMIT: usize = 4096;
    let show = body.len().min(LIMIT);
    let mut text = String::with_capacity(show);
    for &b in &body[..show] {
        if b == b'\n' || b == b'\t' || (32..=126).contains(&b) {
            text.push(b as char);
        } else if b != b'\r' {
            text.push('.');
        }
    }
    miku::println(&text);
    if body.len() > show {
        say(&["... (", &num(body.len() as u64), " bytes total, showing the first 4096)"]);
    }
}

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);

    let mut url: Option<&str> = None;
    let mut method = "GET";
    let mut data: Option<&str> = None;
    let mut out: Option<&str> = None;
    let mut head_only = false;
    let mut insecure = false;
    let mut i = 1usize;

    while i < args.len() {
        match args[i] {
            "-X" => { i += 1; if i >= args.len() { usage(); } method = args[i]; }
            "-d" => { i += 1; if i >= args.len() { usage(); } data = Some(args[i]); }
            "-o" => { i += 1; if i >= args.len() { usage(); } out = Some(args[i]); }
            "-I" => head_only = true,
            "--insecure" | "-k" => insecure = true,
            "-v" => mikunet::plat::set_verbose(true),
            "-h" | "--help" => usage(),
            other if other.starts_with('-') => { err(&["curl: unknown option ", other]); usage(); }
            other => { if url.is_some() { usage(); } url = Some(other); }
        }
        i += 1;
    }

    let url = match url { Some(u) => u, None => usage() };
    // A body implies POST
    if data.is_some() && method == "GET" {
        method = "POST";
    }

    let mut trust = match mikunet::verify::system_trust() {
        Ok(t) => t,
        Err(why) => {
            if insecure {
                err(&["curl: no trust store (", why, ") - continuing because --insecure"]);
                Trust::empty()
            } else {
                err(&["curl: cannot read ", mikunet::verify::SYSTEM_TRUST, ": ", why]);
                err(&["  without it no certificate can be checked; --insecure connects anyway"]);
                miku::exit(1);
            }
        }
    };
    trust.insecure = insecure;
    if insecure {
        err(&["curl: certificates are not being checked - this connection is not authenticated"]);
    }

    let client = match mikunet::http::Client::from_system(&trust) {
        Ok(c) => c,
        Err(why) => {
            err(&["curl: ", why.message()]);
            miku::exit(1);
        }
    };

    let resp = match method {
        "POST" => mikunet::http::post(
            url,
            data.unwrap_or("").as_bytes(),
            "application/x-www-form-urlencoded",
            &client,
        ),
        "GET" => mikunet::http::get(url, &client),
        other => {
            err(&["curl: unsupported method ", other]);
            miku::exit(2);
        }
    };

    let resp = match resp {
        Some(r) => r,
        None => {
            err(&["curl: ", url, ": failed"]);
            miku::exit(1);
        }
    };

    say(&["< HTTP ", &num(resp.status as u64), " ", &resp.reason]);
    if let Some(cl) = resp.content_length {
        say(&["< Content-Length: ", &num(cl as u64)]);
    }
    if let Some(ref loc) = resp.location {
        say(&["< Location: ", loc]);
    }

    if head_only {
        miku::exit(if resp.status < 400 { 0 } else { 1 });
    }

    if resp.body.is_empty() {
        err(&["curl: empty body"]);
        miku::exit(if resp.status < 400 { 0 } else { 1 });
    }

    match out {
        Some(path) => match write_file(path, &resp.body) {
            Ok(n) => say(&["curl: saved '", path, "' (", &num(n as u64), " bytes)"]),
            Err(e) => {
                err(&["curl: cannot save '", path, "': ", e]);
                miku::exit(1);
            }
        },
        None => print_body(&resp.body),
    }

    miku::exit(if resp.status < 400 { 0 } else { 1 });
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::eprintln("curl: panic");
    miku::exit(1);
}
