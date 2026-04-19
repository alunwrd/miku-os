// Command line argument parser
// POSIX-style getopt for parsing argc/argv
// Supports short options (-a, -abc, -o value)
// Also provides a simpler key=value parser for MikuOS apps

use crate::string;
use crate::mem;

// getopt state
#[repr(C)]
pub struct MikuGetopt {
    argv: *const *const u8,
    argc: i32,
    optind: i32,     // current argument index
    optpos: i32,     // position within current arg (for grouped flags)
    optarg: *const u8, // argument to current option
    optopt: u8,      // current option character
    finished: bool,
}

// initialize getopt
#[no_mangle]
pub extern "C" fn miku_getopt_init(
    g: *mut MikuGetopt,
    argc: i32,
    argv: *const *const u8,
) {
    if g.is_null() {
        return;
    }
    unsafe {
        (*g).argv = argv;
        (*g).argc = argc;
        (*g).optind = 1; // skip argv[0]
        (*g).optpos = 0;
        (*g).optarg = core::ptr::null();
        (*g).optopt = 0;
        (*g).finished = false;
    }
}

// Get next option
// optstring: "abc:d" means -a, -b (no arg), -c (requires arg), -d (no arg)
// Returns option character, '?' for unknown, ':' for missing arg, -1 when done
#[no_mangle]
pub extern "C" fn miku_getopt_next(g: *mut MikuGetopt, optstring: *const u8) -> i32 {
    if g.is_null() || optstring.is_null() {
        return -1;
    }
    unsafe {
        let g = &mut *g;
        g.optarg = core::ptr::null();

        if g.finished {
            return -1;
        }

        loop {
            if g.optind >= g.argc {
                g.finished = true;
                return -1;
            }

            let arg = *g.argv.add(g.optind as usize);
            if arg.is_null() {
                g.finished = true;
                return -1;
            }

            // if no position yet, check if this arg starts with '-'
            if g.optpos == 0 {
                if *arg != b'-' || *arg.add(1) == 0 {
                    // not an option
                    g.finished = true;
                    return -1;
                }
                // "--" stops option parsing
                if *arg.add(1) == b'-' && *arg.add(2) == 0 {
                    g.optind += 1;
                    g.finished = true;
                    return -1;
                }
                g.optpos = 1;
            }

            let c = *arg.add(g.optpos as usize);
            if c == 0 {
                // end of this argument, move to next
                g.optind += 1;
                g.optpos = 0;
                continue;
            }

            g.optopt = c;
            g.optpos += 1;

            // look up in optstring
            let mut oi = 0usize;
            let mut found = false;
            let mut needs_arg = false;
            while *optstring.add(oi) != 0 {
                if *optstring.add(oi) == c {
                    found = true;
                    if *optstring.add(oi + 1) == b':' {
                        needs_arg = true;
                    }
                    break;
                }
                oi += 1;
            }

            if !found {
                return b'?' as i32;
            }

            if needs_arg {
                // check if rest of current arg is the value
                let rest = arg.add(g.optpos as usize);
                if *rest != 0 {
                    g.optarg = rest;
                    g.optind += 1;
                    g.optpos = 0;
                } else {
                    // next argument is the value
                    g.optind += 1;
                    g.optpos = 0;
                    if g.optind >= g.argc {
                        return b':' as i32; // missing argument
                    }
                    g.optarg = *g.argv.add(g.optind as usize);
                    g.optind += 1;
                }
            }

            return c as i32;
        }
    }
}

// Get current optind (index of next non-option argument)
#[no_mangle]
pub extern "C" fn miku_getopt_optind(g: *const MikuGetopt) -> i32 {
    if g.is_null() {
        return 0;
    }
    unsafe { (*g).optind }
}

// get optarg (argument to current option)
#[no_mangle]
pub extern "C" fn miku_getopt_optarg(g: *const MikuGetopt) -> *const u8 {
    if g.is_null() {
        return core::ptr::null();
    }
    unsafe { (*g).optarg }
}

// simple key=value argument helpers //

// find value for key in argv (searches "key=value" patterns)
// Returns pointer to value part, or null if not found.
#[no_mangle]
pub extern "C" fn miku_argv_get(
    argc: i32,
    argv: *const *const u8,
    key: *const u8,
) -> *const u8 {
    if argv.is_null() || key.is_null() {
        return core::ptr::null();
    }
    let keylen = string::miku_strlen(key);
    if keylen == 0 {
        return core::ptr::null();
    }

    unsafe {
        for i in 0..argc as usize {
            let arg = *argv.add(i);
            if arg.is_null() { continue; }
            // check if arg starts with key=
            if string::miku_strncmp(arg, key, keylen) == 0 && *arg.add(keylen) == b'=' {
                return arg.add(keylen + 1);
            }
        }
    }
    core::ptr::null()
}

// check if flag exists in argv (e.g., "--verbose" or "-v")
#[no_mangle]
pub extern "C" fn miku_argv_has(
    argc: i32,
    argv: *const *const u8,
    flag: *const u8,
) -> bool {
    if argv.is_null() || flag.is_null() {
        return false;
    }
    unsafe {
        for i in 0..argc as usize {
            let arg = *argv.add(i);
            if arg.is_null() { continue; }
            if string::miku_strcmp(arg, flag) == 0 {
                return true;
            }
        }
    }
    false
}

// count non-option arguments (those after "--" or not starting with "-")
#[no_mangle]
pub extern "C" fn miku_argv_positional_count(
    argc: i32,
    argv: *const *const u8,
) -> i32 {
    if argv.is_null() {
        return 0;
    }
    let mut count = 0i32;
    let mut after_sep = false;
    unsafe {
        for i in 1..argc as usize { // skip argv[0]
            let arg = *argv.add(i);
            if arg.is_null() { continue; }
            if !after_sep && *arg == b'-' && *arg.add(1) == b'-' && *arg.add(2) == 0 {
                after_sep = true;
                continue;
            }
            if after_sep || *arg != b'-' {
                count += 1;
            }
        }
    }
    count
}
