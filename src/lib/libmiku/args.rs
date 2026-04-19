// Higher-level argument handling than getopt
// Supports named flags, options with values, positional args

use crate::string;
use crate::num;
use crate::mem;

const MAX_OPTS: usize = 32;
const MAX_ARGS: usize = 64;

// FFI-safe string slice
#[repr(C)]
#[derive(Copy, Clone)]
pub struct ArgSlice {
    pub ptr: *const u8,
    pub len: usize,
}

const EMPTY_SLICE: ArgSlice = ArgSlice { ptr: core::ptr::null(), len: 0 };

// option type
pub const ARG_FLAG: u8 = 0;   // boolean flag, no value
pub const ARG_VALUE: u8 = 1;  // option with string value
pub const ARG_INT: u8 = 2;    // option with integer value

// option definition
#[repr(C)]
#[derive(Copy, Clone)]
struct OptDef {
    short: u8,                // short flag char, 0 if none
    long: [u8; 32],           // long name (null-terminated)
    kind: u8,                 // ARG_FLAG / ARG_VALUE / ARG_INT
    value: [u8; 64],          // parsed string value
    int_value: i64,           // parsed integer value
    present: bool,            // was this option seen?
    help: [u8; 64],           // help text
}

const EMPTY_OPT: OptDef = OptDef {
    short: 0,
    long: [0; 32],
    kind: ARG_FLAG,
    value: [0; 64],
    int_value: 0,
    present: false,
    help: [0; 64],
};

// argument parser state
#[repr(C)]
pub struct MikuArgs {
    opts: [OptDef; MAX_OPTS],
    opt_count: usize,
    positional: [ArgSlice; MAX_ARGS],
    pos_count: usize,
    program: ArgSlice,
    error: [u8; 128],
    has_error: bool,
}

// create argument parser
#[no_mangle]
pub extern "C" fn miku_args_new() -> MikuArgs {
    MikuArgs {
        opts: [EMPTY_OPT; MAX_OPTS],
        opt_count: 0,
        positional: [EMPTY_SLICE; MAX_ARGS],
        pos_count: 0,
        program: EMPTY_SLICE,
        error: [0; 128],
        has_error: false,
    }
}

// add flag option (boolean, no value)
#[no_mangle]
pub extern "C" fn miku_args_flag(
    a: *mut MikuArgs,
    short: u8,
    long: *const u8,
    help: *const u8,
) {
    add_opt(a, short, long, ARG_FLAG, help);
}

// add string option
#[no_mangle]
pub extern "C" fn miku_args_option(
    a: *mut MikuArgs,
    short: u8,
    long: *const u8,
    help: *const u8,
) {
    add_opt(a, short, long, ARG_VALUE, help);
}

// add integer option
#[no_mangle]
pub extern "C" fn miku_args_int_option(
    a: *mut MikuArgs,
    short: u8,
    long: *const u8,
    help: *const u8,
) {
    add_opt(a, short, long, ARG_INT, help);
}

fn add_opt(a: *mut MikuArgs, short: u8, long: *const u8, kind: u8, help: *const u8) {
    if a.is_null() { return; }
    let a = unsafe { &mut *a };
    if a.opt_count >= MAX_OPTS { return; }

    let mut opt = EMPTY_OPT;
    opt.short = short;
    opt.kind = kind;

    if !long.is_null() {
        let len = string::miku_strlen(long);
        let copy = if len > 31 { 31 } else { len };
        unsafe { mem::miku_memcpy(opt.long.as_mut_ptr(), long, copy); }
    }

    if !help.is_null() {
        let len = string::miku_strlen(help);
        let copy = if len > 63 { 63 } else { len };
        unsafe { mem::miku_memcpy(opt.help.as_mut_ptr(), help, copy); }
    }

    a.opts[a.opt_count] = opt;
    a.opt_count += 1;
}

// parse argument list
// argv: array of ArgSlice, argc: number of arguments
#[no_mangle]
pub extern "C" fn miku_args_parse(
    a: *mut MikuArgs,
    argv: *const ArgSlice,
    argc: usize,
) -> bool {
    if a.is_null() || argv.is_null() { return false; }
    let a = unsafe { &mut *a };
    a.has_error = false;
    a.pos_count = 0;

    if argc == 0 { return true; }

    // first arg is program name
    let s0 = unsafe { *argv.add(0) };
    a.program = s0;

    let mut i = 1usize;
    let mut after_dashdash = false;

    while i < argc {
        let s = unsafe { *argv.add(i) };
        let ptr = s.ptr;
        let len = s.len;
        if ptr.is_null() || len == 0 { i += 1; continue; }

        let first = unsafe { *ptr };

        // after everything is positional
        if after_dashdash {
            if a.pos_count < MAX_ARGS {
                a.positional[a.pos_count] = ArgSlice { ptr, len };
                a.pos_count += 1;
            }
            i += 1;
            continue;
        }

        // check for separator
        if len == 2 && first == b'-' && unsafe { *ptr.add(1) } == b'-' {
            after_dashdash = true;
            i += 1;
            continue;
        }

        // long option --name or --name=value
        if len > 2 && first == b'-' && unsafe { *ptr.add(1) } == b'-' {
            let name_start = unsafe { ptr.add(2) };
            let name_len = len - 2;

            // check for =
            let mut eq_pos = name_len;
            for j in 0..name_len {
                if unsafe { *name_start.add(j) } == b'=' {
                    eq_pos = j;
                    break;
                }
            }

            let actual_name_len = eq_pos;
            let opt_idx = find_long(a, name_start, actual_name_len);

            if opt_idx >= a.opt_count {
                set_error(a, b"unknown option: --", name_start, actual_name_len);
                return false;
            }

            if a.opts[opt_idx].kind == ARG_FLAG {
                a.opts[opt_idx].present = true;
            } else if eq_pos < name_len {
                // value after =
                let val_ptr = unsafe { name_start.add(eq_pos + 1) };
                let val_len = name_len - eq_pos - 1;
                set_opt_value(&mut a.opts[opt_idx], val_ptr, val_len);
            } else {
                // next arg is value
                i += 1;
                if i >= argc {
                    set_error(a, b"missing value for: --", name_start, actual_name_len);
                    return false;
                }
                let vs = unsafe { *argv.add(i) };
                set_opt_value(&mut a.opts[opt_idx], vs.ptr, vs.len);
            }

            i += 1;
            continue;
        }

        // short option -x
        if len >= 2 && first == b'-' {
            let ch = unsafe { *ptr.add(1) };
            let opt_idx = find_short(a, ch);

            if opt_idx >= a.opt_count {
                set_error(a, b"unknown option: -", &ch as *const u8, 1);
                return false;
            }

            if a.opts[opt_idx].kind == ARG_FLAG {
                a.opts[opt_idx].present = true;
                // allow grouped short flags like -abc
                // previously silently ignored unknown chars or non-flag
                // options in the group, so '-abc' where -b takes a value
                // would drop -b entirely. Now we reject the group explicitly.
                for j in 2..len {
                    let ch2 = unsafe { *ptr.add(j) };
                    let idx2 = find_short(a, ch2);
                    if idx2 >= a.opt_count {
                        set_error(a, b"unknown option: -", unsafe { ptr.add(j) }, 1);
                        return false;
                    }
                    if a.opts[idx2].kind != ARG_FLAG {
                        set_error(a, b"option requires a value, cannot be grouped: -", unsafe { ptr.add(j) }, 1);
                        return false;
                    }
                    a.opts[idx2].present = true;
                }
            } else if len > 2 {
                // value attached: -nVALUE
                let val_ptr = unsafe { ptr.add(2) };
                set_opt_value(&mut a.opts[opt_idx], val_ptr, len - 2);
            } else {
                // next arg is value
                i += 1;
                if i >= argc {
                    set_error(a, b"missing value for: -", &ch as *const u8, 1);
                    return false;
                }
                let vs = unsafe { *argv.add(i) };
                set_opt_value(&mut a.opts[opt_idx], vs.ptr, vs.len);
            }

            i += 1;
            continue;
        }

        // positional argument
        if a.pos_count < MAX_ARGS {
            a.positional[a.pos_count] = ArgSlice { ptr, len };
            a.pos_count += 1;
        }
        i += 1;
    }

    true
}

fn find_long(a: &MikuArgs, name: *const u8, len: usize) -> usize {
    for i in 0..a.opt_count {
        let olen = string::miku_strlen(a.opts[i].long.as_ptr());
        if olen == len {
            if mem::miku_memcmp(a.opts[i].long.as_ptr(), name, len) == 0 {
                return i;
            }
        }
    }
    a.opt_count // not found
}

fn find_short(a: &MikuArgs, ch: u8) -> usize {
    for i in 0..a.opt_count {
        if a.opts[i].short == ch { return i; }
    }
    a.opt_count
}

fn set_opt_value(opt: &mut OptDef, ptr: *const u8, len: usize) {
    opt.present = true;
    let copy = if len > 63 { 63 } else { len };
    unsafe { mem::miku_memcpy(opt.value.as_mut_ptr(), ptr, copy); }
    opt.value[copy] = 0;

    if opt.kind == ARG_INT {
        opt.int_value = num::miku_atoi(opt.value.as_ptr());
    }
}

fn set_error(a: &mut MikuArgs, prefix: &[u8], name: *const u8, len: usize) {
    a.has_error = true;
    let plen = prefix.len();
    let copy_p = if plen > 120 { 120 } else { plen };
    unsafe { mem::miku_memcpy(a.error.as_mut_ptr(), prefix.as_ptr(), copy_p); }
    let copy_n = if len > (127 - copy_p) { 127 - copy_p } else { len };
    unsafe { mem::miku_memcpy(a.error.as_mut_ptr().add(copy_p), name, copy_n); }
    a.error[copy_p + copy_n] = 0;
}

// check if flag/option was provided
#[no_mangle]
pub extern "C" fn miku_args_has(a: *const MikuArgs, long: *const u8) -> bool {
    if a.is_null() || long.is_null() { return false; }
    let a = unsafe { &*a };
    let len = string::miku_strlen(long);
    let idx = find_long(a, long, len);
    if idx < a.opt_count { a.opts[idx].present } else { false }
}

// get string value of option
#[no_mangle]
pub extern "C" fn miku_args_get(a: *const MikuArgs, long: *const u8) -> *const u8 {
    if a.is_null() || long.is_null() { return core::ptr::null(); }
    let a = unsafe { &*a };
    let len = string::miku_strlen(long);
    let idx = find_long(a, long, len);
    if idx < a.opt_count && a.opts[idx].present {
        a.opts[idx].value.as_ptr()
    } else {
        core::ptr::null()
    }
}

// get integer value of option
#[no_mangle]
pub extern "C" fn miku_args_get_int(a: *const MikuArgs, long: *const u8) -> i64 {
    if a.is_null() || long.is_null() { return 0; }
    let a = unsafe { &*a };
    let len = string::miku_strlen(long);
    let idx = find_long(a, long, len);
    if idx < a.opt_count && a.opts[idx].present {
        a.opts[idx].int_value
    } else {
        0
    }
}

// get positional argument count
#[no_mangle]
pub extern "C" fn miku_args_positional_count(a: *const MikuArgs) -> usize {
    if a.is_null() { return 0; }
    unsafe { (*a).pos_count }
}

// get positional argument by index
#[no_mangle]
pub extern "C" fn miku_args_positional(
    a: *const MikuArgs,
    idx: usize,
    out_len: *mut usize,
) -> *const u8 {
    if a.is_null() { return core::ptr::null(); }
    let a = unsafe { &*a };
    if idx >= a.pos_count { return core::ptr::null(); }
    let ArgSlice { ptr, len } = a.positional[idx];
    if !out_len.is_null() {
        unsafe { *out_len = len; }
    }
    ptr
}

// check if parse error occurred
#[no_mangle]
pub extern "C" fn miku_args_has_error(a: *const MikuArgs) -> bool {
    if a.is_null() { return false; }
    unsafe { (*a).has_error }
}

// get error message
#[no_mangle]
pub extern "C" fn miku_args_error(a: *const MikuArgs) -> *const u8 {
    if a.is_null() { return b"\0".as_ptr(); }
    unsafe { (*a).error.as_ptr() }
}
