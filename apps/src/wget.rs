#![no_std]
#![no_main]

//! wget: fetches a URL

extern crate alloc;

pub use miku;

use alloc::string::String;
use alloc::vec::Vec;
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
    miku::println("usage: wget <url> [-O <file>] [--insecure] [-v]");
    miku::println("  -O <file>   write the body here; '-' is standard output");
    miku::println("  --insecure  connect even when the certificate does not check out");
    miku::println("  -v          say what the connection is doing");
    miku::exit(2);
}

/// Save name without `-O`: the URL's last path component, or `index.html`
fn infer_filename(url: &str) -> String {
    let after_scheme = url.split("://").last().unwrap_or(url);
    let path = match after_scheme.find('/') {
        Some(i) => &after_scheme[i + 1..],
        None => "",
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    let last = path.rsplit('/').next().unwrap_or("");
    if last.is_empty() {
        String::from("index.html")
    } else {
        String::from(last)
    }
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

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);

    let mut url: Option<&str> = None;
    let mut out: Option<&str> = None;
    let mut insecure = false;
    let mut i = 1usize;

    while i < args.len() {
        match args[i] {
            "-O" => {
                i += 1;
                if i >= args.len() {
                    usage();
                }
                out = Some(args[i]);
            }
            "--insecure" | "-k" => insecure = true,
            "-v" => mikunet::plat::set_verbose(true),
            "-h" | "--help" => usage(),
            other if other.starts_with('-') => {
                err(&["wget: unknown option ", other]);
                usage();
            }
            other => {
                if url.is_some() {
                    usage();
                }
                url = Some(other);
            }
        }
        i += 1;
    }

    let url = match url {
        Some(u) => u,
        None => usage(),
    };

    // No trust store means no connection
    let mut trust = match mikunet::verify::system_trust() {
        Ok(t) => t,
        Err(why) => {
            if insecure {
                err(&[
                    "wget: no trust store (",
                    why,
                    "), continuing because --insecure",
                ]);
                Trust::empty()
            } else {
                err(&[
                    "wget: cannot read ",
                    mikunet::verify::SYSTEM_TRUST,
                    ": ",
                    why,
                ]);
                err(&["  without it no certificate can be checked; --insecure connects anyway"]);
                miku::exit(1);
            }
        }
    };
    trust.insecure = insecure;
    if insecure {
        err(&["wget: certificates are not being checked, this connection is not authenticated"]);
    }

    let client = match mikunet::http::Client::from_system(&trust) {
        Ok(c) => c,
        Err(why) => {
            err(&["wget: ", why.message()]);
            miku::exit(1);
        }
    };

    let resp = match mikunet::http::get(url, &client) {
        Some(r) => r,
        None => {
            err(&["wget: ", url, ": failed"]);
            miku::exit(1);
        }
    };

    say(&["HTTP ", &num(resp.status as u64), " ", &resp.reason]);
    if resp.body.is_empty() {
        err(&["wget: empty response"]);
        miku::exit(1);
    }
    say(&["wget: ", &num(resp.body.len() as u64), " bytes received"]);

    let dest = match out {
        Some("-") => {
            unsafe { miku::miku_write(1, resp.body.as_ptr(), resp.body.len()) };
            miku::exit(0);
        }
        Some(p) => String::from(p),
        None => infer_filename(url),
    };

    match write_file(&dest, &resp.body) {
        Ok(n) => say(&["wget: '", &dest, "' saved (", &num(n as u64), " bytes)"]),
        Err(e) => {
            err(&["wget: cannot save '", &dest, "': ", e]);
            miku::exit(1);
        }
    }
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::eprintln("wget: panic");
    miku::exit(1);
}

/// Keep the linker from dropping the crate when nothing else references it
#[allow(dead_code)]
fn _keep(_: &Vec<u8>) {}
