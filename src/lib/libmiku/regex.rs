///////////////////////////////////////////////////////////////////////////
//                    Regular expression engine                          //
//                                                                       //
// Supports:                                                             //
//   .     - any character                                               //
//   *     - zero or more of previous                                    //
//   +     - one or more of previous                                     //
//   ?     - zero or one of previous                                     //
//   ^     - anchor start                                                //
//   $     - anchor end                                                  //
//   [abc] - character class                                             //
//   [^ab] - negated class                                               //
//   [a-z] - range in class                                              //
//   \d    - digit [0-9]                                                 //
//   \w    - word char [a-zA-Z0-9_]                                      //
//   \s    - whitespace                                                  //
//   \\    - literal backslash                                           //
//                                                                       //
// No heap allocation, Operates on C strings directly                    //
// Inspired by Rob Pike's regex from "Beautiful Code"                    //
///////////////////////////////////////////////////////////////////////////

const MAX_PATTERN: usize = 256;

// compiled pattern token
#[derive(Clone, Copy)]
struct Token {
    kind: u8,       // LITERAL, DOT, CLASS, NCLASS, ANCHOR_START, ANCHOR_END
    ch: u8,         // character for LITERAL
    class_start: u8, // index into class_buf
    class_len: u8,   // length in class_buf
    quant: u8,      // NONE, STAR, PLUS, QUESTION
}

const TK_LITERAL:      u8 = 1;
const TK_DOT:          u8 = 2;
const TK_CLASS:        u8 = 3;
const TK_NCLASS:       u8 = 4;
const TK_ANCHOR_START: u8 = 5;
const TK_ANCHOR_END:   u8 = 6;
const TK_DIGIT:        u8 = 7;
const TK_WORD:         u8 = 8;
const TK_SPACE:        u8 = 9;

const Q_NONE:     u8 = 0;
const Q_STAR:     u8 = 1;
const Q_PLUS:     u8 = 2;
const Q_QUESTION: u8 = 3;

struct CompiledRegex {
    tokens: [Token; MAX_PATTERN],
    ntokens: usize,
    class_buf: [u8; 256], // storage for character class members
    class_used: usize,
}

fn is_digit(c: u8) -> bool { c >= b'0' && c <= b'9' }
fn is_word(c: u8) -> bool {
    (c >= b'a' && c <= b'z') || (c >= b'A' && c <= b'Z')
    || (c >= b'0' && c <= b'9') || c == b'_'
}
fn is_space(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\n' || c == b'\r'
}

impl CompiledRegex {
    fn new() -> Self {
        const EMPTY: Token = Token { kind: 0, ch: 0, class_start: 0, class_len: 0, quant: Q_NONE };
        Self {
            tokens: [EMPTY; MAX_PATTERN],
            ntokens: 0,
            class_buf: [0u8; 256],
            class_used: 0,
        }
    }

    // compile pattern into tokens
    unsafe fn compile(&mut self, pat: *const u8) -> bool {
        let mut pi = 0usize;
        self.ntokens = 0;
        self.class_used = 0;

        loop {
            let c = *pat.add(pi);
            if c == 0 { break; }
            if self.ntokens >= MAX_PATTERN { return false; }

            let tok = &mut self.tokens[self.ntokens];
            tok.quant = Q_NONE;

            match c {
                b'^' => {
                    tok.kind = TK_ANCHOR_START;
                    pi += 1;
                }
                b'$' => {
                    tok.kind = TK_ANCHOR_END;
                    pi += 1;
                }
                b'.' => {
                    tok.kind = TK_DOT;
                    pi += 1;
                }
                b'[' => {
                    pi += 1;
                    let negated = *pat.add(pi) == b'^';
                    if negated { pi += 1; }
                    tok.kind = if negated { TK_NCLASS } else { TK_CLASS };
                    let start = self.class_used as u8;
                    tok.class_start = start;
                    // collect class members
                    while *pat.add(pi) != 0 && *pat.add(pi) != b']' {
                        let lo = *pat.add(pi);
                        if *pat.add(pi + 1) == b'-' && *pat.add(pi + 2) != b']' && *pat.add(pi + 2) != 0 {
                            // range a-z
                            let hi = *pat.add(pi + 2);
                            let mut ch = lo;
                            while ch <= hi {
                                if self.class_used < 256 {
                                    self.class_buf[self.class_used] = ch;
                                    self.class_used += 1;
                                }
                                ch += 1;
                            }
                            pi += 3;
                        } else {
                            if self.class_used < 256 {
                                self.class_buf[self.class_used] = lo;
                                self.class_used += 1;
                            }
                            pi += 1;
                        }
                    }
                    tok.class_len = (self.class_used as u8).wrapping_sub(start);
                    if *pat.add(pi) == b']' { pi += 1; }
                }
                b'\\' => {
                    pi += 1;
                    let esc = *pat.add(pi);
                    if esc == 0 { return false; }
                    match esc {
                        b'd' => tok.kind = TK_DIGIT,
                        b'w' => tok.kind = TK_WORD,
                        b's' => tok.kind = TK_SPACE,
                        _ => { tok.kind = TK_LITERAL; tok.ch = esc; }
                    }
                    pi += 1;
                }
                _ => {
                    tok.kind = TK_LITERAL;
                    tok.ch = c;
                    pi += 1;
                }
            }

            // check for quantifier
            let q = *pat.add(pi);
            match q {
                b'*' => { self.tokens[self.ntokens].quant = Q_STAR; pi += 1; }
                b'+' => { self.tokens[self.ntokens].quant = Q_PLUS; pi += 1; }
                b'?' => { self.tokens[self.ntokens].quant = Q_QUESTION; pi += 1; }
                _ => {}
            }

            self.ntokens += 1;
        }
        true
    }

    // check if a token matches a character
    fn matches_char(&self, tok: &Token, c: u8) -> bool {
        match tok.kind {
            TK_DOT => c != 0,
            TK_LITERAL => c == tok.ch,
            TK_DIGIT => is_digit(c),
            TK_WORD => is_word(c),
            TK_SPACE => is_space(c),
            TK_CLASS => {
                let start = tok.class_start as usize;
                let len = tok.class_len as usize;
                for i in 0..len {
                    if self.class_buf[start + i] == c { return true; }
                }
                false
            }
            TK_NCLASS => {
                let start = tok.class_start as usize;
                let len = tok.class_len as usize;
                for i in 0..len {
                    if self.class_buf[start + i] == c { return false; }
                }
                c != 0 // must match something
            }
            _ => false,
        }
    }

    // recursive match from token index ti at text position
    unsafe fn match_here(&self, ti: usize, txt: *const u8, mut pos: usize) -> bool {
        if ti >= self.ntokens {
            return true; // pattern consumed - match
        }
        let tok = &self.tokens[ti];

        // anchor end
        if tok.kind == TK_ANCHOR_END {
            return *txt.add(pos) == 0;
        }

        match tok.quant {
            Q_STAR => {
                // try matching zero or more
                // greedy: try as many as possible first
                let mut count = 0usize;
                while *txt.add(pos + count) != 0 && self.matches_char(tok, *txt.add(pos + count)) {
                    count += 1;
                }
                // try from longest to shortest
                while count > 0 {
                    if self.match_here(ti + 1, txt, pos + count) {
                        return true;
                    }
                    count -= 1;
                }
                // try zero matches
                self.match_here(ti + 1, txt, pos)
            }
            Q_PLUS => {
                // one or more
                if *txt.add(pos) == 0 || !self.matches_char(tok, *txt.add(pos)) {
                    return false;
                }
                pos += 1;
                let mut count = 0usize;
                while *txt.add(pos + count) != 0 && self.matches_char(tok, *txt.add(pos + count)) {
                    count += 1;
                }
                while count > 0 {
                    if self.match_here(ti + 1, txt, pos + count) {
                        return true;
                    }
                    count -= 1;
                }
                self.match_here(ti + 1, txt, pos)
            }
            Q_QUESTION => {
                // zero or one
                if *txt.add(pos) != 0 && self.matches_char(tok, *txt.add(pos)) {
                    if self.match_here(ti + 1, txt, pos + 1) {
                        return true;
                    }
                }
                self.match_here(ti + 1, txt, pos)
            }
            _ => {
                // no quantifier - must match exactly one
                let c = *txt.add(pos);
                if c == 0 { return false; }
                if !self.matches_char(tok, c) { return false; }
                self.match_here(ti + 1, txt, pos + 1)
            }
        }
    }
}

// Test if text matches pattern (anchored or floating)
// Returns true if any substring of text matches the pattern
#[no_mangle]
pub extern "C" fn miku_regex_match(pattern: *const u8, text: *const u8) -> bool {
    if pattern.is_null() || text.is_null() {
        return false;
    }
    let mut re = CompiledRegex::new();
    if !unsafe { re.compile(pattern) } {
        return false;
    }
    if re.ntokens == 0 {
        return true; // empty pattern matches everything
    }

    unsafe {
        // if pattern starts with ^, try only at position 0
        if re.tokens[0].kind == TK_ANCHOR_START {
            return re.match_here(1, text, 0);
        }
        // try matching at every position
        let mut pos = 0usize;
        loop {
            if re.match_here(0, text, pos) {
                return true;
            }
            if *text.add(pos) == 0 { break; }
            pos += 1;
        }
        false
    }
}

// Test if entire text matches pattern (full match)
#[no_mangle]
pub extern "C" fn miku_regex_match_full(pattern: *const u8, text: *const u8) -> bool {
    if pattern.is_null() || text.is_null() {
        return false;
    }
    // build ^pattern$ implicitly
    let mut re = CompiledRegex::new();
    if !unsafe { re.compile(pattern) } {
        return false;
    }
    // append implicit $ anchor so we verify entire text is consumed
    if re.ntokens < MAX_PATTERN {
        re.tokens[re.ntokens].kind = TK_ANCHOR_END;
        re.tokens[re.ntokens].quant = Q_NONE;
        re.ntokens += 1;
    }
    unsafe {
        re.match_here(0, text, 0)
    }
}

// Find first match position, returns byte offset or -1
#[no_mangle]
pub extern "C" fn miku_regex_find(pattern: *const u8, text: *const u8) -> i64 {
    if pattern.is_null() || text.is_null() {
        return -1;
    }
    let mut re = CompiledRegex::new();
    if !unsafe { re.compile(pattern) } {
        return -1;
    }
    if re.ntokens == 0 {
        return 0;
    }

    unsafe {
        if re.tokens[0].kind == TK_ANCHOR_START {
            return if re.match_here(1, text, 0) { 0 } else { -1 };
        }
        let mut pos = 0usize;
        loop {
            if re.match_here(0, text, pos) {
                return pos as i64;
            }
            if *text.add(pos) == 0 { break; }
            pos += 1;
        }
        -1
    }
}

// find greedy match length at a given start position
// uses stack buffer to avoid heap per attempt
// returns match length, or 0 if no match
unsafe fn find_match_len(pattern: *const u8, text: *const u8, start: usize, text_len: usize) -> usize {
    let mut re = CompiledRegex::new();
    if !re.compile(pattern) { return 0; }
    // append $ anchor for full-match test
    if re.ntokens < MAX_PATTERN {
        re.tokens[re.ntokens].kind = TK_ANCHOR_END;
        re.tokens[re.ntokens].quant = Q_NONE;
        re.ntokens += 1;
    }
    let max_try = (text_len - start).min(512);
    let mut buf = [0u8; 513];
    for end in (start..=start + max_try).rev() {
        let slen = end - start;
        crate::mem::miku_memcpy(buf.as_mut_ptr(), text.add(start), slen);
        buf[slen] = 0;
        if re.match_here(0, buf.as_ptr(), 0) {
            return slen;
        }
    }
    0
}

// find match and return start + length via out params
// returns true if matched
#[no_mangle]
pub extern "C" fn miku_regex_find_span(
    pattern: *const u8,
    text: *const u8,
    out_start: *mut usize,
    out_len: *mut usize,
) -> bool {
    if pattern.is_null() || text.is_null() { return false; }

    let pos = miku_regex_find(pattern, text);
    if pos < 0 { return false; }

    let start = pos as usize;
    let text_len = crate::string::miku_strlen(text);
    let mlen = unsafe { find_match_len(pattern, text, start, text_len) };

    if !out_start.is_null() { unsafe { *out_start = start; } }
    if !out_len.is_null() { unsafe { *out_len = mlen; } }
    true
}

// replace first match of pattern with replacement string
// returns heap-allocated result, caller must free
#[no_mangle]
pub extern "C" fn miku_regex_replace(
    pattern: *const u8,
    text: *const u8,
    replacement: *const u8,
) -> *mut u8 {
    if text.is_null() { return core::ptr::null_mut(); }
    if pattern.is_null() || replacement.is_null() {
        return crate::string::miku_strdup(text);
    }

    let text_len = crate::string::miku_strlen(text);
    let repl_len = crate::string::miku_strlen(replacement);

    let pos = miku_regex_find(pattern, text);
    if pos < 0 {
        return crate::string::miku_strdup(text);
    }
    let start = pos as usize;
    let match_len = unsafe { find_match_len(pattern, text, start, text_len) };
    let match_end = start + match_len;

    let result_len = text_len - match_len + repl_len;
    let result = crate::heap::miku_malloc(result_len + 1);
    if result.is_null() { return core::ptr::null_mut(); }

    unsafe {
        if start > 0 {
            crate::mem::miku_memcpy(result, text, start);
        }
        crate::mem::miku_memcpy(result.add(start), replacement, repl_len);
        let suffix_len = text_len - match_end;
        if suffix_len > 0 {
            crate::mem::miku_memcpy(result.add(start + repl_len), text.add(match_end), suffix_len);
        }
        *result.add(result_len) = 0;
    }
    result
}

// replace all matches of pattern with replacement
// returns heap-allocated result, caller must free
#[no_mangle]
pub extern "C" fn miku_regex_replace_all(
    pattern: *const u8,
    text: *const u8,
    replacement: *const u8,
) -> *mut u8 {
    if text.is_null() { return core::ptr::null_mut(); }
    if pattern.is_null() || replacement.is_null() {
        return crate::string::miku_strdup(text);
    }

    let text_len = crate::string::miku_strlen(text);
    let repl_len = crate::string::miku_strlen(replacement);

    let max_result = text_len.saturating_mul(repl_len + 1).saturating_add(repl_len + 1);
    let mut cap = if max_result > 4096 { 4096 } else { max_result };
    let mut result = crate::heap::miku_malloc(cap);
    if result.is_null() { return core::ptr::null_mut(); }
    let mut rlen = 0usize;

    let mut re = CompiledRegex::new();
    if !unsafe { re.compile(pattern) } {
        return crate::string::miku_strdup(text);
    }

    let mut pos = 0usize;
    unsafe {
        while pos < text_len {
            if re.match_here(0, text, pos) {
                let mlen = find_match_len(pattern, text, pos, text_len);

                // grow if needed: replacement + worst-case tail + null + 1 char
                // (the +1 covers the literal char we copy when mlen == 0)
                let tail = text_len - pos - mlen.min(text_len - pos);
                let need = rlen + repl_len + tail + 2;
                if need > cap {
                    let new_cap = if need > cap * 2 { need } else { cap * 2 };
                    let new_buf = crate::heap::miku_realloc(result, new_cap);
                    if new_buf.is_null() { break; }
                    result = new_buf;
                    cap = new_cap;
                }

                crate::mem::miku_memcpy(result.add(rlen), replacement, repl_len);
                rlen += repl_len;
                if mlen == 0 {
                    if pos < text_len {
                        *result.add(rlen) = *text.add(pos);
                        rlen += 1;
                        pos += 1;
                    } else {
                        break;
                    }
                } else {
                    pos += mlen;
                }
            } else {
                if rlen + 2 > cap {
                    let new_cap = cap * 2;
                    let new_buf = crate::heap::miku_realloc(result, new_cap);
                    if new_buf.is_null() { break; }
                    result = new_buf;
                    cap = new_cap;
                }
                *result.add(rlen) = *text.add(pos);
                rlen += 1;
                pos += 1;
            }
        }
        *result.add(rlen) = 0;
    }
    result
}

// split string by regex pattern
// writes pointers+lengths into out arrays, returns count
// each segment is a pointer into the original text (not copied)
#[no_mangle]
pub extern "C" fn miku_regex_split(
    pattern: *const u8,
    text: *const u8,
    out_starts: *mut *const u8,
    out_lens: *mut usize,
    max_parts: usize,
) -> usize {
    if pattern.is_null() || text.is_null() || out_starts.is_null() || out_lens.is_null() || max_parts == 0 {
        return 0;
    }

    let text_len = crate::string::miku_strlen(text);
    let mut re = CompiledRegex::new();
    if !unsafe { re.compile(pattern) } { return 0; }

    let mut count = 0usize;
    let mut seg_start = 0usize;
    let mut pos = 0usize;

    unsafe {
        while pos < text_len && count < max_parts - 1 {
            if re.match_here(0, text, pos) {
                let mlen = find_match_len(pattern, text, pos, text_len);
                let mlen = if mlen > 0 { mlen } else { 1 };

                *out_starts.add(count) = text.add(seg_start);
                *out_lens.add(count) = pos - seg_start;
                count += 1;
                pos += mlen;
                seg_start = pos;
            } else {
                pos += 1;
            }
        }

        // last segment
        if count < max_parts {
            *out_starts.add(count) = text.add(seg_start);
            *out_lens.add(count) = text_len - seg_start;
            count += 1;
        }
    }
    count
}

// find all non-overlapping matches, returns count
// writes match start offsets and lengths into out arrays
#[no_mangle]
pub extern "C" fn miku_regex_find_all(
    pattern: *const u8,
    text: *const u8,
    out_starts: *mut usize,
    out_lens: *mut usize,
    max_matches: usize,
) -> usize {
    if pattern.is_null() || text.is_null() || out_starts.is_null() || out_lens.is_null() || max_matches == 0 {
        return 0;
    }

    let text_len = crate::string::miku_strlen(text);
    let mut re = CompiledRegex::new();
    if !unsafe { re.compile(pattern) } { return 0; }

    let mut count = 0usize;
    let mut pos = 0usize;

    unsafe {
        while pos < text_len && count < max_matches {
            if re.match_here(0, text, pos) {
                let mlen = find_match_len(pattern, text, pos, text_len);
                let mlen = if mlen > 0 { mlen } else { 1 };

                *out_starts.add(count) = pos;
                *out_lens.add(count) = mlen;
                count += 1;
                pos += mlen;
            } else {
                pos += 1;
            }
        }
    }
    count
}

// count non-overlapping matches
#[no_mangle]
pub extern "C" fn miku_regex_count(pattern: *const u8, text: *const u8) -> usize {
    if pattern.is_null() || text.is_null() {
        return 0;
    }
    let mut re = CompiledRegex::new();
    if !unsafe { re.compile(pattern) } {
        return 0;
    }
    if re.ntokens == 0 {
        return 0;
    }

    unsafe {
        let mut count = 0usize;
        let mut pos = 0usize;
        while *text.add(pos) != 0 {
            if re.match_here(0, text, pos) {
                count += 1;
                // advance past match (at least 1 char to avoid infinite loop)
                pos += 1;
            } else {
                pos += 1;
            }
        }
        count
    }
}
