#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strlen(s: *const u8) -> usize {
    if s.is_null() { return 0; }
    let mut n = 0usize;
    unsafe { while core::ptr::read_volatile(s.add(n)) != 0 { n += 1; } }
    n
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strcmp(a: *const u8, b: *const u8) -> i32 {
    if a.is_null() && b.is_null() { return 0; }
    if a.is_null() { return -1; }
    if b.is_null() { return 1; }
    let mut i = 0usize;
    unsafe {
        loop {
            let ca = core::ptr::read_volatile(a.add(i));
            let cb = core::ptr::read_volatile(b.add(i));
            if ca != cb { return ca as i32 - cb as i32; }
            if ca == 0 { return 0; }
            i += 1;
        }
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strncmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    if n == 0 { return 0; }
    let mut i = 0usize;
    unsafe {
        while i < n {
            let ca = if a.is_null() { 0 } else { core::ptr::read_volatile(a.add(i)) };
            let cb = if b.is_null() { 0 } else { core::ptr::read_volatile(b.add(i)) };
            if ca != cb { return ca as i32 - cb as i32; }
            if ca == 0 { return 0; }
            i += 1;
        }
    }
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strcpy(dst: *mut u8, src: *const u8) -> *mut u8 {
    if dst.is_null() || src.is_null() { return dst; }
    let mut i = 0usize;
    unsafe {
        loop {
            let c = core::ptr::read_volatile(src.add(i));
            core::ptr::write_volatile(dst.add(i), c);
            if c == 0 { break; }
            i += 1;
        }
    }
    dst
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strncpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if dst.is_null() || src.is_null() { return dst; }
    let mut i = 0usize;
    let mut done = false;
    unsafe {
        while i < n {
            if !done {
                let c = core::ptr::read_volatile(src.add(i));
                core::ptr::write_volatile(dst.add(i), c);
                if c == 0 { done = true; }
            } else {
                core::ptr::write_volatile(dst.add(i), 0);
            }
            i += 1;
        }
    }
    dst
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strcat(dst: *mut u8, src: *const u8) -> *mut u8 {
    if dst.is_null() || src.is_null() { return dst; }
    let dlen = miku_strlen(dst);
    let mut i = 0usize;
    unsafe {
        loop {
            let c = *src.add(i);
            *dst.add(dlen + i) = c;
            if c == 0 { break; }
            i += 1;
        }
    }
    dst
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strncat(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if dst.is_null() || src.is_null() { return dst; }
    let dlen = miku_strlen(dst);
    let mut i = 0usize;
    unsafe {
        while i < n {
            let c = *src.add(i);
            if c == 0 { break; }
            *dst.add(dlen + i) = c;
            i += 1;
        }
        *dst.add(dlen + i) = 0;
    }
    dst
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strchr(s: *const u8, c: i32) -> *const u8 {
    if s.is_null() { return core::ptr::null(); }
    let target = c as u8;
    let mut i = 0usize;
    unsafe {
        loop {
            let ch = core::ptr::read_volatile(s.add(i));
            if ch == target { return s.add(i); }
            if ch == 0 { return core::ptr::null(); }
            i += 1;
        }
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strrchr(s: *const u8, c: i32) -> *const u8 {
    if s.is_null() { return core::ptr::null(); }
    let target = c as u8;
    let mut last: *const u8 = core::ptr::null();
    let mut i = 0usize;
    unsafe {
        loop {
            let ch = core::ptr::read_volatile(s.add(i));
            if ch == target { last = s.add(i); }
            if ch == 0 { return last; }
            i += 1;
        }
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strstr(haystack: *const u8, needle: *const u8) -> *const u8 {
    if haystack.is_null() || needle.is_null() { return core::ptr::null(); }
    let nlen = miku_strlen(needle);
    if nlen == 0 { return haystack; }
    let hlen = miku_strlen(haystack);
    if nlen > hlen { return core::ptr::null(); }
    for i in 0..=(hlen - nlen) {
        let mut found = true;
        for j in 0..nlen {
            if unsafe { *haystack.add(i + j) != *needle.add(j) } {
                found = false;
                break;
            }
        }
        if found { return unsafe { haystack.add(i) }; }
    }
    core::ptr::null()
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strdup(s: *const u8) -> *mut u8 {
    if s.is_null() { return core::ptr::null_mut(); }
    let len = miku_strlen(s);
    let p = crate::heap::miku_malloc(len + 1);
    if p.is_null() { return core::ptr::null_mut(); }
    crate::mem::miku_memcpy(p, s, len + 1);
    p
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strndup(s: *const u8, n: usize) -> *mut u8 {
    if s.is_null() { return core::ptr::null_mut(); }
    let slen = miku_strlen(s);
    let len = if slen < n { slen } else { n };
    let p = crate::heap::miku_malloc(len + 1);
    if p.is_null() { return core::ptr::null_mut(); }
    crate::mem::miku_memcpy(p, s, len);
    unsafe { *p.add(len) = 0; }
    p
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strlcpy(dst: *mut u8, src: *const u8, size: usize) -> usize {
    if dst.is_null() || src.is_null() { return 0; }
    let slen = miku_strlen(src);
    if size > 0 {
        let copy = if slen < size { slen } else { size - 1 };
        unsafe {
            let mut i = 0usize;
            while i < copy { *dst.add(i) = *src.add(i); i += 1; }
            *dst.add(copy) = 0;
        }
    }
    slen
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strlcat(dst: *mut u8, src: *const u8, size: usize) -> usize {
    if dst.is_null() || src.is_null() { return 0; }
    let dlen = miku_strlen(dst);
    let slen = miku_strlen(src);
    if dlen >= size { return size + slen; }
    let avail = size - dlen - 1;
    let copy = if slen < avail { slen } else { avail };
    unsafe {
        let mut i = 0usize;
        while i < copy { *dst.add(dlen + i) = *src.add(i); i += 1; }
        *dst.add(dlen + copy) = 0;
    }
    dlen + slen
}

fn is_delim(c: u8, delim: *const u8) -> bool {
    if delim.is_null() { return false; }
    let mut i = 0usize;
    unsafe {
        while core::ptr::read_volatile(delim.add(i)) != 0 {
            if core::ptr::read_volatile(delim.add(i)) == c { return true; }
            i += 1;
        }
    }
    false
}

struct SendPtr(*mut u8);
unsafe impl Send for SendPtr {}

static STRTOK_STATE: crate::sync::SpinLock<SendPtr> =
    crate::sync::SpinLock::new(SendPtr(core::ptr::null_mut()));

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strtok(s: *mut u8, delim: *const u8) -> *mut u8 {
    let mut state = STRTOK_STATE.lock();
    let saved = state.0;
    let result = strtok_inner(s, delim, saved);
    state.0 = result.1;
    result.0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strtok_r(s: *mut u8, delim: *const u8, saveptr: *mut *mut u8) -> *mut u8 {
    if saveptr.is_null() { return core::ptr::null_mut(); }
    let saved = unsafe { *saveptr };
    let result = strtok_inner(s, delim, saved);
    unsafe { *saveptr = result.1; }
    result.0
}

fn strtok_inner(s: *mut u8, delim: *const u8, saved: *mut u8) -> (*mut u8, *mut u8) {
    unsafe {
        let mut p = if !s.is_null() { s } else { saved };
        if p.is_null() { return (core::ptr::null_mut(), core::ptr::null_mut()); }

        while *p != 0 && is_delim(*p, delim) { p = p.add(1); }
        if *p == 0 { return (core::ptr::null_mut(), core::ptr::null_mut()); }

        let start = p;
        while *p != 0 && !is_delim(*p, delim) { p = p.add(1); }

        if *p != 0 {
            *p = 0;
            (start, p.add(1))
        } else {
            (start, core::ptr::null_mut())
        }
    }
}

// bounded strlen //

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strnlen(s: *const u8, maxlen: usize) -> usize {
    if s.is_null() { return 0; }
    let mut n = 0usize;
    unsafe { while n < maxlen && core::ptr::read_volatile(s.add(n)) != 0 { n += 1; } }
    n
}

// case-insensitive comparison //

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strcasecmp(a: *const u8, b: *const u8) -> i32 {
    if a.is_null() && b.is_null() { return 0; }
    if a.is_null() { return -1; }
    if b.is_null() { return 1; }
    let mut i = 0usize;
    unsafe {
        loop {
            let ca = crate::ctype::miku_tolower(core::ptr::read_volatile(a.add(i)) as i32) as u8;
            let cb = crate::ctype::miku_tolower(core::ptr::read_volatile(b.add(i)) as i32) as u8;
            if ca != cb { return ca as i32 - cb as i32; }
            if ca == 0 { return 0; }
            i += 1;
        }
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strncasecmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    if n == 0 { return 0; }
    let mut i = 0usize;
    unsafe {
        while i < n {
            let ca = if a.is_null() { 0 } else { crate::ctype::miku_tolower(core::ptr::read_volatile(a.add(i)) as i32) as u8 };
            let cb = if b.is_null() { 0 } else { crate::ctype::miku_tolower(core::ptr::read_volatile(b.add(i)) as i32) as u8 };
            if ca != cb { return ca as i32 - cb as i32; }
            if ca == 0 { return 0; }
            i += 1;
        }
    }
    0
}

// strsep //

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strsep(stringp: *mut *mut u8, delim: *const u8) -> *mut u8 {
    if stringp.is_null() || delim.is_null() { return core::ptr::null_mut(); }
    unsafe {
        let s = *stringp;
        if s.is_null() { return core::ptr::null_mut(); }

        let begin = s;
        let mut p = s;
        while *p != 0 {
            if is_delim(*p, delim) {
                *p = 0;
                *stringp = p.add(1);
                return begin;
            }
            p = p.add(1);
        }
        *stringp = core::ptr::null_mut();
        begin
    }
}

// string scanning functions (POSIX) //

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strpbrk(s: *const u8, accept: *const u8) -> *const u8 {
    if s.is_null() || accept.is_null() { return core::ptr::null(); }
    let mut i = 0usize;
    unsafe {
        while *s.add(i) != 0 {
            let c = *s.add(i);
            let mut j = 0usize;
            while *accept.add(j) != 0 {
                if c == *accept.add(j) { return s.add(i); }
                j += 1;
            }
            i += 1;
        }
    }
    core::ptr::null()
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strspn(s: *const u8, accept: *const u8) -> usize {
    if s.is_null() || accept.is_null() { return 0; }
    let mut i = 0usize;
    unsafe {
        while *s.add(i) != 0 {
            let c = *s.add(i);
            let mut found = false;
            let mut j = 0usize;
            while *accept.add(j) != 0 {
                if c == *accept.add(j) { found = true; break; }
                j += 1;
            }
            if !found { break; }
            i += 1;
        }
    }
    i
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strcspn(s: *const u8, reject: *const u8) -> usize {
    if s.is_null() || reject.is_null() { return 0; }
    let mut i = 0usize;
    unsafe {
        while *s.add(i) != 0 {
            let c = *s.add(i);
            let mut j = 0usize;
            while *reject.add(j) != 0 {
                if c == *reject.add(j) { return i; }
                j += 1;
            }
            i += 1;
        }
    }
    i
}
