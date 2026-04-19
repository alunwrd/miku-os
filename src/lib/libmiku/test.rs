// test.rs - lightweight test framework
//
// Provides test runner infrastructure for userspace programs.
// Supports test suites, assertions, and summary output.
// Thread-safe counters via atomics.

use crate::io;
use crate::string;
use crate::num;
use core::sync::atomic::{AtomicU32, Ordering};

static TEST_PASS: AtomicU32 = AtomicU32::new(0);
static TEST_FAIL: AtomicU32 = AtomicU32::new(0);
static TEST_NUM: AtomicU32 = AtomicU32::new(0);

fn write_out(s: &[u8]) {
    io::miku_write(1, s.as_ptr(), s.len());
}

fn write_str(s: *const u8) {
    if s.is_null() { return; }
    let len = string::miku_strlen(s);
    io::miku_write(1, s, len);
}

// reset test counters //
#[no_mangle]
pub extern "C" fn miku_test_reset() {
    TEST_PASS.store(0, Ordering::Relaxed);
    TEST_FAIL.store(0, Ordering::Relaxed);
    TEST_NUM.store(0, Ordering::Relaxed);
}

// Run a single test assertion
// Returns true if condition passed
#[no_mangle]
pub extern "C" fn miku_test(name: *const u8, condition: bool) -> bool {
    let num = TEST_NUM.fetch_add(1, Ordering::Relaxed) + 1;

    let mut buf = [0u8; 12];
    num::miku_itoa(num as i64, buf.as_mut_ptr());

    write_out(b"#");
    write_str(buf.as_ptr());
    write_out(b" ");

    if !name.is_null() {
        write_str(name);
    }

    if condition {
        TEST_PASS.fetch_add(1, Ordering::Relaxed);
        write_out(b" -> ok\n");
    } else {
        TEST_FAIL.fetch_add(1, Ordering::Relaxed);
        write_out(b" -> FAIL\n");
    }

    condition
}

// assert equality (i64) //
#[no_mangle]
pub extern "C" fn miku_test_eq(name: *const u8, actual: i64, expected: i64) -> bool {
    let ok = actual == expected;
    if !miku_test(name, ok) {
        write_out(b"  expected: ");
        let mut buf = [0u8; 24];
        num::miku_itoa(expected, buf.as_mut_ptr());
        write_str(buf.as_ptr());
        write_out(b", got: ");
        num::miku_itoa(actual, buf.as_mut_ptr());
        write_str(buf.as_ptr());
        write_out(b"\n");
    }
    ok
}

// assert string equality //
#[no_mangle]
pub extern "C" fn miku_test_streq(
    name: *const u8,
    actual: *const u8,
    expected: *const u8,
) -> bool {
    let ok = if actual.is_null() || expected.is_null() {
        actual.is_null() && expected.is_null()
    } else {
        string::miku_strcmp(actual, expected) == 0
    };

    if !miku_test(name, ok) {
        write_out(b"  expected: \"");
        if !expected.is_null() { write_str(expected); }
        write_out(b"\", got: \"");
        if !actual.is_null() { write_str(actual); }
        write_out(b"\"\n");
    }
    ok
}

// assert not null //
#[no_mangle]
pub extern "C" fn miku_test_not_null(name: *const u8, ptr: *const u8) -> bool {
    miku_test(name, !ptr.is_null())
}

// assert null //
#[no_mangle]
pub extern "C" fn miku_test_null(name: *const u8, ptr: *const u8) -> bool {
    miku_test(name, ptr.is_null())
}

// print test suite header //
#[no_mangle]
pub extern "C" fn miku_test_suite(name: *const u8) {
    write_out(b"\n--- ");
    if !name.is_null() { write_str(name); }
    write_out(b" ---\n");
}

// print summary and return exit code
// Returns 0 if all passed, 1 otherwise.
#[no_mangle]
pub extern "C" fn miku_test_summary() -> i32 {
    write_out(b"\n==================================\n");

    let mut buf = [0u8; 12];
    let pass = TEST_PASS.load(Ordering::Relaxed);
    let fail = TEST_FAIL.load(Ordering::Relaxed);
    let total = TEST_NUM.load(Ordering::Relaxed);

    num::miku_itoa(pass as i64, buf.as_mut_ptr());
    write_str(buf.as_ptr());
    write_out(b"/");
    num::miku_itoa(total as i64, buf.as_mut_ptr());
    write_str(buf.as_ptr());
    write_out(b" tests passed\n");

    if fail == 0 {
        write_out(b"all ok\n");
        0
    } else {
        num::miku_itoa(fail as i64, buf.as_mut_ptr());
        write_str(buf.as_ptr());
        write_out(b" failed\n");
        1
    }
}

// get pass/fail counts //
#[no_mangle]
pub extern "C" fn miku_test_passed() -> u32 { TEST_PASS.load(Ordering::Relaxed) }

#[no_mangle]
pub extern "C" fn miku_test_failed() -> u32 { TEST_FAIL.load(Ordering::Relaxed) }

#[no_mangle]
pub extern "C" fn miku_test_total() -> u32 { TEST_NUM.load(Ordering::Relaxed) }
