// Environment variable storage
// In-process key=value store for userspace programs
// Static table with fixed max entries
// Keys and values are stored as inline byte arrays
// Thread-safe via SpinLock :)

use crate::string;
use crate::mem;
use crate::sync::SpinLock;

const MAX_ENV: usize = 128;
const MAX_KEY_LEN: usize = 63;
const MAX_VAL_LEN: usize = 255;

#[repr(C)]
#[derive(Copy, Clone)]
struct EnvEntry {
    key: [u8; MAX_KEY_LEN + 1],
    val: [u8; MAX_VAL_LEN + 1],
    used: bool,
}

const EMPTY_ENTRY: EnvEntry = EnvEntry {
    key: [0u8; MAX_KEY_LEN + 1],
    val: [0u8; MAX_VAL_LEN + 1],
    used: false,
};

struct EnvStore {
    table: [EnvEntry; MAX_ENV],
    count: usize,
}

static ENV: SpinLock<EnvStore> = SpinLock::new(EnvStore {
    table: [EMPTY_ENTRY; MAX_ENV],
    count: 0,
});

fn find_slot(store: &EnvStore, key: *const u8) -> Option<usize> {
    for i in 0..MAX_ENV {
        if store.table[i].used
            && string::miku_strcmp(store.table[i].key.as_ptr(), key) == 0
        {
            return Some(i);
        }
    }
    None
}

fn find_free(store: &EnvStore) -> Option<usize> {
    for i in 0..MAX_ENV {
        if !store.table[i].used {
            return Some(i);
        }
    }
    None
}

// set environment variable
#[no_mangle]
pub extern "C" fn miku_setenv(key: *const u8, val: *const u8) -> bool {
    if key.is_null() || val.is_null() { return false; }
    let klen = string::miku_strlen(key);
    let vlen = string::miku_strlen(val);
    if klen == 0 || klen > MAX_KEY_LEN || vlen > MAX_VAL_LEN { return false; }

    let mut store = ENV.lock();

    // update existing
    if let Some(i) = find_slot(&store, key) {
        unsafe { mem::miku_memcpy(store.table[i].val.as_mut_ptr(), val, vlen); }
        store.table[i].val[vlen] = 0;
        return true;
    }

    // insert new
    if let Some(i) = find_free(&store) {
        unsafe { mem::miku_memcpy(store.table[i].key.as_mut_ptr(), key, klen); }
        store.table[i].key[klen] = 0;
        unsafe { mem::miku_memcpy(store.table[i].val.as_mut_ptr(), val, vlen); }
        store.table[i].val[vlen] = 0;
        store.table[i].used = true;
        store.count += 1;
        return true;
    }

    false
}

// get environment variable
// Returns pointer to an internal static buffer. Matches POSIX getenv semantics:
// the pointer is valid only until the next miku_getenv call on ANY thread.
// NOT thread-safe for concurrent readers - use miku_getenv_r for that
// The spinlock on the buffer ensures the copy itself is atomic (no torn bytes),
// but the returned pointer may be overwritten the instant we return.
#[no_mangle]
pub extern "C" fn miku_getenv(key: *const u8) -> *const u8 {
    if key.is_null() { return core::ptr::null(); }

    static GETENV_BUF: SpinLock<[u8; MAX_VAL_LEN + 1]> = SpinLock::new([0u8; MAX_VAL_LEN + 1]);

    let store = ENV.lock();
    if let Some(i) = find_slot(&store, key) {
        let val = &store.table[i].val;
        let vlen = string::miku_strlen(val.as_ptr());
        let mut buf = GETENV_BUF.lock();
        mem::miku_memcpy(buf.as_mut_ptr(), val.as_ptr(), vlen);
        buf[vlen] = 0;
        return buf.as_ptr();
    }
    core::ptr::null()
}

// get environment variable, copying value into caller's buffer.
// returns length of value, or -1 if not found.
// this is the safe alternative to miku_getenv for concurrent use.
#[no_mangle]
pub extern "C" fn miku_getenv_r(key: *const u8, buf: *mut u8, buf_size: usize) -> i32 {
    if key.is_null() || buf.is_null() || buf_size == 0 { return -1; }
    let store = ENV.lock();
    if let Some(i) = find_slot(&store, key) {
        let val = &store.table[i].val;
        let vlen = string::miku_strlen(val.as_ptr());
        let copy = if vlen < buf_size { vlen } else { buf_size - 1 };
        unsafe {
            mem::miku_memcpy(buf, val.as_ptr(), copy);
            *buf.add(copy) = 0;
        }
        return vlen as i32;
    }
    -1
}

// remove environment variable
#[no_mangle]
pub extern "C" fn miku_unsetenv(key: *const u8) -> bool {
    if key.is_null() { return false; }
    let mut store = ENV.lock();
    if let Some(i) = find_slot(&store, key) {
        store.table[i].used = false;
        store.table[i].key[0] = 0;
        store.table[i].val[0] = 0;
        if store.count > 0 { store.count -= 1; }
        return true;
    }
    false
}

// check if variable exists
#[no_mangle]
pub extern "C" fn miku_hasenv(key: *const u8) -> bool {
    !miku_getenv(key).is_null()
}

// get number of defined variables
#[no_mangle]
pub extern "C" fn miku_env_count() -> usize {
    let store = ENV.lock();
    store.count
}

// clear all environment variables
#[no_mangle]
pub extern "C" fn miku_env_clear() {
    let mut store = ENV.lock();
    for i in 0..MAX_ENV {
        store.table[i].used = false;
        store.table[i].key[0] = 0;
        store.table[i].val[0] = 0;
    }
    store.count = 0;
}

// iterate over all variables
// Callback: fn(key: *const u8, val: *const u8, ctx: *mut u8)
type EnvCallback = extern "C" fn(*const u8, *const u8, *mut u8);

#[no_mangle]
pub extern "C" fn miku_env_iter(cb: EnvCallback, ctx: *mut u8) {
    let store = ENV.lock();
    for i in 0..MAX_ENV {
        if store.table[i].used {
            cb(
                store.table[i].key.as_ptr(),
                store.table[i].val.as_ptr(),
                ctx,
            );
        }
    }
}

// parse "KEY=VALUE" string and set it
#[no_mangle]
pub extern "C" fn miku_putenv(s: *const u8) -> bool {
    if s.is_null() { return false; }
    let slen = string::miku_strlen(s);

    let mut eq_pos = 0usize;
    let mut found = false;
    for i in 0..slen {
        if unsafe { *s.add(i) } == b'=' {
            eq_pos = i;
            found = true;
            break;
        }
    }
    if !found || eq_pos == 0 { return false; }
    if eq_pos > MAX_KEY_LEN { return false; }

    let mut key_buf = [0u8; MAX_KEY_LEN + 1];
    unsafe { mem::miku_memcpy(key_buf.as_mut_ptr(), s, eq_pos); }
    key_buf[eq_pos] = 0;

    miku_setenv(key_buf.as_ptr(), unsafe { s.add(eq_pos + 1) })
}
