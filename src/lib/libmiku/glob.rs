// Glob-style pattern matching
//
// Supports:
//   *      - match any sequence (including empty)
//   ?      - match exactly one character
//   [abc]  - match any character in set
//   [a-z]  - match any character in range
//   [^abc] - match any character NOT in set
//   \      - escape next character
//
// This is useful for file name matching, command line
// wildcard expansion, and simple filtering

use crate::string;

// match character class [...]
// Returns (matched, new_pattern_index after ']')
unsafe fn match_class(pat: *const u8, mut pi: usize, ch: u8, nocase: bool) -> (bool, usize) {
    let negate = *pat.add(pi) == b'^' || *pat.add(pi) == b'!';
    if negate { pi += 1; }

    let fold = |b: u8| if nocase { to_lower(b) } else { b };
    let ch_f = fold(ch);

    let mut matched = false;
    let mut first = true;

    loop {
        let c = *pat.add(pi);
        if c == 0 { return (false, pi); } // unterminated class
        if c == b']' && !first { pi += 1; break; }
        first = false;

        // check for range: a-z
        if *pat.add(pi + 1) == b'-' && *pat.add(pi + 2) != 0 && *pat.add(pi + 2) != b']' {
            let lo = fold(c);
            let hi = fold(*pat.add(pi + 2));
            if ch_f >= lo && ch_f <= hi {
                matched = true;
            }
            pi += 3;
        } else {
            if ch_f == fold(c) {
                matched = true;
            }
            pi += 1;
        }
    }

    if negate { matched = !matched; }
    (matched, pi)
}

// core glob matching with [class] support
unsafe fn glob_core(pat: *const u8, mut pi: usize, txt: *const u8, mut ti: usize, nocase: bool) -> bool {
    loop {
        let pc = *pat.add(pi);
        let tc = *txt.add(ti);

        if pc == 0 {
            return tc == 0;
        }

        match pc {
            b'*' => {
                while *pat.add(pi) == b'*' { pi += 1; }
                if *pat.add(pi) == 0 { return true; }
                let mut pos = ti;
                while *txt.add(pos) != 0 {
                    if glob_core(pat, pi, txt, pos, nocase) { return true; }
                    pos += 1;
                }
                return glob_core(pat, pi, txt, pos, nocase);
            }
            b'?' => {
                if tc == 0 { return false; }
                pi += 1;
                ti += 1;
            }
            b'[' => {
                if tc == 0 { return false; }
                let (matched, new_pi) = match_class(pat, pi + 1, tc, nocase);
                if !matched { return false; }
                pi = new_pi;
                ti += 1;
            }
            b'\\' => {
                pi += 1;
                let escaped = *pat.add(pi);
                if escaped == 0 { return false; }
                let (a, b) = if nocase { (to_lower(tc), to_lower(escaped)) } else { (tc, escaped) };
                if a != b { return false; }
                pi += 1;
                ti += 1;
            }
            _ => {
                let (a, b) = if nocase { (to_lower(pc), to_lower(tc)) } else { (pc, tc) };
                if a != b { return false; }
                pi += 1;
                ti += 1;
            }
        }
    }
}

fn to_lower(c: u8) -> u8 {
    if c >= b'A' && c <= b'Z' { c + 32 } else { c }
}

// match a glob pattern against a string
#[no_mangle]
pub extern "C" fn miku_glob_match(pattern: *const u8, text: *const u8) -> bool {
    if pattern.is_null() || text.is_null() {
        return pattern.is_null() && text.is_null();
    }
    unsafe { glob_core(pattern, 0, text, 0, false) }
}

// match pattern case-insensitively
#[no_mangle]
pub extern "C" fn miku_glob_match_nocase(pattern: *const u8, text: *const u8) -> bool {
    if pattern.is_null() || text.is_null() {
        return pattern.is_null() && text.is_null();
    }
    unsafe { glob_core(pattern, 0, text, 0, true) }
}

// check if a string contains any glob special characters
#[no_mangle]
pub extern "C" fn miku_glob_has_magic(pattern: *const u8) -> bool {
    if pattern.is_null() { return false; }
    unsafe {
        let mut i = 0;
        loop {
            let c = *pattern.add(i);
            if c == 0 { return false; }
            if c == b'*' || c == b'?' || c == b'[' { return true; }
            if c == b'\\' {
                i += 1;
                if *pattern.add(i) == 0 { return false; }
            }
            i += 1;
        }
    }
}

// escape special characters in string, returns heap-allocated
#[no_mangle]
pub extern "C" fn miku_glob_escape(s: *const u8) -> *mut u8 {
    if s.is_null() { return core::ptr::null_mut(); }
    let len = string::miku_strlen(s);
    let out = crate::heap::miku_malloc(len * 2 + 1);
    if out.is_null() { return core::ptr::null_mut(); }
    unsafe {
        let mut oi = 0usize;
        for i in 0..len {
            let c = *s.add(i);
            if c == b'*' || c == b'?' || c == b'\\' || c == b'[' || c == b']' {
                *out.add(oi) = b'\\';
                oi += 1;
            }
            *out.add(oi) = c;
            oi += 1;
        }
        *out.add(oi) = 0;
    }
    out
}

// filter an array of strings by glob pattern
// Returns count of matches. If out is not null, writes matching pointers
#[no_mangle]
pub extern "C" fn miku_glob_filter(
    pattern: *const u8,
    strings: *const *const u8,
    count: usize,
    out: *mut *const u8,
) -> usize {
    if pattern.is_null() || strings.is_null() || count == 0 { return 0; }
    let mut matched = 0usize;
    unsafe {
        for i in 0..count {
            let s = *strings.add(i);
            if miku_glob_match(pattern, s) {
                if !out.is_null() {
                    *out.add(matched) = s;
                }
                matched += 1;
            }
        }
    }
    matched
}
