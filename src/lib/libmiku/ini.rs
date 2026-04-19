// INI config file parser
// Parses [section] key=value format
// No heap allocation - works with caller-provided buffers
// Supports comments (# and ;), trimming whitespace around keys/values

use crate::string;
use crate::mem;

const MAX_SECTIONS: usize = 32;
const MAX_ENTRIES: usize = 128;
const MAX_KEY_LEN: usize = 63;
const MAX_VAL_LEN: usize = 127;
const MAX_SEC_LEN: usize = 31;

#[repr(C)]
#[derive(Copy, Clone)]
struct IniEntry {
    section: [u8; MAX_SEC_LEN + 1],
    key: [u8; MAX_KEY_LEN + 1],
    val: [u8; MAX_VAL_LEN + 1],
    used: bool,
}

const EMPTY_ENTRY: IniEntry = IniEntry {
    section: [0u8; MAX_SEC_LEN + 1],
    key: [0u8; MAX_KEY_LEN + 1],
    val: [0u8; MAX_VAL_LEN + 1],
    used: false,
};

#[repr(C)]
pub struct MikuIni {
    entries: [IniEntry; MAX_ENTRIES],
    count: usize,
}

// create empty ini context
#[no_mangle]
pub extern "C" fn miku_ini_new() -> MikuIni {
    MikuIni {
        entries: [EMPTY_ENTRY; MAX_ENTRIES],
        count: 0,
    }
}

unsafe fn copy_trimmed(dst: *mut u8, max: usize, src: *const u8, len: usize) -> usize {
    // trim leading whitespace
    let mut start = 0usize;
    while start < len && (*src.add(start) == b' ' || *src.add(start) == b'\t') {
        start += 1;
    }
    // trim trailing whitespace
    let mut end = len;
    while end > start
        && (*src.add(end - 1) == b' '
            || *src.add(end - 1) == b'\t'
            || *src.add(end - 1) == b'\r'
            || *src.add(end - 1) == b'\n')
    {
        end -= 1;
    }
    let trimmed = end - start;
    let copy = if trimmed > max { max } else { trimmed };
    mem::miku_memcpy(dst, src.add(start), copy);
    *dst.add(copy) = 0;
    copy
}

// parse INI data from buffer
// Returns number of entries parsed, or -1 on error
#[no_mangle]
pub extern "C" fn miku_ini_parse(
    ini: *mut MikuIni,
    data: *const u8,
    data_len: usize,
) -> i32 {
    if ini.is_null() || data.is_null() || data_len == 0 {
        return -1;
    }

    let ini = unsafe { &mut *ini };
    ini.count = 0;

    let mut cur_section = [0u8; MAX_SEC_LEN + 1];
    let mut pos = 0usize;

    unsafe {
        while pos < data_len && ini.count < MAX_ENTRIES {
            // find end of line
            let line_start = pos;
            while pos < data_len && *data.add(pos) != b'\n' {
                pos += 1;
            }
            let line_end = pos;
            if pos < data_len { pos += 1; } // skip newline

            let line_len = line_end - line_start;
            if line_len == 0 { continue; }

            // skip whitespace at start
            let mut ls = line_start;
            while ls < line_end
                && (*data.add(ls) == b' ' || *data.add(ls) == b'\t')
            {
                ls += 1;
            }
            if ls >= line_end { continue; }

            let first = *data.add(ls);

            // comment
            if first == b'#' || first == b';' { continue; }

            // section header [name]
            if first == b'[' {
                let sec_start = ls + 1;
                let mut sec_end = sec_start;
                while sec_end < line_end && *data.add(sec_end) != b']' {
                    sec_end += 1;
                }
                copy_trimmed(
                    cur_section.as_mut_ptr(),
                    MAX_SEC_LEN,
                    data.add(sec_start),
                    sec_end - sec_start,
                );
                continue;
            }

            // key=value
            let mut eq_pos = ls;
            while eq_pos < line_end && *data.add(eq_pos) != b'=' {
                eq_pos += 1;
            }
            if eq_pos >= line_end { continue; } // no '='

            let entry = &mut ini.entries[ini.count];
            mem::miku_memcpy(
                entry.section.as_mut_ptr(),
                cur_section.as_ptr(),
                MAX_SEC_LEN + 1,
            );
            copy_trimmed(
                entry.key.as_mut_ptr(),
                MAX_KEY_LEN,
                data.add(ls),
                eq_pos - ls,
            );
            copy_trimmed(
                entry.val.as_mut_ptr(),
                MAX_VAL_LEN,
                data.add(eq_pos + 1),
                line_end - eq_pos - 1,
            );
            entry.used = true;
            ini.count += 1;
        }
    }

    ini.count as i32
}

// get value by section and key
// Returns pointer to value string, or null if not found
#[no_mangle]
pub extern "C" fn miku_ini_get(
    ini: *const MikuIni,
    section: *const u8,
    key: *const u8,
) -> *const u8 {
    if ini.is_null() || key.is_null() { return core::ptr::null(); }
    let ini = unsafe { &*ini };

    for i in 0..ini.count {
        let e = &ini.entries[i];
        if !e.used { continue; }

        // match section (null section = global/empty)
        if !section.is_null() {
            if string::miku_strcmp(e.section.as_ptr(), section) != 0 {
                continue;
            }
        } else if e.section[0] != 0 {
            continue;
        }

        if string::miku_strcmp(e.key.as_ptr(), key) == 0 {
            return e.val.as_ptr();
        }
    }
    core::ptr::null()
}

// get value as integer, with default
#[no_mangle]
pub extern "C" fn miku_ini_get_int(
    ini: *const MikuIni,
    section: *const u8,
    key: *const u8,
    default: i64,
) -> i64 {
    let val = miku_ini_get(ini, section, key);
    if val.is_null() { return default; }
    let n = crate::num::miku_atoi(val);
    if n == 0 && unsafe { *val } != b'0' { default } else { n }
}

// get value as bool ("true", "1", "yes" = true)
#[no_mangle]
pub extern "C" fn miku_ini_get_bool(
    ini: *const MikuIni,
    section: *const u8,
    key: *const u8,
    default: bool,
) -> bool {
    let val = miku_ini_get(ini, section, key);
    if val.is_null() { return default; }
    unsafe {
        let c = *val;
        c == b'1' || c == b't' || c == b'T' || c == b'y' || c == b'Y'
    }
}

// check if section exists
#[no_mangle]
pub extern "C" fn miku_ini_has_section(
    ini: *const MikuIni,
    section: *const u8,
) -> bool {
    if ini.is_null() || section.is_null() { return false; }
    let ini = unsafe { &*ini };
    for i in 0..ini.count {
        if ini.entries[i].used
            && string::miku_strcmp(ini.entries[i].section.as_ptr(), section) == 0
        {
            return true;
        }
    }
    false
}

// check if key exists
#[no_mangle]
pub extern "C" fn miku_ini_has_key(
    ini: *const MikuIni,
    section: *const u8,
    key: *const u8,
) -> bool {
    !miku_ini_get(ini, section, key).is_null()
}

// get number of entries
#[no_mangle]
pub extern "C" fn miku_ini_count(ini: *const MikuIni) -> usize {
    if ini.is_null() { return 0; }
    unsafe { (*ini).count }
}

// iterate over entries in a section
type IniCallback = extern "C" fn(*const u8, *const u8, *mut u8);

#[no_mangle]
pub extern "C" fn miku_ini_iter_section(
    ini: *const MikuIni,
    section: *const u8,
    cb: IniCallback,
    ctx: *mut u8,
) {
    if ini.is_null() { return; }
    let ini = unsafe { &*ini };
    for i in 0..ini.count {
        let e = &ini.entries[i];
        if !e.used { continue; }
        if !section.is_null() {
            if string::miku_strcmp(e.section.as_ptr(), section) != 0 {
                continue;
            }
        }
        cb(e.key.as_ptr(), e.val.as_ptr(), ctx);
    }
}
