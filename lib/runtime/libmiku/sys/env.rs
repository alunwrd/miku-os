// The process environment
//
// A growable array of heap-allocated "KEY=VALUE" strings in `execve`'s shape:
// NULL-terminated, one C string each. `miku_environ` passes that array to a
// spawn as it is. Bounded only by the heap.
//
// `miku_getenv` returns a pointer into the stored string, valid until that
// variable is next set or removed, as POSIX `getenv` specifies. `miku_getenv_r`
// copies into the caller's buffer.

use crate::heap;
use crate::mem;
use crate::string;
use crate::sync::SpinLock;

/// Guards against an `envp` with no NULL terminator; not a limit on a program's
/// variables
const MAX_IMPORT: usize = 4096;

struct EnvStore {
    /// `environ`: `len` pointers to "KEY=VALUE" C strings, then a NULL. `cap`
    /// counts pointer slots including that NULL
    environ: *mut *mut u8,
    len: usize,
    cap: usize,
}

// SAFETY: every access goes through the SpinLock below, and only this table
// frees the strings
unsafe impl Send for EnvStore {}

/// `fork`'s hold on the environment, outside the heap's, since `setenv`
/// allocates with this held. See `proc::miku_fork`
pub extern "C" fn fork_lock_env() {
    ENV.lock_raw();
}

/// # Safety
/// After `fork_lock_env`, once in each process
pub unsafe fn fork_unlock_env() {
    ENV.unlock_raw();
}

static ENV: SpinLock<EnvStore> = SpinLock::new(EnvStore {
    environ: core::ptr::null_mut(),
    len: 0,
    cap: 0,
});

/// Length of the key in a "KEY=VALUE" string, or `None` with no `=` or an empty
/// key
fn key_len(entry: *const u8) -> Option<usize> {
    if entry.is_null() { return None; }
    let mut i = 0usize;
    loop {
        let c = unsafe { *entry.add(i) };
        if c == 0 { return None; }
        if c == b'=' { return if i == 0 { None } else { Some(i) }; }
        i += 1;
    }
}

/// Does `entry` begin with `key` followed by `=`?
fn entry_matches(entry: *const u8, key: *const u8, klen: usize) -> bool {
    for i in 0..klen {
        let a = unsafe { *entry.add(i) };
        let b = unsafe { *key.add(i) };
        if a == 0 || a != b { return false; }
    }
    unsafe { *entry.add(klen) == b'=' }
}

fn find(store: &EnvStore, key: *const u8) -> Option<usize> {
    let klen = string::miku_strlen(key);
    if klen == 0 { return None; }
    for i in 0..store.len {
        let e = unsafe { *store.environ.add(i) };
        if entry_matches(e, key, klen) { return Some(i); }
    }
    None
}

/// Room for one more pointer plus the NULL. Doubles, so building by `setenv` is
/// linear
fn reserve(store: &mut EnvStore) -> bool {
    if store.len + 2 <= store.cap { return true; }
    let want = if store.cap == 0 { 8 } else { store.cap * 2 };
    let bytes = match want.checked_mul(core::mem::size_of::<*mut u8>()) {
        Some(n) => n,
        None    => return false,
    };
    let fresh = heap::miku_malloc(bytes) as *mut *mut u8;
    if fresh.is_null() { return false; }
    unsafe {
        for i in 0..store.len {
            *fresh.add(i) = *store.environ.add(i);
        }
        *fresh.add(store.len) = core::ptr::null_mut();
    }
    if !store.environ.is_null() {
        heap::miku_free(store.environ as *mut u8);
    }
    store.environ = fresh;
    store.cap = want;
    true
}

/// A heap copy of "KEY=VALUE", built from its two halves.
fn make_entry(key: *const u8, klen: usize, val: *const u8) -> *mut u8 {
    let vlen = string::miku_strlen(val);
    let total = klen + 1 + vlen + 1;
    let p = heap::miku_malloc(total);
    if p.is_null() { return p; }
    unsafe {
        mem::miku_memcpy(p, key, klen);
        *p.add(klen) = b'=';
        if vlen > 0 { mem::miku_memcpy(p.add(klen + 1), val, vlen); }
        *p.add(klen + 1 + vlen) = 0;
    }
    p
}

// set environment variable, replacing any previous value. A null key or value
// is refused
#[no_mangle]
pub extern "C" fn miku_setenv(key: *const u8, val: *const u8) -> bool {
    if key.is_null() || val.is_null() { return false; }
    let klen = string::miku_strlen(key);
    if klen == 0 { return false; }
    // A key containing '=' would parse as a different key
    for i in 0..klen {
        if unsafe { *key.add(i) } == b'=' { return false; }
    }

    let entry = make_entry(key, klen, val);
    if entry.is_null() { return false; }

    let mut store = ENV.lock();
    if let Some(i) = find(&store, key) {
        let old = unsafe { *store.environ.add(i) };
        unsafe { *store.environ.add(i) = entry; }
        // Freed only after the slot stops pointing at it, so a racing reader
        // sees one of the two strings
        if !old.is_null() { heap::miku_free(old); }
        return true;
    }
    if !reserve(&mut store) {
        heap::miku_free(entry);
        return false;
    }
    unsafe {
        *store.environ.add(store.len) = entry;
        *store.environ.add(store.len + 1) = core::ptr::null_mut();
    }
    store.len += 1;
    true
}

/// The value of a variable, or NULL. The pointer is into the stored entry,
/// valid until that variable is next set or removed, as POSIX `getenv`
#[no_mangle]
pub extern "C" fn miku_getenv(key: *const u8) -> *const u8 {
    if key.is_null() { return core::ptr::null(); }
    let store = ENV.lock();
    match find(&store, key) {
        Some(i) => {
            let klen = string::miku_strlen(key);
            unsafe { (*store.environ.add(i)).add(klen + 1) as *const u8 }
        }
        None => core::ptr::null(),
    }
}

// Copies a variable's value into the caller's buffer. Returns its full length,
// or -1 if absent. Truncated to fit and always NUL-terminated
#[no_mangle]
pub extern "C" fn miku_getenv_r(key: *const u8, buf: *mut u8, buf_size: usize) -> i32 {
    if key.is_null() || buf.is_null() || buf_size == 0 { return -1; }
    let store = ENV.lock();
    let Some(i) = find(&store, key) else { return -1 };
    let klen = string::miku_strlen(key);
    let val = unsafe { (*store.environ.add(i)).add(klen + 1) };
    let vlen = string::miku_strlen(val);
    let copy = if vlen < buf_size { vlen } else { buf_size - 1 };
    unsafe {
        mem::miku_memcpy(buf, val, copy);
        *buf.add(copy) = 0;
    }
    vlen as i32
}

// remove environment variable
#[no_mangle]
pub extern "C" fn miku_unsetenv(key: *const u8) -> bool {
    if key.is_null() { return false; }
    let mut store = ENV.lock();
    let Some(i) = find(&store, key) else { return false };
    let victim = unsafe { *store.environ.add(i) };
    unsafe {
        // Order matters: the last entry moves down, the NULL follows it, then
        // the string is freed
        *store.environ.add(i) = *store.environ.add(store.len - 1);
        *store.environ.add(store.len - 1) = core::ptr::null_mut();
    }
    store.len -= 1;
    if !victim.is_null() { heap::miku_free(victim); }
    true
}

// check if variable exists
#[no_mangle]
pub extern "C" fn miku_hasenv(key: *const u8) -> bool {
    !miku_getenv(key).is_null()
}

// get number of defined variables
#[no_mangle]
pub extern "C" fn miku_env_count() -> usize {
    ENV.lock().len
}

/// The environment as `execve` wants it: a NULL-terminated array of "KEY=VALUE"
/// C strings. The array is the storage, valid until the next `setenv`,
/// `unsetenv`, or `env_clear`
#[no_mangle]
pub extern "C" fn miku_environ() -> *mut *mut u8 {
    ENV.lock().environ
}

// clear all environment variables
#[no_mangle]
pub extern "C" fn miku_env_clear() {
    let mut store = ENV.lock();
    for i in 0..store.len {
        let e = unsafe { *store.environ.add(i) };
        if !e.is_null() { heap::miku_free(e); }
        unsafe { *store.environ.add(i) = core::ptr::null_mut(); }
    }
    store.len = 0;
    if !store.environ.is_null() {
        unsafe { *store.environ = core::ptr::null_mut(); }
    }
}

// Iterates over all variables. Callback: fn(key: *const u8, val: *const u8,
// ctx: *mut u8).
//
// The key points into the entry and is followed by '=', not a NUL.
// `miku_environ` gives the raw "KEY=VALUE" form
use miku_abi::libtypes::EnvCallback;

#[no_mangle]
pub extern "C" fn miku_env_iter(cb: EnvCallback, ctx: *mut u8) {
    let store = ENV.lock();
    for i in 0..store.len {
        let e = unsafe { *store.environ.add(i) };
        let Some(klen) = key_len(e) else { continue };
        cb(e as *const u8, unsafe { e.add(klen + 1) as *const u8 }, ctx);
    }
}

// parse "KEY=VALUE" and set it
#[no_mangle]
pub extern "C" fn miku_putenv(s: *const u8) -> bool {
    let Some(klen) = key_len(s) else { return false };
    let entry = make_entry(s, klen, unsafe { s.add(klen + 1) });
    if entry.is_null() { return false; }

    let mut store = ENV.lock();
    // Find by the key part of `s` itself, without copying it out first
    let mut existing = None;
    for i in 0..store.len {
        let e = unsafe { *store.environ.add(i) };
        if entry_matches(e, s, klen) { existing = Some(i); break; }
    }
    if let Some(i) = existing {
        let old = unsafe { *store.environ.add(i) };
        unsafe { *store.environ.add(i) = entry; }
        if !old.is_null() { heap::miku_free(old); }
        return true;
    }
    if !reserve(&mut store) {
        heap::miku_free(entry);
        return false;
    }
    unsafe {
        *store.environ.add(store.len) = entry;
        *store.environ.add(store.len + 1) = core::ptr::null_mut();
    }
    store.len += 1;
    true
}

// Seeds the store from the kernel's envp (NULL-terminated "KEY=value" C strings
// on the initial stack, see MikuOS_ABI.md sec 4.4). Called once from
// _start_main before anything reads the environment. Entries without '=' are
// skipped. Returns the number imported
/// Seeds the environment from the kernel envp (3rd argument of
/// `_start_main`). Call once; returns the count
#[no_mangle]
pub extern "C" fn miku_env_init(envp: *const *const u8) -> usize {
    if envp.is_null() { return 0; }
    let mut imported = 0usize;
    let mut i = 0usize;
    while i < MAX_IMPORT {
        let entry = unsafe { *envp.add(i) };
        if entry.is_null() { break; }
        i += 1;
        if miku_putenv(entry) { imported += 1; }
    }
    imported
}
