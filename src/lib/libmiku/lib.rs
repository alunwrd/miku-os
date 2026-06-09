#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// core primitives
pub mod sys;
pub mod errno;
pub mod sync;
pub mod panic;

// memory and allocation
pub mod mem;
pub mod heap;
pub mod arena;
pub mod slab;
pub mod pool;

// process and system
pub mod proc;
pub mod io;
pub mod net;
pub mod signal;
pub mod env;
pub mod time;
pub mod timer;

// strings and text
pub mod string;
pub mod ctype;
pub mod convert;
pub mod num;
pub mod utf8;
pub mod strbuf;
pub mod format;

// formatted I/O and files
pub mod stdio;
pub mod fmt;
pub mod file;
pub mod bufio;
pub mod dir;
pub mod path;
pub mod glob;

// data structures
pub mod vec;
pub mod list;
pub mod queue;
pub mod hashmap;
pub mod treemap;
pub mod trie;
pub mod ringbuf;
pub mod ringbuf2;
pub mod bitset;
pub mod heap_queue;
pub mod channel;

// algorithms
pub mod sort;
pub mod hash;
pub mod bitops;
pub mod math;
pub mod random;

// encoding
pub mod base64;
pub mod hex;
pub mod endian;
pub mod checksum;
pub mod sha256;
pub mod uuid;
pub mod lz;

// parsing and config
pub mod json;
pub mod csv;
pub mod ini;
pub mod regex;
pub mod getopt;
pub mod args;

// date/time and events
pub mod datetime;
pub mod event;

// logging and testing
pub mod log;
pub mod test;

// POSIX/C libc compatibility layer
pub mod libc;

#[no_mangle]
#[link_section = ".text._libmiku_start"]
pub extern "C" fn _libmiku_start() -> ! { loop {} }

#[panic_handler]
fn rust_panic(_: &core::panic::PanicInfo) -> ! {
    io::miku_write(2, b"libmiku: panic\n".as_ptr(), 15);
    proc::miku_exit(127);
}
