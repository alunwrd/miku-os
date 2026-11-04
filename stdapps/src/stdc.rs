//! C in a MikuOS program
//!
//! - `c/stdc.c`, compiled by `build.rs` through `cc` and `lib/c/miku-cc`:
//!   `snprintf` with stack arguments and floats, `strtod`, pthreads (mutex,
//!   condvar, keys, recursive, timed wait), the calendar, stdio, wide ctype,
//!   and header layouts against Rust's `libc`
//! - zstd, tree-sitter with a grammar, and bundled SQLite, through their
//!   crates.io Rust crates

use std::ffi::{CStr, CString, c_char, c_int, c_long};

unsafe extern "C" {
    fn stdc_sizeof(name: *const c_char) -> usize;
    fn stdc_const(name: *const c_char) -> c_long;
    fn stdc_format(out: *mut c_char, n: usize, which: c_int) -> c_int;
    fn stdc_strtod(s: *const c_char, used: *mut c_int) -> f64;
    fn stdc_errno_after_strtod(s: *const c_char) -> c_int;
    fn stdc_threads() -> c_long;
    fn stdc_sync_edges() -> c_int;
    fn stdc_time(out: *mut c_char, n: usize) -> c_int;
    fn stdc_stdio(path: *const c_char) -> c_int;
    fn stdc_wide() -> c_int;
    fn stdc_math() -> f64;
    fn stdc_auxv() -> c_int;
    fn stdc_locks(path: *const c_char) -> c_int;
}

fn check(name: &str, ok: bool, failed: &mut i32) {
    println!("  {}  {name}", if ok { "ok  " } else { "FAIL" });
    if !ok {
        *failed += 1;
    }
}

fn check_eq(name: &str, got: &str, want: &str, failed: &mut i32) {
    if got == want {
        println!("  ok    {name}");
    } else {
        println!("  FAIL  {name}: got {got:?}, wanted {want:?}");
        *failed += 1;
    }
}

fn c(s: &str) -> CString {
    CString::new(s).unwrap()
}

fn format(which: i32) -> (i32, String) {
    let mut buf = [0 as c_char; 256];
    let r = unsafe { stdc_format(buf.as_mut_ptr(), buf.len(), which) };
    let s = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned();
    (r, s)
}

fn main() {
    println!("stdc: C through lib/c - libc, pthreads, zstd, tree-sitter, SQLite");
    let mut failed = 0;

    // The headers' layouts against the Rust libc module's
    {
        let pairs: [(&str, usize); 11] = [
            ("stat", size_of::<libc::stat>()),
            ("dirent", size_of::<libc::dirent>()),
            ("tm", size_of::<libc::tm>()),
            ("timespec", size_of::<libc::timespec>()),
            ("timeval", size_of::<libc::timeval>()),
            ("pollfd", size_of::<libc::pollfd>()),
            ("flock", size_of::<libc::flock>()),
            ("pthread_mutex_t", size_of::<libc::pthread_mutex_t>()),
            ("pthread_cond_t", size_of::<libc::pthread_cond_t>()),
            ("pthread_rwlock_t", size_of::<libc::pthread_rwlock_t>()),
            ("pthread_attr_t", size_of::<libc::pthread_attr_t>()),
        ];
        let bad: Vec<String> = pairs
            .iter()
            .filter_map(|(n, rust)| {
                let cs = unsafe { stdc_sizeof(c(n).as_ptr()) };
                (cs != *rust).then(|| format!("{n}: C {cs}, Rust {rust}"))
            })
            .collect();
        check_eq("C and Rust agree on eleven layouts", &bad.join("; "), "", &mut failed);
        let consts = [
            ("O_CREAT", libc::O_CREAT as c_long),
            ("O_TRUNC", libc::O_TRUNC as c_long),
            ("EAGAIN", libc::EAGAIN as c_long),
            ("ENOENT", libc::ENOENT as c_long),
            ("S_IFDIR", libc::S_IFDIR as c_long),
            ("MAP_SHARED", libc::MAP_SHARED as c_long),
        ];
        let bad: Vec<String> = consts
            .iter()
            .filter_map(|(n, rust)| {
                let cv = unsafe { stdc_const(c(n).as_ptr()) };
                (cv != *rust).then(|| format!("{n}: C {cv}, Rust {rust}"))
            })
            .collect();
        check_eq("and on the generated constants", &bad.join("; "), "", &mut failed);
    }

    // printf
    {
        let want = [
            "42|   42|42   |00042|+42| 42|ff|0XFF|10|010|4000000000|M|miku|abc|%",
            "-9000000000 -1 -2 44 7 -5 deadbeef",
            "3.141590|2.67|    -1.500|0.2       |1.234568e+04|1.230E-04|100000|1e-05|0.1|0.3333333333|0x1p+0",
            "     1|2     |2.000|xy|(nil)|(null)",
            "inf -inf nan -0.000000 0.000000e+00",
            "9:trun",
            "asprintf-7-7.2",
        ];
        let names = [
            "snprintf: flags, widths, fifteen arguments",
            "snprintf: every integer length",
            "snprintf: %f %e %g %a",
            "snprintf: * widths and precisions, NULLs",
            "snprintf: infinities, NaN, negative zero",
            "snprintf: truncation returns the full length",
            "asprintf allocates what it formats",
        ];
        for (i, w) in want.iter().enumerate() {
            let (r, s) = format(i as i32);
            let got = if r as usize == s.len() || i == 5 { s } else { format!("{s} (returned {r})") };
            check_eq(names[i], &got, w, &mut failed);
        }
    }

    // strtod
    {
        let cases: [(&str, f64, i32); 6] = [
            ("1.5e3xyz", 1500.0, 5),
            ("  -0x1.8p1", -3.0, 10),
            ("0.1", 0.1, 3),
            ("2.2250738585072014e-308", 2.2250738585072014e-308, 23),
            ("infinity", f64::INFINITY, 8),
            ("abc", 0.0, 0),
        ];
        let bad: Vec<String> = cases
            .iter()
            .filter_map(|(s, v, n)| {
                let mut used = -1;
                let got = unsafe { stdc_strtod(c(s).as_ptr(), &mut used) };
                (got.to_bits() != v.to_bits() || used != *n).then(|| format!("{s:?} -> {got} ({used})"))
            })
            .collect();
        check_eq("strtod: values exact, ends where they end", &bad.join("; "), "", &mut failed);
        let e = unsafe { stdc_errno_after_strtod(c("1e400").as_ptr()) };
        check("strtod: overflow is ERANGE", e == libc::ERANGE, &mut failed);
    }

    // pthreads
    {
        let r = unsafe { stdc_threads() };
        check_eq(
            "pthreads: 4 x 10000 under a mutex, keys, destructors, a condvar",
            &r.to_string(),
            "40000",
            &mut failed,
        );
        let r = unsafe { stdc_sync_edges() };
        check_eq("pthreads: recursive mutex, timed wait times out", &r.to_string(), "0", &mut failed);
    }

    // The rest of the C library
    {
        let mut buf = [0 as c_char; 128];
        unsafe { stdc_time(buf.as_mut_ptr(), buf.len()) };
        let s = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned();
        check_eq("timegm, gmtime_r, strftime, time", &s, "951827696 2000-02-29 12:34:56 Tue 060 1", &mut failed);
        let r = unsafe { stdc_stdio(c("/tmp/stdc.txt").as_ptr()) };
        check_eq("stdio: fprintf to a file and fgets it back", &r.to_string(), "0", &mut failed);
        check("wctype: Greek, ideographic space, case", unsafe { stdc_wide() } == 1, &mut failed);
        check_eq("math.h from C", &unsafe { stdc_math() }.to_string(), "1028", &mut failed);
        check_eq("getauxval: the page size and the vDSO", &unsafe { stdc_auxv() }.to_string(), "0", &mut failed);
        let r = unsafe { stdc_locks(c("/tmp/stdc.lock").as_ptr()) };
        check_eq("fcntl record locks hold between two processes", &r.to_string(), "0", &mut failed);
    }

    // zstd
    {
        let data: Vec<u8> = (0..20000u32).flat_map(|i| (i % 251).to_le_bytes()).collect();
        let r = zstd::encode_all(&data[..], 3).and_then(|z| {
            let back = zstd::decode_all(&z[..])?;
            Ok((z.len(), back == data))
        });
        match r {
            Ok((n, same)) => check(
                &format!("zstd: {} bytes to {n} and back", data.len()),
                same && n < data.len() / 4,
                &mut failed,
            ),
            Err(e) => check(&format!("zstd: {e}"), false, &mut failed),
        }
    }

    // tree-sitter
    {
        let mut parser = tree_sitter::Parser::new();
        let lang: tree_sitter::Language = tree_sitter_json::LANGUAGE.into();
        let r = parser.set_language(&lang);
        let tree = r.ok().and_then(|_| parser.parse(r#"{"miku": [39, true, null], "os": "x"}"#, None));
        match tree {
            Some(t) => check_eq(
                "tree-sitter parses JSON through its grammar",
                &t.root_node().to_sexp(),
                "(document (object (pair key: (string (string_content)) value: (array (number) (true) (null))) \
                 (pair key: (string (string_content)) value: (string (string_content)))))",
                &mut failed,
            ),
            None => check("tree-sitter parses JSON through its grammar", false, &mut failed),
        }
    }

    // SQLite
    sqlite(&mut failed);

    println!("stdc: {}", if failed == 0 { "all passed" } else { "FAILURES" });
    std::process::exit(if failed == 0 { 0 } else { 1 });
}

/// SQLite through `libsqlite3-sys`, the crate Zed's `sqlez` sits on
fn sqlite(failed: &mut i32) {
    use libsqlite3_sys as ffi;
    use std::ptr;

    unsafe fn exec(db: *mut ffi::sqlite3, sql: &str) -> Result<(), String> {
        let sql = c(sql);
        let mut err: *mut c_char = ptr::null_mut();
        let rc = unsafe { ffi::sqlite3_exec(db, sql.as_ptr(), None, ptr::null_mut(), &mut err) };
        if rc == ffi::SQLITE_OK {
            Ok(())
        } else {
            // Extended code names the operation, and errno what the C library
            // returned
            let ext = unsafe { ffi::sqlite3_extended_errcode(db) };
            let sys = unsafe { ffi::sqlite3_system_errno(db) };
            let msg = if err.is_null() {
                format!("rc {rc}")
            } else {
                let m = unsafe { CStr::from_ptr(err) }.to_string_lossy().into_owned();
                unsafe { ffi::sqlite3_free(err as *mut _) };
                format!("rc {rc}: {m}")
            };
            Err(format!("{msg} (extended {ext}, errno {sys})"))
        }
    }

    unsafe fn query(db: *mut ffi::sqlite3, sql: &str) -> Result<Vec<String>, String> {
        let sql = c(sql);
        let mut st = ptr::null_mut();
        let rc = unsafe { ffi::sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut st, ptr::null_mut()) };
        if rc != ffi::SQLITE_OK {
            return Err(format!("prepare rc {rc}"));
        }
        let mut rows = Vec::new();
        loop {
            match unsafe { ffi::sqlite3_step(st) } {
                ffi::SQLITE_ROW => {
                    let n = unsafe { ffi::sqlite3_column_count(st) };
                    let cols: Vec<String> = (0..n)
                        .map(|i| {
                            let p = unsafe { ffi::sqlite3_column_text(st, i) };
                            if p.is_null() {
                                "NULL".into()
                            } else {
                                unsafe { CStr::from_ptr(p as *const c_char) }.to_string_lossy().into_owned()
                            }
                        })
                        .collect();
                    rows.push(cols.join(","));
                }
                ffi::SQLITE_DONE => break,
                rc => {
                    unsafe { ffi::sqlite3_finalize(st) };
                    return Err(format!("step rc {rc}"));
                }
            }
        }
        unsafe { ffi::sqlite3_finalize(st) };
        Ok(rows)
    }

    unsafe fn open(path: &str) -> Result<*mut ffi::sqlite3, String> {
        let p = c(path);
        let mut db = ptr::null_mut();
        let flags = ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE | ffi::SQLITE_OPEN_FULLMUTEX;
        let rc = unsafe { ffi::sqlite3_open_v2(p.as_ptr(), &mut db, flags, ptr::null()) };
        if rc == ffi::SQLITE_OK {
            Ok(db)
        } else {
            let m = unsafe { CStr::from_ptr(ffi::sqlite3_errmsg(db)) }.to_string_lossy().into_owned();
            Err(format!("open rc {rc}: {m}"))
        }
    }

    let version = unsafe { CStr::from_ptr(ffi::sqlite3_libversion()) }.to_string_lossy().into_owned();
    check(&format!("SQLite {version} is linked"), version.starts_with("3."), failed);

    // In memory: parser, VM, printf, floats
    let r = unsafe {
        open(":memory:").and_then(|db| {
            let r = exec(db, "CREATE TABLE t(a INTEGER, b REAL, c TEXT);
                              INSERT INTO t VALUES (1, 1.5, 'miku'), (2, 2.25, 'os'), (3, NULL, 'x');")
                .and_then(|_| query(db, "SELECT a, b, c, printf('%05.1f', b * 2) FROM t WHERE a < 3 ORDER BY a DESC"));
            ffi::sqlite3_close(db);
            r
        })
    };
    check_eq(
        "SQLite in memory: create, insert, select, printf()",
        &r.map(|v| v.join(";")).unwrap_or_else(|e| e),
        "2,2.25,os,004.5;1,1.5,miku,003.0",
        failed,
    );

    // On disk, then again from another thread through a second connection
    let path = "/tmp/stdc.db";
    let _ = std::fs::remove_file(path);
    let r = unsafe {
        open(path).and_then(|db| {
            let r = exec(db, "CREATE TABLE kv(k TEXT PRIMARY KEY, v INTEGER);
                              BEGIN; INSERT INTO kv VALUES ('a', 1); INSERT INTO kv VALUES ('b', 2); COMMIT;");
            ffi::sqlite3_close(db);
            r
        })
    };
    check_eq("SQLite on disk: a table and a transaction", &format!("{r:?}"), "Ok(())", failed);

    let r = std::thread::spawn(move || unsafe {
        open(path).and_then(|db| {
            let r = exec(db, "UPDATE kv SET v = v * 10").and_then(|_| query(db, "SELECT k, v FROM kv ORDER BY k"));
            ffi::sqlite3_close(db);
            r
        })
    })
    .join()
    .unwrap_or_else(|_| Err("thread panicked".into()));
    check_eq(
        "SQLite on disk: reopened from another thread",
        &r.map(|v| v.join(";")).unwrap_or_else(|e| e),
        "a,10;b,20",
        failed,
    );
    let _ = std::fs::remove_file(path);
}
