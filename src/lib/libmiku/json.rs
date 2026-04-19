// JSON parser :D
//
// Read-only parser for JSON data. No heap allocation for parsing 
// works by returning token positions within the input buffer
// Supports: objects, arrays, strings, numbers, booleans, null
//
// Use miku_json_parse() to tokenize, then query tokens by index
// Token types: object, array, string, number, bool, null
// Objects and arrays store child count in token.size

use crate::string;
use crate::mem;

// token types
pub const JSON_OBJECT: u8 = 1;
pub const JSON_ARRAY: u8 = 2;
pub const JSON_STRING: u8 = 3;
pub const JSON_NUMBER: u8 = 4;
pub const JSON_BOOL: u8 = 5;
pub const JSON_NULL: u8 = 6;

const MAX_DEPTH: usize = 32;

// a single JSON token
#[repr(C)]
#[derive(Copy, Clone)]
pub struct MikuJsonToken {
    pub ttype: u8,        // token type (JSON_*)
    pub start: u32,       // start offset in input
    pub end: u32,         // end offset (exclusive)
    pub size: u32,        // number of child tokens (for object/array)
    pub parent: i32,      // parent token index, -1 for root
}

// parser state
#[repr(C)]
pub struct MikuJsonParser {
    pos: usize,
    next_tok: usize,
    super_stack: [i32; MAX_DEPTH],
    depth: usize,
}

const EMPTY_TOKEN: MikuJsonToken = MikuJsonToken {
    ttype: 0,
    start: 0,
    end: 0,
    size: 0,
    parent: -1,
};

// initialize parser
#[no_mangle]
pub extern "C" fn miku_json_init(p: *mut MikuJsonParser) {
    if p.is_null() { return; }
    unsafe {
        (*p).pos = 0;
        (*p).next_tok = 0;
        (*p).depth = 0;
        for i in 0..MAX_DEPTH {
            (*p).super_stack[i] = -1;
        }
    }
}

fn is_ws(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\n' || c == b'\r'
}

unsafe fn skip_ws(data: *const u8, len: usize, pos: &mut usize) {
    while *pos < len && is_ws(*data.add(*pos)) {
        *pos += 1;
    }
}

unsafe fn alloc_token(
    p: *mut MikuJsonParser,
    tokens: *mut MikuJsonToken,
    max_tokens: usize,
) -> i32 {
    let idx = (*p).next_tok;
    if idx >= max_tokens {
        return -1;
    }
    (*p).next_tok += 1;
    let t = &mut *tokens.add(idx);
    t.ttype = 0;
    t.start = 0;
    t.end = 0;
    t.size = 0;
    t.parent = -1;
    idx as i32
}

unsafe fn parse_string(
    data: *const u8,
    len: usize,
    p: *mut MikuJsonParser,
    tokens: *mut MikuJsonToken,
    max_tokens: usize,
) -> i32 {
    let start = (*p).pos;
    (*p).pos += 1; // skip opening quote

    while (*p).pos < len {
        let c = *data.add((*p).pos);
        if c == b'\\' {
            let esc = if (*p).pos + 1 < len { *data.add((*p).pos + 1) } else { 0 };
            if esc == b'u' && (*p).pos + 5 < len {
                (*p).pos += 6; // \uXXXX
            } else {
                (*p).pos += 2; // all other single-char escapes
            }
            continue;
        }
        if c == b'"' {
            let ti = alloc_token(p, tokens, max_tokens);
            if ti < 0 { return -1; }
            let t = &mut *tokens.add(ti as usize);
            t.ttype = JSON_STRING;
            t.start = (start + 1) as u32; // skip quote
            t.end = (*p).pos as u32;
            if (*p).depth > 0 {
                let parent = (*p).super_stack[(*p).depth - 1];
                t.parent = parent;
                if parent >= 0 {
                    (*tokens.add(parent as usize)).size += 1;
                }
            }
            (*p).pos += 1;
            return ti;
        }
        (*p).pos += 1;
    }
    -1 // unterminated string
}

unsafe fn parse_primitive(
    data: *const u8,
    len: usize,
    p: *mut MikuJsonParser,
    tokens: *mut MikuJsonToken,
    max_tokens: usize,
) -> i32 {
    let start = (*p).pos;

    while (*p).pos < len {
        let c = *data.add((*p).pos);
        if c == b',' || c == b'}' || c == b']' || is_ws(c) {
            break;
        }
        (*p).pos += 1;
    }

    let ti = alloc_token(p, tokens, max_tokens);
    if ti < 0 { return -1; }
    let t = &mut *tokens.add(ti as usize);
    t.start = start as u32;
    t.end = (*p).pos as u32;

    // determine type
    let first = *data.add(start);
    if first == b't' || first == b'f' {
        t.ttype = JSON_BOOL;
    } else if first == b'n' {
        t.ttype = JSON_NULL;
    } else {
        t.ttype = JSON_NUMBER;
    }

    if (*p).depth > 0 {
        let parent = (*p).super_stack[(*p).depth - 1];
        t.parent = parent;
        if parent >= 0 {
            (*tokens.add(parent as usize)).size += 1;
        }
    }
    ti
}

// parse JSON string
// Returns number of tokens parsed, or -1 on error
// tokens: caller-provided array of MikuJsonToken
#[no_mangle]
pub extern "C" fn miku_json_parse(
    p: *mut MikuJsonParser,
    data: *const u8,
    len: usize,
    tokens: *mut MikuJsonToken,
    max_tokens: usize,
) -> i32 {
    if p.is_null() || data.is_null() || tokens.is_null() || max_tokens == 0 {
        return -1;
    }

    unsafe {
        while (*p).pos < len {
            skip_ws(data, len, &mut (*p).pos);
            if (*p).pos >= len { break; }

            let c = *data.add((*p).pos);

            match c {
                b'{' | b'[' => {
                    let ti = alloc_token(p, tokens, max_tokens);
                    if ti < 0 { return -1; }
                    let t = &mut *tokens.add(ti as usize);
                    t.ttype = if c == b'{' { JSON_OBJECT } else { JSON_ARRAY };
                    t.start = (*p).pos as u32;

                    if (*p).depth > 0 {
                        let parent = (*p).super_stack[(*p).depth - 1];
                        t.parent = parent;
                        if parent >= 0 {
                            (*tokens.add(parent as usize)).size += 1;
                        }
                    }

                    if (*p).depth >= MAX_DEPTH { return -1; }
                    (*p).super_stack[(*p).depth] = ti;
                    (*p).depth += 1;
                    (*p).pos += 1;
                }
                b'}' | b']' => {
                    if (*p).depth == 0 { return -1; }
                    (*p).depth -= 1;
                    let ti = (*p).super_stack[(*p).depth] as usize;
                    (*tokens.add(ti)).end = ((*p).pos + 1) as u32;
                    (*p).pos += 1;
                }
                b'"' => {
                    if parse_string(data, len, p, tokens, max_tokens) < 0 {
                        return -1;
                    }
                }
                b':' | b',' => {
                    (*p).pos += 1;
                }
                _ => {
                    if parse_primitive(data, len, p, tokens, max_tokens) < 0 {
                        return -1;
                    }
                }
            }
        }

        (*p).next_tok as i32
    }
}

// get token type
#[no_mangle]
pub extern "C" fn miku_json_type(
    tokens: *const MikuJsonToken,
    index: usize,
) -> u8 {
    if tokens.is_null() { return 0; }
    unsafe { (*tokens.add(index)).ttype }
}

// get token string value (start pointer and length)
// For strings, start points past the opening quote
#[no_mangle]
pub extern "C" fn miku_json_value(
    data: *const u8,
    tokens: *const MikuJsonToken,
    index: usize,
    out_len: *mut usize,
) -> *const u8 {
    if data.is_null() || tokens.is_null() {
        return core::ptr::null();
    }
    unsafe {
        let t = &*tokens.add(index);
        if !out_len.is_null() {
            *out_len = (t.end - t.start) as usize;
        }
        data.add(t.start as usize)
    }
}

// compare token string with a C string
#[no_mangle]
pub extern "C" fn miku_json_eq(
    data: *const u8,
    tokens: *const MikuJsonToken,
    index: usize,
    s: *const u8,
) -> bool {
    if data.is_null() || tokens.is_null() || s.is_null() {
        return false;
    }
    unsafe {
        let t = &*tokens.add(index);
        let tlen = (t.end - t.start) as usize;
        let slen = string::miku_strlen(s);
        if tlen != slen { return false; }
        string::miku_strncmp(data.add(t.start as usize), s, tlen) == 0
    }
}

// get child count of object/array token
#[no_mangle]
pub extern "C" fn miku_json_size(
    tokens: *const MikuJsonToken,
    index: usize,
) -> u32 {
    if tokens.is_null() { return 0; }
    unsafe { (*tokens.add(index)).size }
}

// find key in object, returns token index of value or -1
// Searches direct children of object at obj_index
#[no_mangle]
pub extern "C" fn miku_json_find(
    data: *const u8,
    tokens: *const MikuJsonToken,
    num_tokens: usize,
    obj_index: usize,
    key: *const u8,
) -> i32 {
    if data.is_null() || tokens.is_null() || key.is_null() {
        return -1;
    }
    unsafe {
        let obj = &*tokens.add(obj_index);
        if obj.ttype != JSON_OBJECT { return -1; }

        let pairs = obj.size as usize;
        let mut ti = obj_index + 1;
        let mut found = 0usize;

        while found < pairs && ti < num_tokens {
            // key token
            let kt = &*tokens.add(ti);
            if kt.parent == obj_index as i32 && kt.ttype == JSON_STRING {
                if miku_json_eq(data, tokens, ti, key) {
                    // value is next token
                    if ti + 1 < num_tokens {
                        return (ti + 1) as i32;
                    }
                }
                found += 1;
                // skip key + value (value might be compound)
                ti = skip_token(tokens, num_tokens, ti + 1);
            } else {
                ti += 1;
            }
        }
    }
    -1
}

// skip over a token and all its children
unsafe fn skip_token(
    tokens: *const MikuJsonToken,
    num_tokens: usize,
    index: usize,
) -> usize {
    if index >= num_tokens { return num_tokens; }
    let t = &*tokens.add(index);
    match t.ttype {
        JSON_OBJECT => {
            let mut i = index + 1;
            let mut pairs = 0u32;
            while pairs < t.size && i < num_tokens {
                i = skip_token(tokens, num_tokens, i); // key
                i = skip_token(tokens, num_tokens, i); // value
                pairs += 1;
            }
            i
        }
        JSON_ARRAY => {
            let mut i = index + 1;
            let mut elems = 0u32;
            while elems < t.size && i < num_tokens {
                i = skip_token(tokens, num_tokens, i);
                elems += 1;
            }
            i
        }
        _ => index + 1,
    }
}

// get number of tokens used
#[no_mangle]
pub extern "C" fn miku_json_token_count(p: *const MikuJsonParser) -> usize {
    if p.is_null() { return 0; }
    unsafe { (*p).next_tok }
}

// get array element by index, returns token index or -1
#[no_mangle]
pub extern "C" fn miku_json_array_get(
    tokens: *const MikuJsonToken,
    num_tokens: usize,
    arr_index: usize,
    elem_index: usize,
) -> i32 {
    if tokens.is_null() { return -1; }
    unsafe {
        let arr = &*tokens.add(arr_index);
        if arr.ttype != JSON_ARRAY { return -1; }
        if elem_index >= arr.size as usize { return -1; }

        let mut ti = arr_index + 1;
        let mut count = 0usize;
        while count < arr.size as usize && ti < num_tokens {
            if count == elem_index {
                return ti as i32;
            }
            ti = skip_token(tokens, num_tokens, ti);
            count += 1;
        }
    }
    -1
}

// extract integer value from a number token
#[no_mangle]
pub extern "C" fn miku_json_int(
    data: *const u8,
    tokens: *const MikuJsonToken,
    index: usize,
) -> i64 {
    if data.is_null() || tokens.is_null() { return 0; }
    unsafe {
        let t = &*tokens.add(index);
        if t.ttype != JSON_NUMBER { return 0; }
        let start = t.start as usize;
        let len = (t.end - t.start) as usize;
        // manual atoi for the token range
        let mut neg = false;
        let mut i = 0usize;
        if i < len && *data.add(start + i) == b'-' {
            neg = true;
            i += 1;
        }
        let mut val: i64 = 0;
        while i < len {
            let c = *data.add(start + i);
            if c < b'0' || c > b'9' { break; } // stop at '.' for floats
            val = val.wrapping_mul(10).wrapping_add((c - b'0') as i64);
            i += 1;
        }
        if neg { val.wrapping_neg() } else { val }
    }
}

// extract boolean value from a bool token
#[no_mangle]
pub extern "C" fn miku_json_bool(
    data: *const u8,
    tokens: *const MikuJsonToken,
    index: usize,
) -> i32 {
    if data.is_null() || tokens.is_null() { return -1; }
    unsafe {
        let t = &*tokens.add(index);
        if t.ttype != JSON_BOOL { return -1; }
        if *data.add(t.start as usize) == b't' { 1 } else { 0 }
    }
}

// check if token is null type
#[no_mangle]
pub extern "C" fn miku_json_is_null(
    tokens: *const MikuJsonToken,
    index: usize,
) -> bool {
    if tokens.is_null() { return false; }
    unsafe { (*tokens.add(index)).ttype == JSON_NULL }
}

// copy token string value to buffer (null-terminated)
// Returns number of bytes written (excluding null), or -1 on error
#[no_mangle]
pub extern "C" fn miku_json_strcpy(
    data: *const u8,
    tokens: *const MikuJsonToken,
    index: usize,
    buf: *mut u8,
    buf_size: usize,
) -> i32 {
    if data.is_null() || tokens.is_null() || buf.is_null() || buf_size == 0 { return -1; }
    unsafe {
        let t = &*tokens.add(index);
        let len = (t.end - t.start) as usize;
        let copy_len = if len < buf_size - 1 { len } else { buf_size - 1 };
        mem::miku_memcpy(buf, data.add(t.start as usize), copy_len);
        *buf.add(copy_len) = 0;
        copy_len as i32
    }
}

// unsigned integer extraction

#[no_mangle]
pub extern "C" fn miku_json_u64(
    data: *const u8,
    tokens: *const MikuJsonToken,
    index: usize,
) -> u64 {
    miku_json_int(data, tokens, index) as u64
}

//   string unescape: handle \n, \t, \r, \\, \/, \", \uXXXX
// Returns number of bytes written, or -1 on error
#[no_mangle]
pub extern "C" fn miku_json_unescape(
    data: *const u8,
    tokens: *const MikuJsonToken,
    index: usize,
    buf: *mut u8,
    buf_size: usize,
) -> i32 {
    if data.is_null() || tokens.is_null() || buf.is_null() || buf_size == 0 { return -1; }
    unsafe {
        let t = &*tokens.add(index);
        let src = data.add(t.start as usize);
        let slen = (t.end - t.start) as usize;
        let mut si = 0usize;
        let mut di = 0usize;
        let limit = buf_size - 1;

        while si < slen && di < limit {
            if *src.add(si) == b'\\' && si + 1 < slen {
                si += 1;
                let c = *src.add(si);
                match c {
                    b'n'  => { *buf.add(di) = b'\n'; }
                    b't'  => { *buf.add(di) = b'\t'; }
                    b'r'  => { *buf.add(di) = b'\r'; }
                    b'\\' => { *buf.add(di) = b'\\'; }
                    b'/'  => { *buf.add(di) = b'/'; }
                    b'"'  => { *buf.add(di) = b'"'; }
                    b'b'  => { *buf.add(di) = 0x08; }
                    b'f'  => { *buf.add(di) = 0x0C; }
                    b'u'  => {
                        if si + 4 < slen {
                            let cp = parse_hex4(src.add(si + 1));
                            si += 4;
                            // encode into scratch first so we can range-check
                            let mut tmp = [0u8; 4];
                            let n = crate::utf8::miku_utf8_encode(cp, tmp.as_mut_ptr());
                            if n == 0 {
                                *buf.add(di) = b'?';
                            } else if di + n <= limit + 1 {
                                // limit is buf_size-1, so we can use positions 0..=limit
                                // (limit+1 bytes) before the null terminator at [di..=limit]
                                for k in 0..n {
                                    *buf.add(di + k) = tmp[k];
                                }
                                di += n - 1; // -1 because di += 1 below
                            } else {
                                // not enough room: stop before overflowing
                                break;
                            }
                        } else {
                            *buf.add(di) = b'?';
                        }
                    }
                    _ => { *buf.add(di) = c; }
                }
                si += 1;
                di += 1;
            } else {
                *buf.add(di) = *src.add(si);
                si += 1;
                di += 1;
            }
        }
        *buf.add(di) = 0;
        di as i32
    }
}

fn parse_hex4(p: *const u8) -> u32 {
    let mut val = 0u32;
    for i in 0..4 {
        let c = unsafe { *p.add(i) };
        let d = if c >= b'0' && c <= b'9' { (c - b'0') as u32 }
            else if c >= b'a' && c <= b'f' { (c - b'a' + 10) as u32 }
            else if c >= b'A' && c <= b'F' { (c - b'A' + 10) as u32 }
            else { return val; };
        val = (val << 4) | d;
    }
    val
}

//  path-based query: "key1.key2[0].key3"
// Returns token index or -1

#[no_mangle]
pub extern "C" fn miku_json_get_path(
    data: *const u8,
    tokens: *const MikuJsonToken,
    num_tokens: usize,
    root: usize,
    path: *const u8,
) -> i32 {
    if data.is_null() || tokens.is_null() || path.is_null() { return -1; }

    let plen = string::miku_strlen(path);
    if plen == 0 { return root as i32; }

    let mut current = root;
    let mut pi = 0usize;

    unsafe {
        while pi < plen {
            let tok = &*tokens.add(current);

            if *path.add(pi) == b'[' {
                // array index: [N]
                if tok.ttype != JSON_ARRAY { return -1; }
                pi += 1;
                let mut idx = 0usize;
                while pi < plen && *path.add(pi) >= b'0' && *path.add(pi) <= b'9' {
                    idx = idx * 10 + (*path.add(pi) - b'0') as usize;
                    pi += 1;
                }
                if pi < plen && *path.add(pi) == b']' { pi += 1; }
                if pi < plen && *path.add(pi) == b'.' { pi += 1; }

                let ti = miku_json_array_get(tokens, num_tokens, current, idx);
                if ti < 0 { return -1; }
                current = ti as usize;
            } else {
                // object key: key or key.next
                if tok.ttype != JSON_OBJECT { return -1; }
                let key_start = pi;
                while pi < plen && *path.add(pi) != b'.' && *path.add(pi) != b'[' {
                    pi += 1;
                }
                let key_len = pi - key_start;
                if pi < plen && *path.add(pi) == b'.' { pi += 1; }

                // find key in object
                let mut ti = current + 1;
                let mut found = false;
                let pairs = tok.size as usize;
                let mut count = 0usize;
                while count < pairs && ti < num_tokens {
                    let kt = &*tokens.add(ti);
                    if kt.parent == current as i32 && kt.ttype == JSON_STRING {
                        let klen = (kt.end - kt.start) as usize;
                        if klen == key_len
                            && string::miku_strncmp(
                                data.add(kt.start as usize),
                                path.add(key_start),
                                key_len,
                            ) == 0
                        {
                            if ti + 1 < num_tokens {
                                current = ti + 1;
                                found = true;
                                break;
                            }
                        }
                        count += 1;
                        ti = skip_token(tokens, num_tokens, ti + 1);
                    } else {
                        ti += 1;
                    }
                }
                if !found { return -1; }
            }
        }
    }
    current as i32
}

// iteration helpers //

// iterate over object key-value pairs
// Callback receives (key_index, value_index, user_data) for each pair
// Returns number of pairs iterated
#[no_mangle]
pub extern "C" fn miku_json_object_iter(
    tokens: *const MikuJsonToken,
    num_tokens: usize,
    obj_index: usize,
    cb: unsafe extern "C" fn(usize, usize, *mut u8),
    user: *mut u8,
) -> i32 {
    if tokens.is_null() { return -1; }
    unsafe {
        let obj = &*tokens.add(obj_index);
        if obj.ttype != JSON_OBJECT { return -1; }

        let pairs = obj.size as usize;
        let mut ti = obj_index + 1;
        let mut count = 0usize;

        while count < pairs && ti < num_tokens {
            let kt = &*tokens.add(ti);
            if kt.parent == obj_index as i32 && kt.ttype == JSON_STRING {
                let key_idx = ti;
                let val_idx = ti + 1;
                if val_idx < num_tokens {
                    cb(key_idx, val_idx, user);
                }
                count += 1;
                ti = skip_token(tokens, num_tokens, ti + 1);
            } else {
                ti += 1;
            }
        }
        count as i32
    }
}

// iterate over array elements
// Callback receives (element_index, position, user_data) for each element
// Returns number of elements iterated
#[no_mangle]
pub extern "C" fn miku_json_array_iter(
    tokens: *const MikuJsonToken,
    num_tokens: usize,
    arr_index: usize,
    cb: unsafe extern "C" fn(usize, usize, *mut u8),
    user: *mut u8,
) -> i32 {
    if tokens.is_null() { return -1; }
    unsafe {
        let arr = &*tokens.add(arr_index);
        if arr.ttype != JSON_ARRAY { return -1; }

        let elems = arr.size as usize;
        let mut ti = arr_index + 1;
        let mut count = 0usize;

        while count < elems && ti < num_tokens {
            cb(ti, count, user);
            count += 1;
            ti = skip_token(tokens, num_tokens, ti);
        }
        count as i32
    }
}

// JSON writer

const JSON_WRITE_MAX_DEPTH: usize = 32;

#[repr(C)]
pub struct MikuJsonWriter {
    buf: *mut u8,
    cap: usize,
    len: usize,
    depth: usize,
    container_stack: [u8; JSON_WRITE_MAX_DEPTH], // '{' or '['
    needs_comma: [bool; JSON_WRITE_MAX_DEPTH],
    error: bool,
}

#[no_mangle]
pub extern "C" fn miku_json_writer_init(w: *mut MikuJsonWriter, buf: *mut u8, cap: usize) {
    if w.is_null() { return; }
    unsafe {
        (*w).buf = buf;
        (*w).cap = cap;
        (*w).len = 0;
        (*w).depth = 0;
        (*w).error = false;
        for i in 0..JSON_WRITE_MAX_DEPTH {
            (*w).container_stack[i] = 0;
            (*w).needs_comma[i] = false;
        }
    }
}

unsafe fn w_byte(w: *mut MikuJsonWriter, b: u8) {
    if (*w).error { return; }
    if (*w).len >= (*w).cap { (*w).error = true; return; }
    if !(*w).buf.is_null() {
        *(*w).buf.add((*w).len) = b;
    }
    (*w).len += 1;
}

unsafe fn w_bytes(w: *mut MikuJsonWriter, data: *const u8, len: usize) {
    for i in 0..len {
        w_byte(w, *data.add(i));
    }
}

unsafe fn w_comma(w: *mut MikuJsonWriter) {
    if (*w).depth > 0 && (*w).needs_comma[(*w).depth - 1] {
        w_byte(w, b',');
    }
    if (*w).depth > 0 {
        (*w).needs_comma[(*w).depth - 1] = true;
    }
}

// begin object
#[no_mangle]
pub extern "C" fn miku_json_write_object_begin(w: *mut MikuJsonWriter) {
    if w.is_null() { return; }
    unsafe {
        w_comma(w);
        w_byte(w, b'{');
        if (*w).depth < JSON_WRITE_MAX_DEPTH {
            (*w).container_stack[(*w).depth] = b'{';
            (*w).needs_comma[(*w).depth] = false;
            (*w).depth += 1;
        }
    }
}

// end object
#[no_mangle]
pub extern "C" fn miku_json_write_object_end(w: *mut MikuJsonWriter) {
    if w.is_null() { return; }
    unsafe {
        if (*w).depth > 0 { (*w).depth -= 1; }
        w_byte(w, b'}');
    }
}

// begin array
#[no_mangle]
pub extern "C" fn miku_json_write_array_begin(w: *mut MikuJsonWriter) {
    if w.is_null() { return; }
    unsafe {
        w_comma(w);
        w_byte(w, b'[');
        if (*w).depth < JSON_WRITE_MAX_DEPTH {
            (*w).container_stack[(*w).depth] = b'[';
            (*w).needs_comma[(*w).depth] = false;
            (*w).depth += 1;
        }
    }
}

// end array
#[no_mangle]
pub extern "C" fn miku_json_write_array_end(w: *mut MikuJsonWriter) {
    if w.is_null() { return; }
    unsafe {
        if (*w).depth > 0 { (*w).depth -= 1; }
        w_byte(w, b']');
    }
}

// write object key (automatically adds quotes and colon)
#[no_mangle]
pub extern "C" fn miku_json_write_key(w: *mut MikuJsonWriter, key: *const u8) {
    if w.is_null() || key.is_null() { return; }
    unsafe {
        w_comma(w);
        // reset comma flag because value follows
        if (*w).depth > 0 { (*w).needs_comma[(*w).depth - 1] = false; }
        w_byte(w, b'"');
        w_string_escaped(w, key, string::miku_strlen(key));
        w_byte(w, b'"');
        w_byte(w, b':');
    }
}

unsafe fn w_string_escaped(w: *mut MikuJsonWriter, s: *const u8, len: usize) {
    for i in 0..len {
        let c = *s.add(i);
        match c {
            b'"'  => { w_byte(w, b'\\'); w_byte(w, b'"'); }
            b'\\' => { w_byte(w, b'\\'); w_byte(w, b'\\'); }
            b'\n' => { w_byte(w, b'\\'); w_byte(w, b'n'); }
            b'\r' => { w_byte(w, b'\\'); w_byte(w, b'r'); }
            b'\t' => { w_byte(w, b'\\'); w_byte(w, b't'); }
            0x08  => { w_byte(w, b'\\'); w_byte(w, b'b'); }
            0x0C  => { w_byte(w, b'\\'); w_byte(w, b'f'); }
            _ if c < 0x20 => {
                // \u00XX
                w_byte(w, b'\\'); w_byte(w, b'u');
                w_byte(w, b'0'); w_byte(w, b'0');
                w_byte(w, hex_digit(c >> 4));
                w_byte(w, hex_digit(c & 0xF));
            }
            _ => { w_byte(w, c); }
        }
    }
}

fn hex_digit(v: u8) -> u8 {
    if v < 10 { b'0' + v } else { b'a' + v - 10 }
}

// write string value
#[no_mangle]
pub extern "C" fn miku_json_write_str(w: *mut MikuJsonWriter, val: *const u8) {
    if w.is_null() || val.is_null() { return; }
    unsafe {
        w_comma(w);
        w_byte(w, b'"');
        w_string_escaped(w, val, string::miku_strlen(val));
        w_byte(w, b'"');
    }
}

// write string value with explicit length (not null-terminated)
#[no_mangle]
pub extern "C" fn miku_json_write_strn(w: *mut MikuJsonWriter, val: *const u8, len: usize) {
    if w.is_null() || val.is_null() { return; }
    unsafe {
        w_comma(w);
        w_byte(w, b'"');
        w_string_escaped(w, val, len);
        w_byte(w, b'"');
    }
}

// write integer value
#[no_mangle]
pub extern "C" fn miku_json_write_int(w: *mut MikuJsonWriter, val: i64) {
    if w.is_null() { return; }
    unsafe {
        w_comma(w);
        let mut buf = [0u8; 24];
        crate::num::miku_itoa(val, buf.as_mut_ptr());
        let len = string::miku_strlen(buf.as_ptr());
        w_bytes(w, buf.as_ptr(), len);
    }
}

// write unsigned integer value
#[no_mangle]
pub extern "C" fn miku_json_write_u64(w: *mut MikuJsonWriter, val: u64) {
    if w.is_null() { return; }
    unsafe {
        w_comma(w);
        let mut buf = [0u8; 24];
        crate::num::miku_utoa(val, buf.as_mut_ptr());
        let len = string::miku_strlen(buf.as_ptr());
        w_bytes(w, buf.as_ptr(), len);
    }
}

// write boolean value
#[no_mangle]
pub extern "C" fn miku_json_write_bool(w: *mut MikuJsonWriter, val: bool) {
    if w.is_null() { return; }
    unsafe {
        w_comma(w);
        if val {
            w_bytes(w, b"true".as_ptr(), 4);
        } else {
            w_bytes(w, b"false".as_ptr(), 5);
        }
    }
}

// write null value
#[no_mangle]
pub extern "C" fn miku_json_write_null(w: *mut MikuJsonWriter) {
    if w.is_null() { return; }
    unsafe {
        w_comma(w);
        w_bytes(w, b"null".as_ptr(), 4);
    }
}

// write raw JSON (already-formatted string, no escaping)
#[no_mangle]
pub extern "C" fn miku_json_write_raw(w: *mut MikuJsonWriter, raw: *const u8, len: usize) {
    if w.is_null() || raw.is_null() { return; }
    unsafe {
        w_comma(w);
        w_bytes(w, raw, len);
    }
}

// finish writing: null-terminate and return length
// Returns total bytes written (excluding null), or -1 on error
#[no_mangle]
pub extern "C" fn miku_json_write_finish(w: *mut MikuJsonWriter) -> i32 {
    if w.is_null() { return -1; }
    unsafe {
        if (*w).error { return -1; }
        if (*w).len < (*w).cap && !(*w).buf.is_null() {
            *(*w).buf.add((*w).len) = 0;
        }
        (*w).len as i32
    }
}

// check if writer had an error (buffer overflow)
#[no_mangle]
pub extern "C" fn miku_json_write_error(w: *const MikuJsonWriter) -> bool {
    if w.is_null() { return true; }
    unsafe { (*w).error }
}

// convenience: write key + value in one call

#[no_mangle]
pub extern "C" fn miku_json_write_kv_str(w: *mut MikuJsonWriter, key: *const u8, val: *const u8) {
    miku_json_write_key(w, key);
    miku_json_write_str(w, val);
}

#[no_mangle]
pub extern "C" fn miku_json_write_kv_int(w: *mut MikuJsonWriter, key: *const u8, val: i64) {
    miku_json_write_key(w, key);
    miku_json_write_int(w, val);
}

#[no_mangle]
pub extern "C" fn miku_json_write_kv_bool(w: *mut MikuJsonWriter, key: *const u8, val: bool) {
    miku_json_write_key(w, key);
    miku_json_write_bool(w, val);
}

#[no_mangle]
pub extern "C" fn miku_json_write_kv_null(w: *mut MikuJsonWriter, key: *const u8) {
    miku_json_write_key(w, key);
    miku_json_write_null(w);
}

// token parent access

#[no_mangle]
pub extern "C" fn miku_json_parent(
    tokens: *const MikuJsonToken,
    index: usize,
) -> i32 {
    if tokens.is_null() { return -1; }
    unsafe { (*tokens.add(index)).parent }
}

// check if token is string type

#[no_mangle]
pub extern "C" fn miku_json_is_string(tokens: *const MikuJsonToken, index: usize) -> bool {
    if tokens.is_null() { return false; }
    unsafe { (*tokens.add(index)).ttype == JSON_STRING }
}

#[no_mangle]
pub extern "C" fn miku_json_is_number(tokens: *const MikuJsonToken, index: usize) -> bool {
    if tokens.is_null() { return false; }
    unsafe { (*tokens.add(index)).ttype == JSON_NUMBER }
}

#[no_mangle]
pub extern "C" fn miku_json_is_bool(tokens: *const MikuJsonToken, index: usize) -> bool {
    if tokens.is_null() { return false; }
    unsafe { (*tokens.add(index)).ttype == JSON_BOOL }
}

#[no_mangle]
pub extern "C" fn miku_json_is_object(tokens: *const MikuJsonToken, index: usize) -> bool {
    if tokens.is_null() { return false; }
    unsafe { (*tokens.add(index)).ttype == JSON_OBJECT }
}

#[no_mangle]
pub extern "C" fn miku_json_is_array(tokens: *const MikuJsonToken, index: usize) -> bool {
    if tokens.is_null() { return false; }
    unsafe { (*tokens.add(index)).ttype == JSON_ARRAY }
}

//   heap-allocated string extraction
// Caller must free returned pointer with miku_free

#[no_mangle]
pub extern "C" fn miku_json_strdup(
    data: *const u8,
    tokens: *const MikuJsonToken,
    index: usize,
) -> *mut u8 {
    if data.is_null() || tokens.is_null() { return core::ptr::null_mut(); }
    unsafe {
        let t = &*tokens.add(index);
        let len = (t.end - t.start) as usize;
        let buf = crate::heap::miku_malloc(len + 1);
        if buf.is_null() { return core::ptr::null_mut(); }
        mem::miku_memcpy(buf, data.add(t.start as usize), len);
        *buf.add(len) = 0;
        buf
    }
}

//   validate JSON structure
// Returns 0 if valid, -1 if invalid

#[no_mangle]
pub extern "C" fn miku_json_validate(data: *const u8, len: usize) -> i32 {
    if data.is_null() || len == 0 { return -1; }
    let mut parser: MikuJsonParser = unsafe { core::mem::zeroed() };
    miku_json_init(&mut parser);
    let mut tokens = [EMPTY_TOKEN; 256];
    let max = (len / 2 + 1).min(tokens.len());
    let n = miku_json_parse(
        &mut parser, data, len,
        tokens.as_mut_ptr(), max,
    );
    if n < 0 { -1 } else { 0 }
}
