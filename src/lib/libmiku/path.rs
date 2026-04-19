// Path manipulation utilities
// POSIX-style path operations: basename, dirname, join, extension, normalize

use crate::heap;
use crate::mem;
use crate::string;

// Basename: return heap-allocated last component of path
// Strips trailing slashes before extracting the component
// "/usr/lib/file.txt" -> "file.txt"
// "/usr/lib/" -> "lib"
// "/" -> "/"
// Caller must free the result

#[no_mangle]
pub extern "C" fn miku_basename(path: *const u8) -> *mut u8 {
    if path.is_null() { return string::miku_strdup(b".\0".as_ptr()); }
    let len = string::miku_strlen(path);
    if len == 0 { return string::miku_strdup(b".\0".as_ptr()); }

    // skip trailing slashes
    let mut end = len;
    while end > 0 && unsafe { *path.add(end - 1) } == b'/' {
        end -= 1;
    }
    if end == 0 {
        // path is all slashes - return "/"
        return string::miku_strdup(b"/\0".as_ptr());
    }

    // find last slash before end
    let mut start = end;
    while start > 0 {
        start -= 1;
        if unsafe { *path.add(start) } == b'/' {
            start += 1;
            break;
        }
    }

    // component is path[start..end]
    string::miku_strndup(unsafe { path.add(start) }, end - start)
}

// Dirname: return heap-allocated string of directory part
// "/usr/lib/file.txt" -> "/usr/lib"
// Caller must free the result

#[no_mangle]
pub extern "C" fn miku_dirname(path: *const u8) -> *mut u8 {
    if path.is_null() { return string::miku_strdup(b".\0".as_ptr()); }
    let len = string::miku_strlen(path);
    if len == 0 { return string::miku_strdup(b".\0".as_ptr()); }

    // skip trailing slashes
    let mut end = len;
    while end > 0 && unsafe { *path.add(end - 1) } == b'/' {
        end -= 1;
    }
    if end == 0 {
        return string::miku_strdup(b"/\0".as_ptr());
    }

    // find last slash
    let mut i = end;
    while i > 0 {
        i -= 1;
        if unsafe { *path.add(i) } == b'/' {
            // skip consecutive slashes
            while i > 0 && unsafe { *path.add(i - 1) } == b'/' {
                i -= 1;
            }
            if i == 0 { i = 1; } // keep root slash
            return string::miku_strndup(path, i);
        }
    }

    // no slash found
    string::miku_strdup(b".\0".as_ptr())
}

// Extension: return heap-allocated file extension (after last dot)
// "file.tar.gz" -> "gz"
// Returns null if no extension, caller must free the result

#[no_mangle]
pub extern "C" fn miku_path_ext(path: *const u8) -> *mut u8 {
    if path.is_null() { return core::ptr::null_mut(); }

    let base = miku_basename(path);
    if base.is_null() { return core::ptr::null_mut(); }
    let base_len = string::miku_strlen(base);
    if base_len == 0 {
        heap::miku_free(base);
        return core::ptr::null_mut();
    }

    // search from end for dot
    let mut i = base_len;
    let mut dot_pos = 0usize;
    let mut found = false;
    while i > 0 {
        i -= 1;
        if unsafe { *base.add(i) } == b'.' {
            // don't count leading dot (hidden files)
            if i == 0 { break; }
            dot_pos = i + 1;
            found = true;
            break;
        }
    }

    if !found {
        heap::miku_free(base);
        return core::ptr::null_mut();
    }

    let result = string::miku_strdup(unsafe { base.add(dot_pos) });
    heap::miku_free(base);
    result
}

// Stem: return heap-allocated filename without extension
// "file.tar.gz" -> "file.tar"
// Caller must free

#[no_mangle]
pub extern "C" fn miku_path_stem(path: *const u8) -> *mut u8 {
    if path.is_null() { return core::ptr::null_mut(); }

    let base = miku_basename(path);
    if base.is_null() { return core::ptr::null_mut(); }
    let base_len = string::miku_strlen(base);
    if base_len == 0 {
        let result = string::miku_strdup(base);
        heap::miku_free(base);
        return result;
    }

    // find last dot
    let mut dot_pos = base_len; // no dot found
    let mut i = base_len;
    while i > 0 {
        i -= 1;
        if unsafe { *base.add(i) } == b'.' && i > 0 {
            dot_pos = i;
            break;
        }
    }

    let result = string::miku_strndup(base, dot_pos);
    heap::miku_free(base);
    result
}

// Join: concatenate two paths with separator
// Heap-allocated result. Caller must free
// miku_path_join("/usr", "lib") -> "/usr/lib"
// miku_path_join("/usr/", "lib") -> "/usr/lib"
// miku_path_join("", "lib") -> "lib"
// miku_path_join("/usr", "/lib") -> "/lib" (absolute second path wins)

#[no_mangle]
pub extern "C" fn miku_path_join(a: *const u8, b: *const u8) -> *mut u8 {
    if a.is_null() || string::miku_strlen(a) == 0 {
        return if b.is_null() { string::miku_strdup(b"\0".as_ptr()) } else { string::miku_strdup(b) };
    }
    if b.is_null() || string::miku_strlen(b) == 0 {
        return string::miku_strdup(a);
    }

    // if b starts with /, it's absolute - return copy of b
    if unsafe { *b } == b'/' {
        return string::miku_strdup(b);
    }

    let a_len = string::miku_strlen(a);
    let b_len = string::miku_strlen(b);
    let need_sep = unsafe { *a.add(a_len - 1) } != b'/';

    let total = a_len + b_len + if need_sep { 1 } else { 0 } + 1;
    let buf = heap::miku_malloc(total);
    if buf.is_null() { return core::ptr::null_mut(); }

    mem::miku_memcpy(buf, a, a_len);
    let mut pos = a_len;
    if need_sep {
        unsafe { *buf.add(pos) = b'/'; }
        pos += 1;
    }
    mem::miku_memcpy(unsafe { buf.add(pos) }, b, b_len);
    pos += b_len;
    unsafe { *buf.add(pos) = 0; }

    buf
}

// Normalize: resolve "." and ".." in a path 
// Heap-allocated result. Caller must free
// "/usr/lib/../bin/./test" -> "/usr/bin/test"

#[no_mangle]
pub extern "C" fn miku_path_normalize(path: *const u8) -> *mut u8 {
    if path.is_null() { return string::miku_strdup(b"/\0".as_ptr()); }
    let len = string::miku_strlen(path);
    if len == 0 { return string::miku_strdup(b".\0".as_ptr()); }

    let is_absolute = unsafe { *path } == b'/';

    // allocate output buffer (same size + 1 is always enough)
    let buf = heap::miku_malloc(len + 2);
    if buf.is_null() { return core::ptr::null_mut(); }

    // stack of component start positions in buf
    // max depth = len/2 components
    let stack_max = len / 2 + 1;
    let stack_bytes = stack_max * core::mem::size_of::<usize>();
    let stack_raw = heap::miku_malloc(stack_bytes);
    if stack_raw.is_null() {
        heap::miku_free(buf);
        return core::ptr::null_mut();
    }
    let stack = stack_raw as *mut usize;
    let mut stack_len = 0usize;

    let mut out_pos = 0usize;
    if is_absolute {
        unsafe { *buf.add(out_pos) = b'/'; }
        out_pos += 1;
    }

    let mut i = 0usize;
    while i < len {
        // skip slashes
        while i < len && unsafe { *path.add(i) } == b'/' { i += 1; }
        if i >= len { break; }

        // find component end
        let comp_start = i;
        while i < len && unsafe { *path.add(i) } != b'/' { i += 1; }
        let comp_len = i - comp_start;

        // check for "." - skip
        if comp_len == 1 && unsafe { *path.add(comp_start) } == b'.' {
            continue;
        }

        // check for ".." - pop
        if comp_len == 2
            && unsafe { *path.add(comp_start) } == b'.'
            && unsafe { *path.add(comp_start + 1) } == b'.'
        {
            if stack_len > 0 && unsafe { *stack.add(stack_len - 1) } != usize::MAX {
                // top of stack is a regular directory - pop it
                stack_len -= 1;
                out_pos = unsafe { *stack.add(stack_len) };
            } else if !is_absolute {
                // relative path: keep the ".." and mark it as a dotdot slot
                unsafe { *stack.add(stack_len) = usize::MAX; } // sentinel
                stack_len += 1;
                mem::miku_memcpy(unsafe { buf.add(out_pos) }, unsafe { path.add(comp_start) }, comp_len);
                out_pos += comp_len;
                unsafe { *buf.add(out_pos) = b'/'; }
                out_pos += 1;
            }
            continue;
        }

        // normal component - push
        unsafe { *stack.add(stack_len) = out_pos; }
        stack_len += 1;
        mem::miku_memcpy(unsafe { buf.add(out_pos) }, unsafe { path.add(comp_start) }, comp_len);
        out_pos += comp_len;
        unsafe { *buf.add(out_pos) = b'/'; }
        out_pos += 1;
    }

    // trim trailing slash (unless root)
    if out_pos > 1 && unsafe { *buf.add(out_pos - 1) } == b'/' {
        out_pos -= 1;
    }

    // empty result
    if out_pos == 0 {
        unsafe { *buf = b'.'; }
        out_pos = 1;
    }

    unsafe { *buf.add(out_pos) = 0; }
    heap::miku_free(stack_raw);
    buf
}

// is_absolute: check if path starts with /

#[no_mangle]
pub extern "C" fn miku_path_is_absolute(path: *const u8) -> bool {
    if path.is_null() { return false; }
    unsafe { *path == b'/' }
}

// Depth: count number of components in path
// "/usr/lib/file" -> 3

#[no_mangle]
pub extern "C" fn miku_path_depth(path: *const u8) -> usize {
    if path.is_null() { return 0; }
    let len = string::miku_strlen(path);
    let mut count = 0usize;
    let mut i = 0usize;
    let mut in_component = false;

    while i < len {
        let c = unsafe { *path.add(i) };
        if c == b'/' {
            if in_component { in_component = false; }
        } else {
            if !in_component {
                count += 1;
                in_component = true;
            }
        }
        i += 1;
    }

    count
}

// has_extension: check if path has given extension (case-sensitive)
// miku_path_has_ext("file.txt", "txt") -> true
#[no_mangle]
pub extern "C" fn miku_path_has_ext(path: *const u8, ext: *const u8) -> bool {
    if path.is_null() || ext.is_null() { return false; }
    let got = miku_path_ext(path);
    if got.is_null() { return false; }
    let result = string::miku_strcmp(got, ext) == 0;
    heap::miku_free(got);
    result
}

// common_prefix: longest common directory prefix of two paths
// Heap-allocated result. Caller must free.
// common_prefix("/usr/lib/a", "/usr/bin/b") -> "/usr"
#[no_mangle]
pub extern "C" fn miku_path_common(a: *const u8, b: *const u8) -> *mut u8 {
    if a.is_null() || b.is_null() { return string::miku_strdup(b"\0".as_ptr()); }
    let alen = string::miku_strlen(a);
    let blen = string::miku_strlen(b);
    let limit = if alen < blen { alen } else { blen };

    let mut last_sep = 0usize;
    let mut i = 0usize;
    unsafe {
        while i < limit && *a.add(i) == *b.add(i) {
            if *a.add(i) == b'/' {
                last_sep = i;
            }
            i += 1;
        }
    }

    // if they diverged right away
    if i == 0 { return string::miku_strdup(b"\0".as_ptr()); }

    // if they matched fully up to one string's end
    if i == limit {
        if i == alen && (i == blen || unsafe { *b.add(i) } == b'/') {
            return string::miku_strndup(a, alen);
        }
        if i == blen && unsafe { *a.add(i) } == b'/' {
            return string::miku_strndup(b, blen);
        }
    }

    // use last separator position
    if last_sep == 0 && unsafe { *a } == b'/' {
        return string::miku_strdup(b"/\0".as_ptr());
    }
    if last_sep == 0 { return string::miku_strdup(b"\0".as_ptr()); }
    string::miku_strndup(a, last_sep)
}

// is_relative: check if path is relative (does not start with /)
#[no_mangle]
pub extern "C" fn miku_path_is_relative(path: *const u8) -> bool {
    !miku_path_is_absolute(path)
}

// parent: same as dirname but returns "/" for root paths, "." for relative single-component
#[no_mangle]
pub extern "C" fn miku_path_parent(path: *const u8) -> *mut u8 {
    miku_dirname(path)
}
