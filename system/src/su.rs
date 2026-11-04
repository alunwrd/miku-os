#![no_std]
#![no_main]

//! su: front end for `mikudo`, rewriting its arguments and exec'ing /bin/mikudo
//!
//! Usage: su [-] [user] [-c command]

extern crate alloc;

pub use miku;

use alloc::vec::Vec;

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);

    let mut user = "root";
    let mut command: Vec<&str> = Vec::new();
    let mut i = 1usize;
    while i < args.len() {
        match args[i] {
            // A bare `-` is a login shell, as `-s` already gives
            "-" | "-l" | "--login" => {}
            "-c" => {
                i += 1;
                command.extend_from_slice(&args[i..]);
                break;
            }
            "-h" | "--help" => {
                out("usage: su [-] [user] [-c command]\n");
                miku::exit(0);
            }
            other if other.starts_with('-') => {
                out("su: unknown option\n");
                miku::exit(2);
            }
            other => user = other,
        }
        i += 1;
    }

    let mut argv_s: Vec<Vec<u8>> = Vec::new();
    argv_s.push(cstr("/bin/mikudo"));
    argv_s.push(cstr("-u"));
    argv_s.push(cstr(user));
    if command.is_empty() {
        argv_s.push(cstr("-s"));
    } else {
        // `--` keeps the command's options from mikudo's parser
        argv_s.push(cstr("--"));
        for c in command {
            argv_s.push(cstr(c));
        }
    }

    let ptrs: Vec<*const u8> = argv_s.iter().map(|v| v.as_ptr()).collect();
    let path = cstr("/bin/mikudo");
    unsafe {
        miku::miku_exec(path.as_ptr(), ptrs.as_ptr(), ptrs.len());
    }
    out("su: cannot run /bin/mikudo\n");
    miku::exit(127);
}

fn out(s: &str) {
    miku::write(2, s.as_bytes());
}

fn cstr(s: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    v
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
