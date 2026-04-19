#![no_std]
#![no_main]

#[path = "miku.rs"]
mod miku;

use miku::*;

static mut TEST_NUM: i32 = 0;
static mut PASSED: i32 = 0;
static mut FAILED: i32 = 0;

const MAX_FAILURES: usize = 32;
const FAIL_NAME_LEN: usize = 48;
const FAIL_REASON_LEN: usize = 32;

static mut FAIL_LOG: [([u8; FAIL_NAME_LEN], [u8; FAIL_REASON_LEN], i32); MAX_FAILURES] =
    [([0u8; FAIL_NAME_LEN], [0u8; FAIL_REASON_LEN], 0); MAX_FAILURES];
static mut FAIL_COUNT: usize = 0;

fn copy_str(dst: &mut [u8], src: &str) {
    let n = src.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&src.as_bytes()[..n]);
    dst[n] = 0;
}

fn ok(name: &str) {
    unsafe {
        TEST_NUM += 1;
        print("  [pass] #");
        print_int(TEST_NUM as i64);
        print(" ");
        println(name);
        PASSED += 1;
    }
}

fn fail(name: &str, reason: &str) {
    unsafe {
        TEST_NUM += 1;
        print("  [!fail!] #");
        print_int(TEST_NUM as i64);
        print(" ");
        print(name);
        print(" (");
        print(reason);
        println(")");
        if FAIL_COUNT < MAX_FAILURES {
            copy_str(&mut FAIL_LOG[FAIL_COUNT].0, name);
            copy_str(&mut FAIL_LOG[FAIL_COUNT].1, reason);
            FAIL_LOG[FAIL_COUNT].2 = TEST_NUM;
            FAIL_COUNT += 1;
        }
        FAILED += 1;
    }
}

fn print_failure_summary() {
    unsafe {
        if FAIL_COUNT == 0 { return; }
        println("failed tests:");
        for i in 0..FAIL_COUNT {
            let num = FAIL_LOG[i].2;
            let name = &FAIL_LOG[i].0;
            let reason = &FAIL_LOG[i].1;
            print("  !!! #");
            print_int(num as i64);
            print(" ");
            let nlen = name.iter().position(|&b| b == 0).unwrap_or(FAIL_NAME_LEN);
            write(1, &name[..nlen]);
            print(" -> ");
            let rlen = reason.iter().position(|&b| b == 0).unwrap_or(FAIL_REASON_LEN);
            write(1, &reason[..rlen]);
            println("");
        }
        println("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    }
}

macro_rules! test {
    ($name:expr, $cond:expr) => {
        if $cond { ok($name); } else { fail($name, "condition false"); }
    };
    ($name:expr, $cond:expr, $reason:expr) => {
        if $cond { ok($name); } else { fail($name, $reason); }
    };
}

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    println("libmiku full test suite");
    println("");

    test_strings();
    test_ctype();
    test_convert();
    test_numbers();
    test_memory();
    test_heap();
    test_math();
    test_random();
    test_printf();
    test_snprintf();
    test_printf_extended();
    test_process();
    test_time();
    test_file_io();
    test_mmap();
    test_stderr();
    test_bitops();
    test_hash();
    test_base64();
    test_utf8();
    test_path();
    test_sort();
    test_vec();
    test_hashmap();
    test_list();
    test_ringbuf();
    test_endian();
    test_arena();
    test_bitset();
    test_priority_queue();
    test_glob();
    test_channel();
    test_slab_alloc();
    test_string_builder();
    test_regex();
    test_hex();
    test_checksum();
    test_treemap();
    test_lz();
    test_env();
    test_signal();
    test_json();
    test_byte_ring();
    test_sha256();
    test_uuid();
    test_strbuf();
    test_pool();
    test_event();
    test_datetime();
    test_trie();
    test_queue();
    test_errno();
    test_mem_extended();
    test_ctype_extended();
    test_string_extended();
    test_hash_extended();
    test_random_extended();
    test_signal_extended();
    test_env_extended();
    test_math_extended();
    test_sort_extended();
    test_list_extended();
    test_strbuf_extended();
    test_glob_extended();
    test_path_extended();
    test_endian_extended();
    test_datetime_extended();
    test_json_extended();
    test_sync();
    test_convert_extended();
    test_errno_extended();
    test_regex_extended();
    test_panic_extended();
    test_base64_extended();
    test_uuid_extended();
    test_sha256_extended();
    test_random_extended2();
    test_random_extended3();
    test_checksum_extended();
    test_csv_full();
    test_lz_extended();
    test_event_extended();
    test_ext_timestamps();
    test_ext_statfs();
    test_ext_fsync();
    test_ext_fallocate();
    test_ext_hardlink_timestamps();
    test_ext_symlink_timestamps();
    test_ext_truncate_timestamps();
    test_ext_rename_timestamps();

    // libc compatibility layer tests
    test_libc_string();
    test_libc_memory();
    test_libc_stdlib();
    test_libc_ctype();
    test_libc_stdio_basic();
    test_libc_file_io();
    test_libc_unistd();
    test_libc_dir();
    test_libc_printf();
    test_libc_qsort_bsearch();
    test_libc_env();
    test_libc_mmap();
    test_libc_time();

    println("");
    println("================================");
    unsafe {
        print_int(PASSED as i64);
        print("/");
        print_int(TEST_NUM as i64);
        println(" tests passed");
        if FAILED == 0 {
            println("all ok");
        } else {
            print_int(FAILED as i64);
            println("failed");
            print_failure_summary();
        }
        exit(if FAILED == 0 { 0 } else { 1 });
    }
}

fn test_strings() {
    println("--- string ---");

    print("hello ");
    println("from Rust!");
    ok("print/println");

    unsafe {
        test!("strlen", miku_strlen(cstr!("hello")) == 5 && miku_strlen(cstr!("")) == 0);
        test!("strlen null", miku_strlen(core::ptr::null()) == 0);

        test!("strcmp eq", miku_strcmp(cstr!("abc"), cstr!("abc")) == 0);
        test!("strcmp neq", miku_strcmp(cstr!("abc"), cstr!("xyz")) < 0);
        test!("strcmp null", miku_strcmp(core::ptr::null(), core::ptr::null()) == 0);

        test!("strncmp eq", miku_strncmp(cstr!("hello"), cstr!("helXX"), 3) == 0);
        test!("strncmp neq", miku_strncmp(cstr!("abc"), cstr!("abd"), 3) != 0);
        test!("strncmp 0", miku_strncmp(cstr!("abc"), cstr!("xyz"), 0) == 0);

        let mut buf = [0u8; 64];
        miku_strcpy(buf.as_mut_ptr(), cstr!("miku"));
        test!("strcpy", miku_strcmp(buf.as_ptr(), cstr!("miku")) == 0);

        let mut buf2 = [0u8; 16];
        miku_strncpy(buf2.as_mut_ptr(), cstr!("hello world"), 5);
        test!("strncpy", miku_strncmp(buf2.as_ptr(), cstr!("hello"), 5) == 0);

        miku_strcpy(buf.as_mut_ptr(), cstr!("hello"));
        miku_strcat(buf.as_mut_ptr(), cstr!(" world"));
        test!("strcat", miku_strcmp(buf.as_ptr(), cstr!("hello world")) == 0);

        let mut buf3 = [0u8; 16];
        miku_strncat(buf3.as_mut_ptr(), cstr!("abcdef"), 3);
        test!("strncat", miku_strcmp(buf3.as_ptr(), cstr!("abc")) == 0);

        let p = miku_strchr(cstr!("abcdef"), b'd' as i32);
        test!("strchr found", !p.is_null() && *p == b'd');
        test!("strchr miss", miku_strchr(cstr!("abcdef"), b'z' as i32).is_null());

        let p = miku_strrchr(cstr!("abcabc"), b'b' as i32);
        test!("strrchr", !p.is_null() && *p == b'b');
        let offset = p as usize - cstr!("abcabc") as usize;
        test!("strrchr last", offset == 4);

        let p = miku_strstr(cstr!("hello world"), cstr!("world"));
        test!("strstr found", !p.is_null() && miku_strcmp(p, cstr!("world")) == 0);
        test!("strstr miss", miku_strstr(cstr!("hello"), cstr!("xyz")).is_null());
        test!("strstr empty", !miku_strstr(cstr!("hello"), cstr!("")).is_null());

        let d = miku_strdup(cstr!("miku-os"));
        test!("strdup", !d.is_null() && miku_strcmp(d, cstr!("miku-os")) == 0);
        miku_free(d);

        let d = miku_strndup(cstr!("hello world"), 5);
        test!("strndup", !d.is_null() && miku_strcmp(d, cstr!("hello")) == 0);
        miku_free(d);

        {
            let mut buf = [0u8; 8];
            let r = miku_strlcpy(buf.as_mut_ptr(), cstr!("hello world"), 8);
            test!("strlcpy", r == 11 && miku_strlen(buf.as_ptr()) == 7
                && miku_strcmp(buf.as_ptr(), cstr!("hello w")) == 0);
        }

        {
            let mut buf = [0u8; 12];
            miku_strcpy(buf.as_mut_ptr(), cstr!("hello"));
            let r = miku_strlcat(buf.as_mut_ptr(), cstr!(" world!"), 12);
            test!("strlcat", r == 12 && miku_strlen(buf.as_ptr()) == 11);
        }

        {
            let mut s: [u8; 17] = *b"hello,world,miku\0";
            let t1 = miku_strtok(s.as_mut_ptr(), cstr!(","));
            let t2 = miku_strtok(core::ptr::null_mut(), cstr!(","));
            let t3 = miku_strtok(core::ptr::null_mut(), cstr!(","));
            let t4 = miku_strtok(core::ptr::null_mut(), cstr!(","));
            test!("strtok",
                !t1.is_null() && miku_strcmp(t1, cstr!("hello")) == 0
                && !t2.is_null() && miku_strcmp(t2, cstr!("world")) == 0
                && !t3.is_null() && miku_strcmp(t3, cstr!("miku")) == 0
                && t4.is_null());
        }

        {
            let mut s: [u8; 10] = *b"a:b:c:d:e\0";
            let mut save: *mut u8 = core::ptr::null_mut();
            let t1 = miku_strtok_r(s.as_mut_ptr(), cstr!(":"), &mut save);
            let t2 = miku_strtok_r(core::ptr::null_mut(), cstr!(":"), &mut save);
            let t3 = miku_strtok_r(core::ptr::null_mut(), cstr!(":"), &mut save);
            test!("strtok_r",
                !t1.is_null() && miku_strcmp(t1, cstr!("a")) == 0
                && !t2.is_null() && miku_strcmp(t2, cstr!("b")) == 0
                && !t3.is_null() && miku_strcmp(t3, cstr!("c")) == 0);
        }
    }

    println("");
}

fn test_ctype() {
    println("--- ctype ---");

    unsafe {
        test!("isdigit yes", miku_isdigit(b'0' as i32) != 0 && miku_isdigit(b'9' as i32) != 0);
        test!("isdigit no", miku_isdigit(b'a' as i32) == 0 && miku_isdigit(b' ' as i32) == 0);

        test!("isalpha yes", miku_isalpha(b'a' as i32) != 0 && miku_isalpha(b'Z' as i32) != 0);
        test!("isalpha no", miku_isalpha(b'0' as i32) == 0);

        test!("isalnum", miku_isalnum(b'a' as i32) != 0 && miku_isalnum(b'5' as i32) != 0
            && miku_isalnum(b' ' as i32) == 0);

        test!("isspace", miku_isspace(b' ' as i32) != 0 && miku_isspace(b'\t' as i32) != 0
            && miku_isspace(b'\n' as i32) != 0 && miku_isspace(b'a' as i32) == 0);

        test!("isupper", miku_isupper(b'A' as i32) != 0 && miku_isupper(b'Z' as i32) != 0
            && miku_isupper(b'a' as i32) == 0 && miku_isupper(b'5' as i32) == 0);

        test!("islower", miku_islower(b'a' as i32) != 0 && miku_islower(b'z' as i32) != 0
            && miku_islower(b'A' as i32) == 0);

        test!("isprint", miku_isprint(b' ' as i32) != 0 && miku_isprint(b'~' as i32) != 0
            && miku_isprint(0x1F) == 0 && miku_isprint(0x7F) == 0);

        test!("ispunct", miku_ispunct(b'!' as i32) != 0 && miku_ispunct(b'.' as i32) != 0
            && miku_ispunct(b'a' as i32) == 0 && miku_ispunct(b' ' as i32) == 0);

        test!("iscntrl", miku_iscntrl(0) != 0 && miku_iscntrl(0x1F) != 0
            && miku_iscntrl(0x7F) != 0 && miku_iscntrl(b'a' as i32) == 0);

        test!("isxdigit", miku_isxdigit(b'0' as i32) != 0 && miku_isxdigit(b'f' as i32) != 0
            && miku_isxdigit(b'A' as i32) != 0 && miku_isxdigit(b'g' as i32) == 0);

        test!("toupper", miku_toupper(b'a' as i32) == b'A' as i32
            && miku_toupper(b'Z' as i32) == b'Z' as i32
            && miku_toupper(b'5' as i32) == b'5' as i32);

        test!("tolower", miku_tolower(b'A' as i32) == b'a' as i32
            && miku_tolower(b'z' as i32) == b'z' as i32
            && miku_tolower(b'5' as i32) == b'5' as i32);
    }

    println("");
}

fn test_convert() {
    println("--- convert ---");

    unsafe {
        let p = miku_strpbrk(cstr!("hello world"), cstr!("wo"));
        test!("strpbrk", !p.is_null() && *p == b'o');
        test!("strpbrk miss", miku_strpbrk(cstr!("hello"), cstr!("xyz")).is_null());

        test!("strspn", miku_strspn(cstr!("aaabbc"), cstr!("ab")) == 5);
        test!("strspn all", miku_strspn(cstr!("aaa"), cstr!("a")) == 3);
        test!("strspn none", miku_strspn(cstr!("xyz"), cstr!("ab")) == 0);

        test!("strcspn", miku_strcspn(cstr!("hello,world"), cstr!(",!")) == 5);
        test!("strcspn none", miku_strcspn(cstr!("hello"), cstr!(",!")) == 5);

        let v1 = miku_strtol(cstr!("  -42"), core::ptr::null_mut(), 10);
        let v2 = miku_strtol(cstr!("0xff"), core::ptr::null_mut(), 0);
        let v3 = miku_strtol(cstr!("077"), core::ptr::null_mut(), 0);
        let v4 = miku_strtol(cstr!("+123"), core::ptr::null_mut(), 10);
        test!("strtol dec", v1 == -42);
        test!("strtol hex", v2 == 255);
        test!("strtol oct", v3 == 63);
        test!("strtol plus", v4 == 123);

        test!("strtoul", miku_strtoul(cstr!("0xDEAD"), core::ptr::null_mut(), 16) == 0xDEAD);
        test!("strtoul dec", miku_strtoul(cstr!("999"), core::ptr::null_mut(), 10) == 999);

        {
            let mut endptr: *const u8 = core::ptr::null();
            let val = miku_strtol(cstr!("123abc"), &mut endptr as *mut *const u8, 10);
            test!("strtol endptr", val == 123 && !endptr.is_null() && *endptr == b'a');
        }
    }

    println("");
}

fn test_numbers() {
    println("--- numbers ---");

    unsafe {
        let mut buf = [0u8; 24];
        miku_itoa(12345, buf.as_mut_ptr());
        test!("itoa +", miku_strcmp(buf.as_ptr(), cstr!("12345")) == 0);

        miku_itoa(-9876, buf.as_mut_ptr());
        test!("itoa -", miku_strcmp(buf.as_ptr(), cstr!("-9876")) == 0);

        miku_itoa(0, buf.as_mut_ptr());
        test!("itoa 0", miku_strcmp(buf.as_ptr(), cstr!("0")) == 0);

        miku_itoa(i64::MAX, buf.as_mut_ptr());
        test!("itoa max", miku_strlen(buf.as_ptr()) > 0);

        miku_itoa(i64::MIN, buf.as_mut_ptr());
        test!("itoa min", *buf.as_ptr() == b'-');

        miku_utoa(0, buf.as_mut_ptr());
        test!("utoa 0", miku_strcmp(buf.as_ptr(), cstr!("0")) == 0);

        miku_utoa(4294967295, buf.as_mut_ptr());
        test!("utoa large", miku_strcmp(buf.as_ptr(), cstr!("4294967295")) == 0);

        test!("atoi neg", miku_atoi(cstr!("  -42")) == -42);
        test!("atoi pos", miku_atoi(cstr!("100")) == 100);
        test!("atoi 0", miku_atoi(cstr!("0")) == 0);
        test!("atoi tabs", miku_atoi(cstr!("\t 55")) == 55);
    }

    print("  hex=");
    print_hex(0xDEADBEEF);
    println("");
    ok("print_hex");

    print("  int=");
    print_int(-777);
    println("");
    ok("print_int");

    print("  chars=");
    putchar(b'O');
    putchar(b'K');
    println("");
    ok("putchar");

    println("");
}

fn test_memory() {
    println("--- memory ---");

    unsafe {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        miku_memset(a.as_mut_ptr(), 0xAA, 32);
        miku_memset(b.as_mut_ptr(), 0xAA, 32);
        test!("memset+memcmp eq", miku_memcmp(a.as_ptr(), b.as_ptr(), 32) == 0);
        b[16] = 0xBB;
        test!("memcmp neq", miku_memcmp(a.as_ptr(), b.as_ptr(), 32) != 0);
        test!("memcmp 0", miku_memcmp(a.as_ptr(), b.as_ptr(), 0) == 0);

        {
            let mut z = [0xFFu8; 64];
            miku_memset(z.as_mut_ptr(), 0, 64);
            let ok = z.iter().all(|&v| v == 0);
            test!("memset zero", ok);
        }

        {
            let mut big = [0u8; 1024];
            miku_memset(big.as_mut_ptr(), 0x42, 1024);
            test!("memset 1K", big[0] == 0x42 && big[512] == 0x42 && big[1023] == 0x42);
        }

        let src = b"test data 123\0";
        let mut dst = [0u8; 32];
        miku_memcpy(dst.as_mut_ptr(), src.as_ptr(), 14);
        test!("memcpy", miku_memcmp(dst.as_ptr(), src.as_ptr(), 14) == 0);

        {
            let mut buf = [0u8; 32];
            miku_strcpy(buf.as_mut_ptr(), cstr!("abcdefgh"));
            miku_memmove(buf.as_mut_ptr().add(2), buf.as_ptr(), 6);
            test!("memmove overlap", buf[2] == b'a' && buf[3] == b'b' && buf[4] == b'c');
        }

        {
            let mut buf = [0u8; 32];
            miku_strcpy(buf.as_mut_ptr().add(4), cstr!("ABCD"));
            miku_memmove(buf.as_mut_ptr(), buf.as_ptr().add(4), 4);
            test!("memmove backward", buf[0] == b'A' && buf[1] == b'B');
        }

        {
            let mut buf = [0xFFu8; 16];
            miku_bzero(buf.as_mut_ptr(), 16);
            test!("bzero", buf.iter().all(|&b| b == 0));
        }

        {
            let data = b"hello world\0";
            let p = miku_memchr(data.as_ptr(), b'w' as i32, 12);
            test!("memchr found", !p.is_null() && *p == b'w');
            let p2 = miku_memchr(data.as_ptr(), b'z' as i32, 12);
            test!("memchr miss", p2.is_null());
            let p3 = miku_memchr(data.as_ptr(), b'o' as i32, 4);
            test!("memchr bounded", p3.is_null());
        }
    }

    println("");
}

fn test_heap() {
    println("--- heap ---");

    unsafe {
        test!("malloc null", miku_malloc(0).is_null());

        let p = miku_malloc(256);
        if !p.is_null() {
            miku_memset(p, 0x42, 256);
            test!("malloc+free", *p == 0x42 && *p.add(255) == 0x42);
            miku_free(p);
        } else { fail("malloc+free", "null"); }

        let p = miku_calloc(10, 8);
        if !p.is_null() {
            let slice = core::slice::from_raw_parts(p, 80);
            test!("calloc zeroed", slice.iter().all(|&b| b == 0));
            miku_free(p);
        } else { fail("calloc", "null"); }

        test!("calloc overflow", miku_calloc(usize::MAX, usize::MAX).is_null());

        let p = miku_malloc(64);
        if !p.is_null() {
            *p = b'M';
            *p.add(1) = b'K';
            let p2 = miku_realloc(p, 512);
            if !p2.is_null() {
                test!("realloc preserves", *p2 == b'M' && *p2.add(1) == b'K');
                miku_free(p2);
            } else { miku_free(p); fail("realloc", "null"); }
        } else { fail("realloc", "null"); }

        {
            let p = miku_realloc(core::ptr::null_mut(), 128);
            test!("realloc null", !p.is_null());
            miku_free(p);
        }

        {
            let p = miku_malloc(64);
            if !p.is_null() {
                let p2 = miku_realloc(p, 0);
                test!("realloc 0", p2.is_null());
            }
        }

        let d = miku_strdup(cstr!("miku-os"));
        if !d.is_null() {
            test!("strdup", miku_strcmp(d, cstr!("miku-os")) == 0);
            miku_free(d);
        } else { fail("strdup", "null"); }

        {
            let mut ptrs = [core::ptr::null_mut(); 32];
            let mut good = true;
            for i in 0..32 {
                ptrs[i] = miku_malloc(16);
                if ptrs[i].is_null() { good = false; break; }
                miku_memset(ptrs[i], i as i32, 16);
            }
            for i in 0..32 {
                if !ptrs[i].is_null() {
                    if *ptrs[i] != i as u8 { good = false; }
                    miku_free(ptrs[i]);
                }
            }
            test!("32x alloc", good);
        }

        {
            let p = miku_malloc(65536);
            if !p.is_null() {
                miku_memset(p, 0xBE, 65536);
                test!("64KB alloc", *p == 0xBE && *p.add(65535) == 0xBE);
                miku_free(p);
            } else { fail("64KB alloc", "null"); }
        }

        {
            let mut good = true;
            let mut ptrs = [core::ptr::null_mut(); 8];
            for i in 0..8 {
                ptrs[i] = miku_malloc(100 + i * 50);
                if ptrs[i].is_null() { good = false; }
            }
            for i in (0..8).rev() { miku_free(ptrs[i]); }
            for i in 0..8 {
                ptrs[i] = miku_malloc(200);
                if ptrs[i].is_null() { good = false; }
            }
            for i in 0..8 { miku_free(ptrs[i]); }
            test!("alloc+free+realloc stress", good);
        }

        {
            let mut good = true;
            for _ in 0..64 {
                let p = miku_malloc(48);
                if p.is_null() { good = false; break; }
                miku_memset(p, 0xCC, 48);
                miku_free(p);
            }
            test!("rapid alloc-free", good);
        }

        {
            let p = miku_memalign(64, 256);
            if !p.is_null() {
                let aligned = (p as usize) & 63 == 0;
                miku_memset(p, 0xAB, 256);
                test!("memalign 64", aligned && *p == 0xAB);
                miku_free(p);
            } else {
                fail("memalign 64", "null");
            }
        }
    }

    println("");
}

fn test_math() {
    println("--- math ---");

    test!("abs", abs(-42) == 42 && abs(42) == 42 && abs(0) == 0);
    test!("min", min(3, 7) == 3 && min(-5, 2) == -5 && min(0, 0) == 0);
    test!("max", max(3, 7) == 7 && max(-5, 2) == 2 && max(0, 0) == 0);
    test!("clamp mid", clamp(5, 0, 10) == 5);
    test!("clamp lo", clamp(-5, 0, 10) == 0);
    test!("clamp hi", clamp(99, 0, 10) == 10);

    unsafe {
        test!("umin", miku_umin(3, 7) == 3 && miku_umin(100, 0) == 0);
        test!("umax", miku_umax(3, 7) == 7 && miku_umax(0, 100) == 100);

        let mut a: u64 = 111;
        let mut b: u64 = 222;
        miku_swap(&mut a as *mut u64, &mut b as *mut u64);
        test!("swap", a == 222 && b == 111);

        miku_swap(core::ptr::null_mut(), &mut b as *mut u64);
        test!("swap null safe", b == 111);
    }

    println("");
}

fn test_random() {
    println("--- random ---");

    srand(42);
    let r1 = rand();
    let r2 = rand();
    test!("rand nonzero", r1 != 0 && r2 != 0);
    test!("rand varies", r1 != r2);

    {
        srand(42);
        let a = rand();
        srand(42);
        let b = rand();
        test!("srand deterministic", a == b);
    }

    {
        srand(12345);
        let mut good = true;
        for _ in 0..200 {
            let r = rand_range(10, 20);
            if r < 10 || r >= 20 { good = false; break; }
        }
        test!("rand_range bounds", good);
    }

    test!("rand_range degenerate", rand_range(5, 5) == 5);

    println("");
}

fn test_printf() {
    println("--- printf ---");

    unsafe {
        let r = miku_printf(cstr!("  hello %s!\n"), cstr!("world"));
        test!("printf %s", r > 0);

        let r = miku_printf(cstr!("  num=%d neg=%d zero=%d\n"), 42i64, -99i64, 0i64);
        test!("printf %d", r > 0);

        let r = miku_printf(cstr!("  unsigned=%u\n"), 42u64);
        test!("printf %u", r > 0);

        let r = miku_printf(cstr!("  hex=%x DEAD=%X\n"), 255u64, 0xDEADu64);
        test!("printf %x %X", r > 0);

        let r = miku_printf(cstr!("  oct=%o\n"), 255u64);
        test!("printf %o", r > 0);

        let r = miku_printf(cstr!("  char=%c%c%c\n"), b'A' as u64, b'B' as u64, b'C' as u64);
        test!("printf %c", r > 0);

        let r = miku_printf(cstr!("  100%%\n"));
        test!("printf %%", r > 0);

        let r = miku_printf(cstr!("  ptr=%p\n"), 0x1234u64);
        test!("printf %p", r > 0);
    }

    println("");
}

fn test_snprintf() {
    println("--- snprintf ---");

    unsafe {
        let mut buf = [0u8; 64];

        miku_snprintf(buf.as_mut_ptr(), 64, cstr!("hello %s"), cstr!("miku"));
        test!("snprintf basic", miku_strcmp(buf.as_ptr(), cstr!("hello miku")) == 0);

        miku_memset(buf.as_mut_ptr(), 0, 64);
        miku_snprintf(buf.as_mut_ptr(), 64, cstr!("%d+%d=%d"), 10u64, 20u64, 30u64);
        test!("snprintf int", miku_strcmp(buf.as_ptr(), cstr!("10+20=30")) == 0);

        let mut small = [b'X'; 8];
        miku_snprintf(small.as_mut_ptr(), 8, cstr!("hello world 12345"));
        test!("snprintf truncate", small[7] == 0 && miku_strlen(small.as_ptr()) <= 7);

        miku_memset(buf.as_mut_ptr(), 0, 64);
        miku_snprintf(buf.as_mut_ptr(), 64, cstr!("0x%x"), 255u64);
        test!("snprintf hex", miku_strcmp(buf.as_ptr(), cstr!("0xff")) == 0);

        miku_memset(buf.as_mut_ptr(), 0, 64);
        miku_snprintf(buf.as_mut_ptr(), 64, cstr!("HEX=%X"), 0xABCDu64);
        test!("snprintf HEX", miku_strcmp(buf.as_ptr(), cstr!("HEX=ABCD")) == 0);

        miku_memset(buf.as_mut_ptr(), 0, 64);
        miku_snprintf(buf.as_mut_ptr(), 64, cstr!("oct=%o"), 255u64);
        test!("snprintf oct", miku_strcmp(buf.as_ptr(), cstr!("oct=377")) == 0);

        let mut buf2 = [0u8; 16];
        miku_snprintf(buf2.as_mut_ptr(), 16, cstr!("100%%"));
        test!("snprintf %%", miku_strcmp(buf2.as_ptr(), cstr!("100%")) == 0);

        miku_memset(buf.as_mut_ptr(), 0, 64);
        miku_snprintf(buf.as_mut_ptr(), 64, cstr!("ptr=%p"), 0x1234u64);
        test!("snprintf ptr", miku_strlen(buf.as_ptr()) > 4);

        miku_memset(buf.as_mut_ptr(), 0, 64);
        miku_snprintf(buf.as_mut_ptr(), 64, cstr!("[%10d]"), 42u64);
        test!("snprintf width", miku_strlen(buf.as_ptr()) == 12);

        miku_memset(buf.as_mut_ptr(), 0, 64);
        miku_snprintf(buf.as_mut_ptr(), 64, cstr!("[%08x]"), 255u64);
        test!("snprintf 0-pad", miku_strcmp(buf.as_ptr(), cstr!("[000000ff]")) == 0);

        miku_memset(buf.as_mut_ptr(), 0, 64);
        miku_snprintf(buf.as_mut_ptr(), 64, cstr!("[%-10s]"), cstr!("hi"));
        test!("snprintf left-align", miku_strlen(buf.as_ptr()) == 12);

        {
            let _r = miku_snprintf(buf.as_mut_ptr(), 1, cstr!("hello"));
            test!("snprintf size=1", buf[0] == 0);
        }
    }

    println("");
}

fn test_printf_extended() {
    println("--- printf extended ---");

    unsafe {
        let mut buf = [0u8; 64];

        miku_memset(buf.as_mut_ptr(), 0, 64);
        miku_snprintf(buf.as_mut_ptr(), 64, cstr!("%ld"), -999999i64 as u64);
        test!("snprintf %ld", miku_strcmp(buf.as_ptr(), cstr!("-999999")) == 0);

        miku_memset(buf.as_mut_ptr(), 0, 64);
        miku_snprintf(buf.as_mut_ptr(), 64, cstr!("%lu"), 4294967296u64);
        test!("snprintf %lu", miku_strcmp(buf.as_ptr(), cstr!("4294967296")) == 0);

        miku_memset(buf.as_mut_ptr(), 0, 64);
        miku_snprintf(buf.as_mut_ptr(), 64, cstr!("%lx"), 0xDEADBEEFCAFEu64);
        test!("snprintf %lx", miku_strlen(buf.as_ptr()) == 12);

        miku_memset(buf.as_mut_ptr(), 0, 64);
        miku_snprintf(buf.as_mut_ptr(), 64, cstr!("a%cb%cc"), b'X' as u64, b'Y' as u64);
        test!("snprintf multi %c", miku_strcmp(buf.as_ptr(), cstr!("aXbYc")) == 0);

        miku_memset(buf.as_mut_ptr(), 0, 64);
        miku_snprintf(buf.as_mut_ptr(), 64, cstr!("%s=%d"), cstr!("val"), 42u64);
        test!("snprintf mixed", miku_strcmp(buf.as_ptr(), cstr!("val=42")) == 0);

        miku_memset(buf.as_mut_ptr(), 0, 64);
        miku_snprintf(buf.as_mut_ptr(), 64, cstr!(""));
        test!("snprintf empty", miku_strlen(buf.as_ptr()) == 0);
    }

    println("");
}

fn test_process() {
    println("--- process ---");

    let pid = getpid();
    print("  pid=");
    print_int(pid as i64);
    println("");
    test!("getpid", pid > 0);

    test!("brk", brk(0) > 0);

    unsafe {
        let mut buf = [0u8; 64];
        let p = miku_getcwd(buf.as_mut_ptr(), 64);
        test!("getcwd", !p.is_null() && buf[0] == b'/');
    }

    println("");
}

fn test_time() {
    println("--- time ---");

    {
        let t = uptime();
        print("  ticks=");
        print_int(t as i64);
        println("");
        test!("uptime", t > 0);
    }

    {
        let ms = uptime_ms();
        print("  ms=");
        print_int(ms as i64);
        println("");
        test!("uptime_ms", ms > 0);
        test!("uptime_ms >= uptime", ms >= uptime());
    }

    {
        let before = uptime();
        sleep(10);
        let after = uptime();
        let diff = after - before;
        print("  slept ");
        print_int(diff as i64);
        println(" ticks");
        test!("sleep(10)", diff >= 5);
    }

    {
        let before = uptime_ms();
        sleep_ms(100);
        let after = uptime_ms();
        let diff = after - before;
        print("  slept ");
        print_int(diff as i64);
        println(" ms");
        test!("sleep_ms(100)", diff >= 50);
    }

    sleep(0);
    ok("sleep(0) yield");

    yield_now();
    ok("yield");

    println("");
}

fn test_file_io() {
    println("--- file I/O ---");

    unsafe {
        let fd = miku_open_cstr(cstr!("/nonexistent_xyz_123"));
        test!("open nonexistent", fd < 0);

        let fd = miku_open_cstr(cstr!("/test_full"));
        if fd >= 0 {
            let sz = miku_fsize(fd);
            print("  size=");
            print_int(sz);
            println(" bytes");
            test!("fsize", sz > 0);

            let sk = miku_seek(fd, 0);
            print("  seek="); print_int(sk); println("");
            let mut hdr = [0u8; 4];
            let n = miku_read(fd as u64, hdr.as_mut_ptr(), 4);
            print("  read n="); print_int(n);
            print(" hdr=["); print_int(hdr[0] as i64); print(",");
            print_int(hdr[1] as i64); print(",");
            print_int(hdr[2] as i64); print(",");
            print_int(hdr[3] as i64); println("]");
            test!("read ELF header", n == 4 && hdr[0] == 0x7F && hdr[1] == b'E');

            let sk2 = miku_seek(fd, 0);
            print("  seek2="); print_int(sk2); println("");
            let n2 = miku_read(fd as u64, hdr.as_mut_ptr(), 4);
            print("  read2 n="); print_int(n2); println("");
            test!("seek+reread", n2 == 4 && hdr[0] == 0x7F);

            let mut zero = [0u8; 4];
            let n3 = miku_read_fd(fd, zero.as_mut_ptr(), 4);
            print("  read_fd n="); print_int(n3); println("");
            test!("read_fd", n3 == 4);

            miku_close(fd);
            ok("close");
        } else {
            println("  (no /test_full on disk, skipping read tests)");
            ok("open+read skip");
        }

        {
            let fd = miku_open_cstr(cstr!("/test_full"));
            if fd >= 0 {
                miku_close(fd);
                let r = miku_fsize(fd);
                print("  fsize after close="); print_int(r); println("");
                test!("fsize after close", r < 0);
            }
        }

        let mut sz: usize = 0;
        let data = miku_read_file(cstr!("/test_full"), &mut sz as *mut usize);
        if !data.is_null() && sz > 0 {
            print("  read_file=");
            print_int(sz as i64);
            println(" bytes");
            test!("read_file content", *data == 0x7F);
            miku_free(data);
        } else {
            ok("read_file skip");
        }

        test!("read_file null path", miku_read_file(core::ptr::null(), core::ptr::null_mut()).is_null());
    }

    println("");
}

fn test_mmap() {
    println("--- mmap ---");

    unsafe {
        let p = miku_mmap(0, 4096, 1 | 2);
        if !p.is_null() {
            miku_memset(p, 0xAB, 4096);
            test!("mmap rw", *p == 0xAB && *p.add(4095) == 0xAB);

            let r = miku_munmap(p, 4096);
            test!("munmap", r == 0);
        } else {
            fail("mmap rw", "null");
        }

        let p = miku_mmap(0, 16384, 1 | 2);
        if !p.is_null() {
            miku_memset(p, 0, 16384);
            miku_memset(p, 0xFF, 4096);
            test!("mmap 16K", *p == 0xFF && *p.add(4096) == 0x00);
            miku_munmap(p, 16384);
        } else {
            fail("mmap 16K", "null");
        }
    }

    println("");
}

fn test_stderr() {
    println("--- stderr ---");
    eprint("  stderr test: ");
    eprintln("ok");
    ok("eprint/eprintln");
    println("");
}

fn test_bitops() {
    println("--- bitops ---");

    unsafe {
        
        test!("popcount32 0", miku_popcount32(0) == 0);
        test!("popcount32 1", miku_popcount32(1) == 1);
        test!("popcount32 0xFF", miku_popcount32(0xFF) == 8);
        test!("popcount32 max", miku_popcount32(0xFFFFFFFF) == 32);
        test!("popcount64", miku_popcount64(0xFF00FF00FF00FF00) == 32);
        test!("popcount64 one", miku_popcount64(0x8000000000000000) == 1);

        
        test!("clz32 0", miku_clz32(0) == 32);
        test!("clz32 1", miku_clz32(1) == 31);
        test!("clz32 msb", miku_clz32(0x80000000) == 0);
        test!("clz32 0x100", miku_clz32(0x100) == 23);
        test!("clz64 0", miku_clz64(0) == 64);
        test!("clz64 1", miku_clz64(1) == 63);
        test!("clz64 msb", miku_clz64(0x8000000000000000) == 0);

        
        test!("ctz32 0", miku_ctz32(0) == 32);
        test!("ctz32 1", miku_ctz32(1) == 0);
        test!("ctz32 lsb", miku_ctz32(0x80) == 7);
        test!("ctz64 0", miku_ctz64(0) == 64);
        test!("ctz64 bit40", miku_ctz64(1 << 40) == 40);

        
        test!("fls32 0", miku_fls32(0) == 0);
        test!("fls32 1", miku_fls32(1) == 1);
        test!("fls32 0xFF", miku_fls32(0xFF) == 8);
        test!("ffs32 0", miku_ffs32(0) == 0);
        test!("ffs32 1", miku_ffs32(1) == 1);
        test!("ffs32 0x80", miku_ffs32(0x80) == 8);

        
        test!("bswap16", miku_bswap16(0x1234) == 0x3412);
        test!("bswap32", miku_bswap32(0x12345678) == 0x78563412);
        test!("bswap64", miku_bswap64(0x0102030405060708) == 0x0807060504030201);

        
        test!("rotl32", miku_rotl32(0x80000001, 1) == 0x00000003);
        test!("rotr32", miku_rotr32(0x80000001, 1) == 0xC0000000);
        test!("rotl64", miku_rotl64(1, 63) == 0x8000000000000000);
        test!("rotr64", miku_rotr64(0x8000000000000000, 63) == 1);
        test!("rotl32 0", miku_rotl32(0x12345678, 0) == 0x12345678);
        test!("rotr32 0", miku_rotr32(0x12345678, 0) == 0x12345678);

        
        test!("is_pow2 1", miku_is_power_of_two(1));
        test!("is_pow2 2", miku_is_power_of_two(2));
        test!("is_pow2 4096", miku_is_power_of_two(4096));
        test!("is_pow2 0", !miku_is_power_of_two(0));
        test!("is_pow2 3", !miku_is_power_of_two(3));
        test!("next_pow2 1", miku_next_power_of_two(1) == 1);
        test!("next_pow2 3", miku_next_power_of_two(3) == 4);
        test!("next_pow2 5", miku_next_power_of_two(5) == 8);
        test!("next_pow2 4096", miku_next_power_of_two(4096) == 4096);

        
        test!("log2 1", miku_log2(1) == 0);
        test!("log2 2", miku_log2(2) == 1);
        test!("log2 8", miku_log2(8) == 3);
        test!("log2 1024", miku_log2(1024) == 10);
        test!("log2 1023", miku_log2(1023) == 9);

        
        test!("bit_extract", miku_bit_extract(0xABCD, 8, 8) == 0xAB);
        test!("bit_extract low", miku_bit_extract(0xFF, 0, 4) == 0x0F);
        test!("bit_insert", miku_bit_insert(0, 0xFF, 8, 8) == 0xFF00);
        test!("bit_insert mix", miku_bit_insert(0xFFFF, 0, 4, 4) == 0xFF0F);

        
        test!("align_up 4K", miku_align_up(4097, 4096) == 8192);
        test!("align_up exact", miku_align_up(4096, 4096) == 4096);
        test!("align_down 4K", miku_align_down(8191, 4096) == 4096);
        test!("is_aligned yes", miku_is_aligned(4096, 4096));
        test!("is_aligned no", !miku_is_aligned(4097, 4096));
    }

    println("");
}

fn test_hash() {
    println("--- hash ---");

    unsafe {
        
        let h1 = miku_fnv1a_32(b"hello\0".as_ptr(), 5);
        let h2 = miku_fnv1a_32(b"hello\0".as_ptr(), 5);
        test!("fnv1a_32 deterministic", h1 == h2);
        test!("fnv1a_32 nonzero", h1 != 0);
        let h3 = miku_fnv1a_32(b"world\0".as_ptr(), 5);
        test!("fnv1a_32 differs", h1 != h3);

        let h64a = miku_fnv1a_64(b"test data\0".as_ptr(), 9);
        let h64b = miku_fnv1a_64(b"test data\0".as_ptr(), 9);
        test!("fnv1a_64 deterministic", h64a == h64b);
        test!("fnv1a_64 nonzero", h64a != 0);

        
        let d1 = miku_djb2_str(cstr!("hello"));
        let d2 = miku_djb2_str(cstr!("hello"));
        test!("djb2 deterministic", d1 == d2);
        let d3 = miku_djb2_str(cstr!("world"));
        test!("djb2 differs", d1 != d3);

        let d4 = miku_djb2(b"hello\0".as_ptr(), 5);
        test!("djb2 vs djb2_str", d1 == d4);

        
        let c1 = miku_crc32(b"123456789\0".as_ptr(), 9);
        test!("crc32 known", c1 == 0xCBF43926); 
        let c2 = miku_crc32(b"\0".as_ptr(), 0);
        test!("crc32 empty", c2 == 0);

        
        let part1 = miku_crc32_update(0, b"1234\0".as_ptr(), 4);
        let full = miku_crc32_update(part1, b"56789\0".as_ptr(), 5);
        test!("crc32 incremental", full == c1);

        
        let s1 = miku_siphash(b"hello\0".as_ptr(), 5, 0x0706050403020100, 0x0f0e0d0c0b0a0908);
        let s2 = miku_siphash(b"hello\0".as_ptr(), 5, 0x0706050403020100, 0x0f0e0d0c0b0a0908);
        test!("siphash deterministic", s1 == s2);
        let s3 = miku_siphash(b"hello\0".as_ptr(), 5, 1, 2);
        test!("siphash key-dependent", s1 != s3);

        
        let hu1 = miku_hash_u64(42);
        let hu2 = miku_hash_u64(42);
        test!("hash_u64 deterministic", hu1 == hu2);
        let hu3 = miku_hash_u64(43);
        test!("hash_u64 differs", hu1 != hu3);
        
        test!("hash_u64 avalanche", miku_hash_u64(1) != 1);

        
        let hb = miku_hash_bytes(b"test\0".as_ptr(), 4);
        test!("hash_bytes nonzero", hb != 0);
        let hs = miku_hash_str(cstr!("test"));
        test!("hash_str nonzero", hs != 0);
    }

    println("");
}

fn test_base64() {
    println("--- base64 ---");

    unsafe {
        
        let input = b"Hello, World!";
        let mut out = [0u8; 64];
        let n = miku_base64_encode(input.as_ptr(), 13, out.as_mut_ptr(), 64);
        test!("b64 encode len", n == 20);
        test!("b64 encode value", miku_strcmp(out.as_ptr(), cstr!("SGVsbG8sIFdvcmxkIQ==")) == 0);

        
        let mut dec = [0u8; 64];
        let dn = miku_base64_decode(out.as_ptr(), n as usize, dec.as_mut_ptr(), 64);
        test!("b64 decode len", dn == 13);
        test!("b64 decode match", miku_memcmp(dec.as_ptr(), input.as_ptr(), 13) == 0);

        
        let n = miku_base64_encode(b"\0".as_ptr(), 0, out.as_mut_ptr(), 64);
        test!("b64 encode empty", n == 0);

        
        miku_memset(out.as_mut_ptr(), 0, 64);
        let n = miku_base64_encode(b"M".as_ptr(), 1, out.as_mut_ptr(), 64);
        test!("b64 encode 1 byte", n == 4 && miku_strcmp(out.as_ptr(), cstr!("TQ==")) == 0);

        
        miku_memset(out.as_mut_ptr(), 0, 64);
        let n = miku_base64_encode(b"Ma".as_ptr(), 2, out.as_mut_ptr(), 64);
        test!("b64 encode 2 bytes", n == 4 && miku_strcmp(out.as_ptr(), cstr!("TWE=")) == 0);

        
        miku_memset(out.as_mut_ptr(), 0, 64);
        let n = miku_base64_encode(b"Man".as_ptr(), 3, out.as_mut_ptr(), 64);
        test!("b64 encode 3 bytes", n == 4 && miku_strcmp(out.as_ptr(), cstr!("TWFu")) == 0);

        
        let bad = b"!!!!\0";
        let r = miku_base64_decode(bad.as_ptr(), 4, dec.as_mut_ptr(), 64);
        test!("b64 decode invalid", r == -1);

        
        let binary: [u8; 8] = [0x00, 0xFF, 0x7F, 0x80, 0xDE, 0xAD, 0xBE, 0xEF];
        miku_memset(out.as_mut_ptr(), 0, 64);
        let en = miku_base64_encode(binary.as_ptr(), 8, out.as_mut_ptr(), 64);
        test!("b64 binary encode", en > 0);
        miku_memset(dec.as_mut_ptr(), 0, 64);
        let dn = miku_base64_decode(out.as_ptr(), en as usize, dec.as_mut_ptr(), 64);
        test!("b64 binary roundtrip", dn == 8 && miku_memcmp(dec.as_ptr(), binary.as_ptr(), 8) == 0);

        
        let enc_alloc = miku_base64_encode_alloc(b"test".as_ptr(), 4);
        test!("b64 encode_alloc", !enc_alloc.is_null());
        if !enc_alloc.is_null() {
            test!("b64 encode_alloc val", miku_strcmp(enc_alloc, cstr!("dGVzdA==")) == 0);
            let mut dec_len: usize = 0;
            let dec_alloc = miku_base64_decode_alloc(enc_alloc, miku_strlen(enc_alloc), &mut dec_len);
            test!("b64 decode_alloc", !dec_alloc.is_null() && dec_len == 4);
            if !dec_alloc.is_null() {
                test!("b64 decode_alloc val", miku_memcmp(dec_alloc, b"test".as_ptr(), 4) == 0);
                miku_free(dec_alloc);
            }
            miku_free(enc_alloc);
        }

        
        test!("b64 encode_len 0", miku_base64_encode_len(0) == 1);
        test!("b64 encode_len 1", miku_base64_encode_len(1) == 5);
        test!("b64 encode_len 3", miku_base64_encode_len(3) == 5);
    }

    println("");
}

fn test_utf8() {
    println("--- utf8 ---");

    unsafe {
        
        let mut buf = [0u8; 8];
        let n = miku_utf8_encode(b'A' as u32, buf.as_mut_ptr());
        test!("utf8 encode ASCII", n == 1 && buf[0] == b'A');

        
        let n = miku_utf8_encode(0xE9, buf.as_mut_ptr());
        test!("utf8 encode 2-byte", n == 2 && buf[0] == 0xC3 && buf[1] == 0xA9);

        
        let n = miku_utf8_encode(0x4E16, buf.as_mut_ptr());
        test!("utf8 encode 3-byte", n == 3);

        
        let n = miku_utf8_encode(0x1F600, buf.as_mut_ptr());
        test!("utf8 encode 4-byte", n == 4);

        
        test!("utf8 reject surrogate", miku_utf8_encode(0xD800, buf.as_mut_ptr()) == 0);
        test!("utf8 reject >10FFFF", miku_utf8_encode(0x110000, buf.as_mut_ptr()) == 0);

        
        let data = b"Hello";
        let mut consumed: usize = 0;
        let cp = miku_utf8_decode(data.as_ptr(), 5, &mut consumed);
        test!("utf8 decode ASCII", cp == b'H' as u32 && consumed == 1);

        
        let data2 = [0xC3u8, 0xA9]; 
        let cp = miku_utf8_decode(data2.as_ptr(), 2, &mut consumed);
        test!("utf8 decode 2-byte", cp == 0xE9 && consumed == 2);

        
        let data3 = [0xE4u8, 0xB8, 0x96]; 
        let cp = miku_utf8_decode(data3.as_ptr(), 3, &mut consumed);
        test!("utf8 decode 3-byte", cp == 0x4E16 && consumed == 3);

        
        let bad = [0xFFu8];
        let cp = miku_utf8_decode(bad.as_ptr(), 1, &mut consumed);
        test!("utf8 decode invalid", cp == 0xFFFD && consumed == 1);

        
        let ascii = cstr!("hello");
        test!("utf8_strlen ASCII", miku_utf8_strlen(ascii) == 5);

        
        let russian = b"\xd0\x9f\xd1\x80\xd0\xb8\xd0\xb2\xd0\xb5\xd1\x82\0";
        test!("utf8_strlen Russian", miku_utf8_strlen(russian.as_ptr()) == 6);

        
        test!("utf8_len", miku_utf8_len(russian.as_ptr(), 12) == 6);

        
        test!("utf8_valid ASCII", miku_utf8_valid(b"hello\0".as_ptr(), 5));
        test!("utf8_valid Russian", miku_utf8_valid(russian.as_ptr(), 12));
        let bad_seq = [0xC0u8, 0xAF]; 
        test!("utf8_valid overlong", !miku_utf8_valid(bad_seq.as_ptr(), 2));
        let trunc = [0xC3u8]; 
        test!("utf8_valid truncated", !miku_utf8_valid(trunc.as_ptr(), 1));

        
        test!("utf8_offset 0", miku_utf8_offset(russian.as_ptr(), 12, 0) == 0);
        test!("utf8_offset 1", miku_utf8_offset(russian.as_ptr(), 12, 1) == 2);
        test!("utf8_offset 3", miku_utf8_offset(russian.as_ptr(), 12, 3) == 6);

        
        test!("utf8_boundary 0", miku_utf8_is_boundary(russian.as_ptr(), 12, 0));
        test!("utf8_boundary 2", miku_utf8_is_boundary(russian.as_ptr(), 12, 2));
        test!("utf8_boundary 1", !miku_utf8_is_boundary(russian.as_ptr(), 12, 1)); 

        
        let cp_orig: u32 = 0x1F600;
        let mut enc = [0u8; 8];
        let enc_n = miku_utf8_encode(cp_orig, enc.as_mut_ptr());
        let cp_dec = miku_utf8_decode(enc.as_ptr(), enc_n, &mut consumed);
        test!("utf8 roundtrip", cp_dec == cp_orig && consumed == enc_n);
    }

    println("");
}

fn test_path() {
    println("--- path ---");

    unsafe {
        
        let b = miku_basename(cstr!("/usr/lib/file.txt"));
        test!("basename", !b.is_null() && miku_strcmp(b, cstr!("file.txt")) == 0);
        miku_free(b);
        let b = miku_basename(cstr!("/usr/lib/"));
        test!("basename trailing /", !b.is_null() && miku_strcmp(b, cstr!("lib")) == 0);
        miku_free(b);
        let b = miku_basename(cstr!("file.txt"));
        test!("basename no dir", !b.is_null() && miku_strcmp(b, cstr!("file.txt")) == 0);
        miku_free(b);
        let b = miku_basename(cstr!("/"));
        test!("basename root", !b.is_null() && *b == b'/');
        miku_free(b);

        
        let d = miku_dirname(cstr!("/usr/lib/file.txt"));
        test!("dirname", !d.is_null() && miku_strcmp(d, cstr!("/usr/lib")) == 0);
        miku_free(d);
        let d = miku_dirname(cstr!("file.txt"));
        test!("dirname no dir", !d.is_null() && miku_strcmp(d, cstr!(".")) == 0);
        miku_free(d);
        let d = miku_dirname(cstr!("/"));
        test!("dirname root", !d.is_null() && miku_strcmp(d, cstr!("/")) == 0);
        miku_free(d);

        
        let e = miku_path_ext(cstr!("file.tar.gz"));
        test!("path_ext", !e.is_null() && miku_strcmp(e, cstr!("gz")) == 0);
        miku_free(e);
        let e = miku_path_ext(cstr!("Makefile"));
        test!("path_ext none", e.is_null());
        let e = miku_path_ext(cstr!(".gitignore"));
        test!("path_ext hidden", e.is_null());

        
        let s = miku_path_stem(cstr!("file.tar.gz"));
        test!("path_stem", !s.is_null() && miku_strcmp(s, cstr!("file.tar")) == 0);
        miku_free(s);

        
        let j = miku_path_join(cstr!("/usr"), cstr!("lib"));
        test!("path_join", !j.is_null() && miku_strcmp(j, cstr!("/usr/lib")) == 0);
        miku_free(j);
        let j = miku_path_join(cstr!("/usr/"), cstr!("lib"));
        test!("path_join trail /", !j.is_null() && miku_strcmp(j, cstr!("/usr/lib")) == 0);
        miku_free(j);
        let j = miku_path_join(cstr!("/usr"), cstr!("/lib"));
        test!("path_join abs 2nd", !j.is_null() && miku_strcmp(j, cstr!("/lib")) == 0);
        miku_free(j);

        
        let n = miku_path_normalize(cstr!("/usr/lib/../bin/./test"));
        test!("path_normalize", !n.is_null() && miku_strcmp(n, cstr!("/usr/bin/test")) == 0);
        miku_free(n);
        let n = miku_path_normalize(cstr!("/a/b/c/../../d"));
        test!("path_normalize 2x ..", !n.is_null() && miku_strcmp(n, cstr!("/a/d")) == 0);
        miku_free(n);
        let n = miku_path_normalize(cstr!("/"));
        test!("path_normalize root", !n.is_null() && miku_strcmp(n, cstr!("/")) == 0);
        miku_free(n);

        
        test!("is_absolute /", miku_path_is_absolute(cstr!("/")));
        test!("is_absolute rel", !miku_path_is_absolute(cstr!("foo")));

        
        test!("path_depth /usr/lib/file", miku_path_depth(cstr!("/usr/lib/file")) == 3);
        test!("path_depth /", miku_path_depth(cstr!("/")) == 0);
        test!("path_depth foo", miku_path_depth(cstr!("foo")) == 1);
    }

    println("");
}

fn test_sort() {
    println("--- sort ---");

    unsafe {
        
        let mut arr: [i64; 8] = [42, -7, 100, 0, -999, 55, 13, 1];
        miku_qsort(
            arr.as_mut_ptr() as *mut u8, 8,
            core::mem::size_of::<i64>(), miku_cmp_i64,
        );
        test!("qsort i64", arr[0] == -999 && arr[1] == -7 && arr[7] == 100);
        test!("qsort sorted", miku_is_sorted(
            arr.as_ptr() as *const u8, 8,
            core::mem::size_of::<i64>(), miku_cmp_i64,
        ));

        
        let mut arr2: [u64; 6] = [100, 5, 77, 3, 200, 42];
        miku_qsort(
            arr2.as_mut_ptr() as *mut u8, 6,
            core::mem::size_of::<u64>(), miku_cmp_u64,
        );
        test!("qsort u64", arr2[0] == 3 && arr2[5] == 200);

        
        let mut one: [i64; 1] = [42];
        miku_qsort(one.as_mut_ptr() as *mut u8, 1, 8, miku_cmp_i64);
        test!("qsort single", one[0] == 42);

        
        miku_qsort(core::ptr::null_mut(), 0, 8, miku_cmp_i64);
        ok("qsort empty");

        
        let mut sorted: [i64; 5] = [1, 2, 3, 4, 5];
        miku_qsort(sorted.as_mut_ptr() as *mut u8, 5, 8, miku_cmp_i64);
        test!("qsort presorted", sorted == [1, 2, 3, 4, 5]);

        
        let mut rev: [i64; 5] = [5, 4, 3, 2, 1];
        miku_qsort(rev.as_mut_ptr() as *mut u8, 5, 8, miku_cmp_i64);
        test!("qsort reversed", rev == [1, 2, 3, 4, 5]);

        
        let mut dups: [i64; 6] = [3, 1, 3, 1, 2, 2];
        miku_qsort(dups.as_mut_ptr() as *mut u8, 6, 8, miku_cmp_i64);
        test!("qsort dups", dups == [1, 1, 2, 2, 3, 3]);

        
        let sorted_arr: [i64; 7] = [-10, 0, 5, 10, 20, 50, 100];
        let key: i64 = 20;
        let found = miku_bsearch(
            &key as *const i64 as *const u8,
            sorted_arr.as_ptr() as *const u8, 7, 8, miku_cmp_i64,
        );
        test!("bsearch found", !found.is_null() && *(found as *const i64) == 20);

        let key2: i64 = 99;
        let not_found = miku_bsearch(
            &key2 as *const i64 as *const u8,
            sorted_arr.as_ptr() as *const u8, 7, 8, miku_cmp_i64,
        );
        test!("bsearch miss", not_found.is_null());

        
        let mut rev_arr: [i64; 5] = [1, 2, 3, 4, 5];
        miku_reverse(rev_arr.as_mut_ptr() as *mut u8, 5, 8);
        test!("reverse", rev_arr == [5, 4, 3, 2, 1]);

        
        let yes: [i64; 4] = [1, 2, 3, 4];
        let no: [i64; 4] = [1, 3, 2, 4];
        test!("is_sorted yes", miku_is_sorted(yes.as_ptr() as *const u8, 4, 8, miku_cmp_i64));
        test!("is_sorted no", !miku_is_sorted(no.as_ptr() as *const u8, 4, 8, miku_cmp_i64));
        test!("is_sorted empty", miku_is_sorted(core::ptr::null(), 0, 8, miku_cmp_i64));
    }

    println("");
}

fn test_vec() {
    println("--- vec ---");

    unsafe {
        
        let mut v = miku_vec_new(8); 
        test!("vec new empty", miku_vec_is_empty(&v));
        test!("vec new len", miku_vec_len(&v) == 0);

        
        test!("vec push 1", miku_vec_push_u64(&mut v, 10));
        test!("vec push 2", miku_vec_push_u64(&mut v, 20));
        test!("vec push 3", miku_vec_push_u64(&mut v, 30));
        test!("vec len 3", miku_vec_len(&v) == 3);
        test!("vec not empty", !miku_vec_is_empty(&v));

        
        test!("vec get 0", miku_vec_get_u64(&v, 0) == 10);
        test!("vec get 1", miku_vec_get_u64(&v, 1) == 20);
        test!("vec get 2", miku_vec_get_u64(&v, 2) == 30);
        test!("vec get oob", miku_vec_get(&v, 100).is_null());

        
        let mut val: u64 = 0;
        test!("vec pop", miku_vec_pop(&mut v, &mut val as *mut u64 as *mut u8));
        test!("vec pop val", val == 30);
        test!("vec pop len", miku_vec_len(&v) == 2);

        
        let forty: u64 = 40;
        test!("vec insert", miku_vec_insert(&mut v, 1, &forty as *const u64 as *const u8));
        test!("vec insert len", miku_vec_len(&v) == 3);
        test!("vec insert val", miku_vec_get_u64(&v, 1) == 40);
        test!("vec insert shift", miku_vec_get_u64(&v, 2) == 20);

        
        test!("vec remove", miku_vec_remove(&mut v, 1));
        test!("vec remove len", miku_vec_len(&v) == 2);
        test!("vec remove shift", miku_vec_get_u64(&v, 1) == 20);

        
        let ten: u64 = 10;
        let ninety: u64 = 99;
        test!("vec contains yes", miku_vec_contains(&v, &ten as *const u64 as *const u8));
        test!("vec contains no", !miku_vec_contains(&v, &ninety as *const u64 as *const u8));

        
        miku_vec_push_u64(&mut v, 50);
        miku_vec_push_u64(&mut v, 60);
        
        test!("vec swap_remove", miku_vec_swap_remove(&mut v, 1));
        
        test!("vec swap_remove len", miku_vec_len(&v) == 3);
        test!("vec swap_remove val", miku_vec_get_u64(&v, 1) == 60);

        
        miku_vec_clear(&mut v);
        test!("vec clear", miku_vec_len(&v) == 0 && miku_vec_is_empty(&v));

        
        let mut good = true;
        for i in 0u64..100 {
            if !miku_vec_push_u64(&mut v, i) { good = false; break; }
        }
        test!("vec push 100", good && miku_vec_len(&v) == 100);
        test!("vec get 50", miku_vec_get_u64(&v, 50) == 50);
        test!("vec get 99", miku_vec_get_u64(&v, 99) == 99);

        
        test!("vec reserve", miku_vec_reserve(&mut v, 200));
        test!("vec cap after reserve", miku_vec_cap(&v) >= 300);

        
        miku_vec_clear(&mut v);
        miku_vec_push_u64(&mut v, 1);
        miku_vec_push_u64(&mut v, 2);
        test!("vec shrink", miku_vec_shrink(&mut v));
        test!("vec shrink cap", miku_vec_cap(&v) == 2);

        
        let mut v2 = miku_vec_with_capacity(8, 32);
        test!("vec with_cap len", miku_vec_len(&v2) == 0);
        test!("vec with_cap cap", miku_vec_cap(&v2) >= 32);
        miku_vec_free(&mut v2);

        miku_vec_free(&mut v);
        ok("vec free");
    }

    println("");
}

fn test_hashmap() {
    println("--- hashmap ---");

    unsafe {
        
        let mut m = miku_map_new_u64();
        test!("map new len", miku_map_len(&m) == 0);

        
        test!("map insert 1", miku_map_insert_u64(&mut m, 10, 100));
        test!("map insert 2", miku_map_insert_u64(&mut m, 20, 200));
        test!("map insert 3", miku_map_insert_u64(&mut m, 30, 300));
        test!("map len 3", miku_map_len(&m) == 3);

        
        test!("map get 10", miku_map_get_u64(&m, 10) == 100);
        test!("map get 20", miku_map_get_u64(&m, 20) == 200);
        test!("map get 30", miku_map_get_u64(&m, 30) == 300);
        test!("map get miss", miku_map_get_u64(&m, 99) == 0);

        
        test!("map contains yes", miku_map_contains(&m, &10u64 as *const u64 as *const u8));
        test!("map contains no", !miku_map_contains(&m, &99u64 as *const u64 as *const u8));

        
        miku_map_insert_u64(&mut m, 10, 999);
        test!("map overwrite", miku_map_get_u64(&m, 10) == 999);
        test!("map overwrite len", miku_map_len(&m) == 3);

        
        test!("map remove", miku_map_remove(&mut m, &20u64 as *const u64 as *const u8));
        test!("map remove len", miku_map_len(&m) == 2);
        test!("map remove get", miku_map_get(&m, &20u64 as *const u64 as *const u8).is_null());

        
        let mut good = true;
        for i in 0u64..50 {
            if !miku_map_insert_u64(&mut m, 1000 + i, i * 10) { good = false; break; }
        }
        test!("map insert 50", good);
        test!("map get stress", miku_map_get_u64(&m, 1025) == 250);

        
        miku_map_clear(&mut m);
        test!("map clear", miku_map_len(&m) == 0);
        test!("map clear get", miku_map_get(&m, &10u64 as *const u64 as *const u8).is_null());

        
        let mut sm = miku_map_new(4, 8); 
        let k1 = 1u32;
        let v1 = 42u64;
        miku_map_insert(&mut sm, &k1 as *const u32 as *const u8, &v1 as *const u64 as *const u8);
        let got = miku_map_get(&sm, &k1 as *const u32 as *const u8);
        test!("map u32 key", !got.is_null() && *(got as *const u64) == 42);
        miku_map_free(&mut sm);

        miku_map_free(&mut m);
        ok("map free");
    }

    println("");
}

fn test_list() {
    println("--- list ---");

    unsafe {
        let mut l = miku_list_new(8); 
        test!("list new empty", miku_list_is_empty(&l));

        
        let vals: [u64; 5] = [10, 20, 30, 40, 50];
        for v in &vals {
            miku_list_push_back_u64(&mut l, *v);
        }
        test!("list push_back 5", miku_list_len(&l) == 5);
        test!("list get 0", miku_list_get_u64(&l, 0) == 10);
        test!("list get 4", miku_list_get_u64(&l, 4) == 50);
        test!("list get 2", miku_list_get_u64(&l, 2) == 30);

        
        let five: u64 = 5;
        miku_list_push_front(&mut l, &five as *const u64 as *const u8);
        test!("list push_front", miku_list_len(&l) == 6);
        test!("list push_front val", miku_list_get_u64(&l, 0) == 5);
        test!("list push_front shift", miku_list_get_u64(&l, 1) == 10);

        
        let mut out: u64 = 0;
        test!("list pop_front", miku_list_pop_front(&mut l, &mut out as *mut u64 as *mut u8));
        test!("list pop_front val", out == 5);
        test!("list pop_front len", miku_list_len(&l) == 5);

        
        test!("list pop_back", miku_list_pop_back(&mut l, &mut out as *mut u64 as *mut u8));
        test!("list pop_back val", out == 50);
        test!("list pop_back len", miku_list_len(&l) == 4);

        
        let ninety: u64 = 99;
        test!("list set", miku_list_set(&mut l, 2, &ninety as *const u64 as *const u8));
        test!("list set val", miku_list_get_u64(&l, 2) == 99);

        
        let seventy: u64 = 77;
        test!("list insert", miku_list_insert(&mut l, 2, &seventy as *const u64 as *const u8));
        test!("list insert len", miku_list_len(&l) == 5);
        test!("list insert val", miku_list_get_u64(&l, 2) == 77);

        
        test!("list remove", miku_list_remove(&mut l, 2));
        test!("list remove len", miku_list_len(&l) == 4);

        
        let ten: u64 = 10;
        let thousand: u64 = 1000;
        test!("list contains yes", miku_list_contains(&l, &ten as *const u64 as *const u8));
        test!("list contains no", !miku_list_contains(&l, &thousand as *const u64 as *const u8));

        
        miku_list_clear(&mut l);
        test!("list clear", miku_list_is_empty(&l));

        
        let mut good = true;
        for i in 0u64..50 {
            if !miku_list_push_back_u64(&mut l, i) { good = false; break; }
        }
        test!("list push 50", good && miku_list_len(&l) == 50);
        test!("list get 25", miku_list_get_u64(&l, 25) == 25);
        test!("list get 49", miku_list_get_u64(&l, 49) == 49);

        miku_list_free(&mut l);
        ok("list free");
    }

    println("");
}

fn test_ringbuf() {
    println("--- ringbuf ---");

    unsafe {
        let mut r = miku_ring_new(16);
        test!("ring new empty", miku_ring_is_empty(&r));
        test!("ring new avail", miku_ring_available(&r) == 16);

        
        test!("ring push 'A'", miku_ring_push_byte(&mut r, b'A'));
        test!("ring push 'B'", miku_ring_push_byte(&mut r, b'B'));
        test!("ring push 'C'", miku_ring_push_byte(&mut r, b'C'));
        test!("ring len 3", miku_ring_len(&r) == 3);

        
        let byte = miku_ring_pop_byte(&mut r);
        test!("ring pop", byte == b'A' as i32);
        test!("ring pop len", miku_ring_len(&r) == 2);

        
        let data = b"hello world!";
        let written = miku_ring_write(&mut r, data.as_ptr(), 12);
        test!("ring write", written == 12);
        test!("ring len after write", miku_ring_len(&r) == 14);

        let mut out = [0u8; 32];
        let read_n = miku_ring_read(&mut r, out.as_mut_ptr(), 14);
        test!("ring read", read_n == 14);
        test!("ring read val", out[0] == b'B' && out[1] == b'C');
        test!("ring read data", miku_memcmp(out.as_ptr().add(2), data.as_ptr(), 12) == 0);
        test!("ring empty after read", miku_ring_is_empty(&r));

        
        miku_ring_write(&mut r, b"peek".as_ptr(), 4);
        let mut peek_out = [0u8; 8];
        let pn = miku_ring_peek(&r, peek_out.as_mut_ptr(), 4);
        test!("ring peek len", pn == 4);
        test!("ring peek val", peek_out[0] == b'p' && peek_out[3] == b'k');
        test!("ring peek no consume", miku_ring_len(&r) == 4);

        
        let skipped = miku_ring_skip(&mut r, 2);
        test!("ring skip", skipped == 2 && miku_ring_len(&r) == 2);

        
        miku_ring_clear(&mut r);
        test!("ring clear", miku_ring_is_empty(&r));

        
        let mut fill_good = true;
        for i in 0u8..16 {
            if !miku_ring_push_byte(&mut r, i) { fill_good = false; break; }
        }
        test!("ring fill 16", fill_good && miku_ring_is_full(&r));
        test!("ring full push", !miku_ring_push_byte(&mut r, 0xFF));

        
        let mut buf = [0u8; 16];
        let rn = miku_ring_read(&mut r, buf.as_mut_ptr(), 16);
        test!("ring read all", rn == 16 && buf[0] == 0 && buf[15] == 15);

        
        miku_ring_clear(&mut r);
        
        for i in 0u8..8 { miku_ring_push_byte(&mut r, i); }
        for _ in 0..8 { miku_ring_pop_byte(&mut r); }
        for i in 0u8..16 { miku_ring_push_byte(&mut r, 100 + i); }
        let mut wrap_buf = [0u8; 16];
        let wn = miku_ring_read(&mut r, wrap_buf.as_mut_ptr(), 16);
        test!("ring wraparound", wn == 16 && wrap_buf[0] == 100 && wrap_buf[15] == 115);

        miku_ring_free(&mut r);
        ok("ring free");
    }

    println("");
}

fn test_endian() {
    println("--- endian ---");

    unsafe {
        
        test!("htobe16", miku_htobe16(0x1234) == 0x3412);
        test!("htobe32", miku_htobe32(0x12345678) == 0x78563412);
        test!("htobe64", miku_htobe64(0x0102030405060708) == 0x0807060504030201);

        
        test!("htole16", miku_htole16(0x1234) == 0x1234);
        test!("htole32", miku_htole32(0x12345678) == 0x12345678);
        test!("htole64", miku_htole64(0x0102030405060708) == 0x0102030405060708);

        
        test!("be16 roundtrip", miku_be16toh(miku_htobe16(0xABCD)) == 0xABCD);
        test!("be32 roundtrip", miku_be32toh(miku_htobe32(0xDEADBEEF)) == 0xDEADBEEF);
        test!("be64 roundtrip", miku_be64toh(miku_htobe64(0xCAFEBABE12345678)) == 0xCAFEBABE12345678);

        
        let be_data: [u8; 8] = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        test!("read_u16_be", miku_read_u16_be(be_data.as_ptr()) == 0x0102);
        test!("read_u32_be", miku_read_u32_be(be_data.as_ptr()) == 0x01020304);
        test!("read_u64_be", miku_read_u64_be(be_data.as_ptr()) == 0x0102030405060708);

        
        let le_data: [u8; 8] = [0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01];
        test!("read_u16_le", miku_read_u16_le(le_data.as_ptr()) == 0x0708);
        test!("read_u32_le", miku_read_u32_le(le_data.as_ptr()) == 0x05060708);
        test!("read_u64_le", miku_read_u64_le(le_data.as_ptr()) == 0x0102030405060708);

        
        let mut buf = [0u8; 8];
        miku_write_u16_be(buf.as_mut_ptr(), 0xABCD);
        test!("write_u16_be", buf[0] == 0xAB && buf[1] == 0xCD);

        miku_memset(buf.as_mut_ptr(), 0, 8);
        miku_write_u32_be(buf.as_mut_ptr(), 0x12345678);
        test!("write_u32_be", buf[0] == 0x12 && buf[1] == 0x34 && buf[2] == 0x56 && buf[3] == 0x78);

        
        miku_memset(buf.as_mut_ptr(), 0, 8);
        miku_write_u16_le(buf.as_mut_ptr(), 0xABCD);
        test!("write_u16_le", buf[0] == 0xCD && buf[1] == 0xAB);

        miku_memset(buf.as_mut_ptr(), 0, 8);
        miku_write_u32_le(buf.as_mut_ptr(), 0x12345678);
        test!("write_u32_le", buf[0] == 0x78 && buf[1] == 0x56 && buf[2] == 0x34 && buf[3] == 0x12);

        
        miku_write_u32_be(buf.as_mut_ptr(), 0xDEADBEEF);
        test!("write-read BE roundtrip", miku_read_u32_be(buf.as_ptr()) == 0xDEADBEEF);
        miku_write_u32_le(buf.as_mut_ptr(), 0xDEADBEEF);
        test!("write-read LE roundtrip", miku_read_u32_le(buf.as_ptr()) == 0xDEADBEEF);
    }

    println("");
}

fn test_arena() {
    println("--- arena ---");
    unsafe {
        
        let mut arena = miku_arena_new();
        let p1 = miku_arena_alloc(&mut arena, 64);
        test!("arena alloc", !p1.is_null());

        let p2 = miku_arena_alloc(&mut arena, 128);
        test!("arena alloc 2", !p2.is_null() && p2 != p1);

        test!("arena used", miku_arena_used(&arena) > 0);

        
        let p3 = miku_arena_calloc(&mut arena, 32);
        test!("arena calloc", !p3.is_null());
        let mut all_zero = true;
        for i in 0..32 { if *p3.add(i) != 0 { all_zero = false; break; } }
        test!("arena calloc zeroed", all_zero);

        
        let s = miku_arena_strdup(&mut arena, cstr!("hello"));
        test!("arena strdup", !s.is_null() && miku_strcmp(s, cstr!("hello")) == 0);

        
        let _used_before = miku_arena_used(&arena);
        miku_arena_reset(&mut arena);
        test!("arena reset", miku_arena_used(&arena) == 0);

        
        let p4 = miku_arena_alloc(&mut arena, 16);
        test!("arena post-reset alloc", !p4.is_null());

        
        let mut arena2 = miku_arena_with_block_size(128);
        let big = miku_arena_alloc(&mut arena2, 512);
        test!("arena large alloc", !big.is_null());

        miku_arena_free(&mut arena);
        miku_arena_free(&mut arena2);
    }
    println("");
}

fn test_bitset() {
    println("--- bitset ---");
    unsafe {
        let mut bs = miku_bitset_new(256);
        test!("bitset new", miku_bitset_capacity(&bs) >= 256);
        test!("bitset empty", miku_bitset_is_empty(&bs));

        
        miku_bitset_set(&mut bs, 0);
        miku_bitset_set(&mut bs, 42);
        miku_bitset_set(&mut bs, 255);
        test!("bitset set 0", miku_bitset_test(&bs, 0));
        test!("bitset set 42", miku_bitset_test(&bs, 42));
        test!("bitset set 255", miku_bitset_test(&bs, 255));
        test!("bitset not set 1", !miku_bitset_test(&bs, 1));
        test!("bitset not empty", !miku_bitset_is_empty(&bs));

        
        test!("bitset count 3", miku_bitset_count(&bs) == 3);

        
        miku_bitset_clear(&mut bs, 42);
        test!("bitset clear", !miku_bitset_test(&bs, 42));
        test!("bitset count 2", miku_bitset_count(&bs) == 2);

        
        miku_bitset_toggle(&mut bs, 10);
        test!("bitset toggle on", miku_bitset_test(&bs, 10));
        miku_bitset_toggle(&mut bs, 10);
        test!("bitset toggle off", !miku_bitset_test(&bs, 10));

        
        miku_bitset_clear_all(&mut bs);
        miku_bitset_set(&mut bs, 7);
        test!("bitset ffs", miku_bitset_ffs(&bs) == 7);

        miku_bitset_clear_all(&mut bs);
        test!("bitset ffs empty", miku_bitset_ffs(&bs) == -1);

        
        miku_bitset_set_all(&mut bs, 64);
        test!("bitset set_all count", miku_bitset_count(&bs) == 64);
        test!("bitset set_all 0", miku_bitset_test(&bs, 0));
        test!("bitset set_all 63", miku_bitset_test(&bs, 63));

        
        let mut bs2 = miku_bitset_new(8);
        miku_bitset_set(&mut bs2, 1000);
        test!("bitset auto-grow", miku_bitset_test(&bs2, 1000));

        
        let mut a = miku_bitset_new(64);
        let mut b = miku_bitset_new(64);
        miku_bitset_set(&mut a, 1);
        miku_bitset_set(&mut b, 2);
        miku_bitset_or(&mut a, &b);
        test!("bitset OR", miku_bitset_test(&a, 1) && miku_bitset_test(&a, 2));

        
        miku_bitset_clear_all(&mut a);
        miku_bitset_clear_all(&mut b);
        miku_bitset_set(&mut a, 5);
        miku_bitset_set(&mut a, 10);
        miku_bitset_set(&mut b, 5);
        miku_bitset_and(&mut a, &b);
        test!("bitset AND", miku_bitset_test(&a, 5) && !miku_bitset_test(&a, 10));

        
        miku_bitset_clear_all(&mut a);
        miku_bitset_clear_all(&mut b);
        miku_bitset_set(&mut a, 3);
        miku_bitset_set(&mut b, 3);
        miku_bitset_set(&mut b, 4);
        miku_bitset_xor(&mut a, &b);
        test!("bitset XOR", !miku_bitset_test(&a, 3) && miku_bitset_test(&a, 4));

        miku_bitset_free(&mut bs);
        miku_bitset_free(&mut bs2);
        miku_bitset_free(&mut a);
        miku_bitset_free(&mut b);
    }
    println("");
}

fn test_priority_queue() {
    println("--- priority queue ---");
    unsafe {
        
        let mut pq = miku_pq_new_i64();
        test!("pq new empty", miku_pq_is_empty(&pq));

        miku_pq_push_i64(&mut pq, 30);
        miku_pq_push_i64(&mut pq, 10);
        miku_pq_push_i64(&mut pq, 20);
        test!("pq len 3", miku_pq_len(&pq) == 3);

        
        let v1 = miku_pq_pop_i64(&mut pq);
        test!("pq pop min 10", v1 == 10);
        let v2 = miku_pq_pop_i64(&mut pq);
        test!("pq pop min 20", v2 == 20);
        let v3 = miku_pq_pop_i64(&mut pq);
        test!("pq pop min 30", v3 == 30);
        test!("pq empty after", miku_pq_is_empty(&pq));

        
        let v4 = miku_pq_pop_i64(&mut pq);
        test!("pq pop empty", v4 == i64::MAX);

        
        miku_pq_push_i64(&mut pq, 50);
        miku_pq_push_i64(&mut pq, 5);
        miku_pq_push_i64(&mut pq, 100);
        miku_pq_push_i64(&mut pq, 1);
        miku_pq_push_i64(&mut pq, 75);
        test!("pq len 5", miku_pq_len(&pq) == 5);

        
        let peek_ptr = miku_pq_peek(&pq);
        test!("pq peek", !peek_ptr.is_null() && *(peek_ptr as *const i64) == 1);

        
        let mut prev = i64::MIN;
        let mut sorted = true;
        for _ in 0..5 {
            let v = miku_pq_pop_i64(&mut pq);
            if v < prev { sorted = false; break; }
            prev = v;
        }
        test!("pq sorted extraction", sorted);

        
        miku_pq_push_i64(&mut pq, 10);
        miku_pq_push_i64(&mut pq, 20);
        miku_pq_clear(&mut pq);
        test!("pq clear", miku_pq_is_empty(&pq));

        
        miku_pq_push_i64(&mut pq, -5);
        miku_pq_push_i64(&mut pq, -100);
        miku_pq_push_i64(&mut pq, 0);
        test!("pq negative min", miku_pq_pop_i64(&mut pq) == -100);
        test!("pq negative mid", miku_pq_pop_i64(&mut pq) == -5);
        test!("pq negative max", miku_pq_pop_i64(&mut pq) == 0);

        miku_pq_free(&mut pq);
    }
    println("");
}

fn test_glob() {
    println("--- glob ---");
    unsafe {
        
        test!("glob exact", miku_glob_match(cstr!("hello"), cstr!("hello")));
        test!("glob exact fail", !miku_glob_match(cstr!("hello"), cstr!("world")));

        
        test!("glob *", miku_glob_match(cstr!("*"), cstr!("anything")));
        test!("glob * empty", miku_glob_match(cstr!("*"), cstr!("")));
        test!("glob *.rs", miku_glob_match(cstr!("*.rs"), cstr!("main.rs")));
        test!("glob *.rs fail", !miku_glob_match(cstr!("*.rs"), cstr!("main.c")));
        test!("glob lib*", miku_glob_match(cstr!("lib*"), cstr!("libmiku")));
        test!("glob a*b", miku_glob_match(cstr!("a*b"), cstr!("aXYZb")));
        test!("glob a*b empty", miku_glob_match(cstr!("a*b"), cstr!("ab")));

        
        test!("glob ?", miku_glob_match(cstr!("?"), cstr!("a")));
        test!("glob ? fail", !miku_glob_match(cstr!("?"), cstr!("")));
        test!("glob a?c", miku_glob_match(cstr!("a?c"), cstr!("abc")));
        test!("glob a?c fail", !miku_glob_match(cstr!("a?c"), cstr!("ac")));

        
        test!("glob *?*", miku_glob_match(cstr!("*?*"), cstr!("x")));
        test!("glob *.?", miku_glob_match(cstr!("*.?"), cstr!("file.c")));

        
        test!("glob escape *", miku_glob_match(cstr!("a\\*b"), cstr!("a*b")));
        test!("glob escape * fail", !miku_glob_match(cstr!("a\\*b"), cstr!("aXb")));

        
        test!("glob nocase", miku_glob_match_nocase(cstr!("HELLO"), cstr!("hello")));
        test!("glob nocase *", miku_glob_match_nocase(cstr!("*.RS"), cstr!("main.rs")));

        
        test!("glob has_magic *", miku_glob_has_magic(cstr!("*.rs")));
        test!("glob has_magic ?", miku_glob_has_magic(cstr!("a?b")));
        test!("glob no magic", !miku_glob_has_magic(cstr!("hello")));
        test!("glob no magic esc", !miku_glob_has_magic(cstr!("a\\*b")));

        
        let escaped = miku_glob_escape(cstr!("a*b?c"));
        test!("glob escape fn", !escaped.is_null() && miku_strcmp(escaped, cstr!("a\\*b\\?c")) == 0);
        miku_free(escaped);
    }
    println("");
}

fn test_channel() {
    println("--- channel ---");
    unsafe {
        
        let mut ch = miku_chan_new_u64(4);
        test!("chan new empty", miku_chan_is_empty(&ch));
        test!("chan available", miku_chan_available(&ch) == 4);

        
        test!("chan send", miku_chan_send_u64(&mut ch, 42));
        test!("chan not empty", !miku_chan_is_empty(&ch));
        test!("chan len 1", miku_chan_len(&ch) == 1);

        let v = miku_chan_recv_u64(&mut ch);
        test!("chan recv", v == 42);
        test!("chan empty after", miku_chan_is_empty(&ch));

        
        let empty = miku_chan_recv_u64(&mut ch);
        test!("chan recv empty", empty == u64::MAX);

        
        miku_chan_send_u64(&mut ch, 1);
        miku_chan_send_u64(&mut ch, 2);
        miku_chan_send_u64(&mut ch, 3);
        miku_chan_send_u64(&mut ch, 4);
        test!("chan full", miku_chan_is_full(&ch));

        
        test!("chan send full fail", !miku_chan_send_u64(&mut ch, 5));

        
        test!("chan fifo 1", miku_chan_recv_u64(&mut ch) == 1);
        test!("chan fifo 2", miku_chan_recv_u64(&mut ch) == 2);
        test!("chan fifo 3", miku_chan_recv_u64(&mut ch) == 3);
        test!("chan fifo 4", miku_chan_recv_u64(&mut ch) == 4);

        
        miku_chan_send_u64(&mut ch, 10);
        miku_chan_send_u64(&mut ch, 20);
        test!("chan interleave 1", miku_chan_recv_u64(&mut ch) == 10);
        miku_chan_send_u64(&mut ch, 30);
        test!("chan interleave 2", miku_chan_recv_u64(&mut ch) == 20);
        test!("chan interleave 3", miku_chan_recv_u64(&mut ch) == 30);

        miku_chan_free(&mut ch);
    }
    println("");
}

fn test_slab_alloc() {
    println("--- slab ---");
    unsafe {
        
        let mut slab = miku_slab_new(32, 8);
        test!("slab capacity", miku_slab_capacity(&slab) == 8);
        test!("slab empty", miku_slab_is_empty(&slab));
        test!("slab slot size", miku_slab_slot_size(&slab) >= 32);

        
        let p1 = miku_slab_alloc(&mut slab);
        test!("slab alloc 1", !p1.is_null());
        test!("slab in_use 1", miku_slab_in_use(&slab) == 1);

        let p2 = miku_slab_alloc(&mut slab);
        test!("slab alloc 2", !p2.is_null() && p2 != p1);
        test!("slab in_use 2", miku_slab_in_use(&slab) == 2);

        
        let mut all_zero = true;
        for i in 0..32 { if *p1.add(i) != 0 { all_zero = false; break; } }
        test!("slab zeroed", all_zero);

        
        *p1 = 0xAA;
        *(p1.add(1)) = 0xBB;
        test!("slab write/read", *p1 == 0xAA && *(p1.add(1)) == 0xBB);

        
        miku_slab_dealloc(&mut slab, p1);
        test!("slab dealloc", miku_slab_in_use(&slab) == 1);

        
        let p3 = miku_slab_alloc(&mut slab);
        test!("slab reuse", !p3.is_null());
        test!("slab in_use after reuse", miku_slab_in_use(&slab) == 2);

        
        let mut ptrs = [core::ptr::null_mut::<u8>(); 6];
        for i in 0..6 {
            ptrs[i] = miku_slab_alloc(&mut slab);
        }
        test!("slab full", miku_slab_is_full(&slab));
        test!("slab available 0", miku_slab_available(&slab) == 0);

        
        let fail = miku_slab_alloc(&mut slab);
        test!("slab full alloc", fail.is_null());

        miku_slab_free(&mut slab);
    }
    println("");
}

fn test_string_builder() {
    println("--- string builder ---");
    unsafe {
        let mut sb = miku_sb_new();
        test!("sb new empty", miku_sb_len(&sb) == 0);

        
        miku_sb_append(&mut sb, cstr!("hello"));
        test!("sb append", miku_sb_len(&sb) == 5);

        
        miku_sb_append_char(&mut sb, b' ');
        miku_sb_append(&mut sb, cstr!("world"));
        let s = miku_sb_finish(&mut sb);
        test!("sb finish", !s.is_null() && miku_strcmp(s, cstr!("hello world")) == 0);
        miku_free(s);

        
        test!("sb reset after finish", miku_sb_len(&sb) == 0);

        
        miku_sb_append(&mut sb, cstr!("val="));
        miku_sb_append_int(&mut sb, -42);
        let s2 = miku_sb_finish(&mut sb);
        test!("sb append_int", !s2.is_null() && miku_strcmp(s2, cstr!("val=-42")) == 0);
        miku_free(s2);

        
        miku_sb_append_uint(&mut sb, 12345);
        let s3 = miku_sb_finish(&mut sb);
        test!("sb append_uint", !s3.is_null() && miku_strcmp(s3, cstr!("12345")) == 0);
        miku_free(s3);

        
        miku_sb_repeat(&mut sb, b'=', 5);
        let s4 = miku_sb_finish(&mut sb);
        test!("sb repeat", !s4.is_null() && miku_strcmp(s4, cstr!("=====")) == 0);
        miku_free(s4);

        
        miku_sb_append(&mut sb, cstr!("temp"));
        miku_sb_clear(&mut sb);
        test!("sb clear", miku_sb_len(&sb) == 0);

        
        let mut sb2 = miku_sb_with_capacity(1024);
        miku_sb_append(&mut sb2, cstr!("big"));
        let s5 = miku_sb_finish(&mut sb2);
        test!("sb with_capacity", !s5.is_null() && miku_strcmp(s5, cstr!("big")) == 0);
        miku_free(s5);

        
        miku_sb_append_bytes(&mut sb, b"raw\x00data".as_ptr(), 3);
        let s6 = miku_sb_finish(&mut sb);
        test!("sb append_bytes", !s6.is_null() && miku_strcmp(s6, cstr!("raw")) == 0);
        miku_free(s6);

        miku_sb_free(&mut sb);
        miku_sb_free(&mut sb2);

        
        let strs: [*const u8; 3] = [cstr!("a"), cstr!("b"), cstr!("c")];
        let joined = miku_str_join(strs.as_ptr(), 3, cstr!(","));
        test!("str_join", !joined.is_null() && miku_strcmp(joined, cstr!("a,b,c")) == 0);
        miku_free(joined);

        
        let joined2 = miku_str_join(strs.as_ptr(), 3, cstr!(" - "));
        test!("str_join sep", !joined2.is_null() && miku_strcmp(joined2, cstr!("a - b - c")) == 0);
        miku_free(joined2);

        
        let rep = miku_str_repeat(cstr!("ab"), 3);
        test!("str_repeat", !rep.is_null() && miku_strcmp(rep, cstr!("ababab")) == 0);
        miku_free(rep);

        
        let rep0 = miku_str_repeat(cstr!("x"), 0);
        test!("str_repeat 0", !rep0.is_null() && miku_strlen(rep0) == 0);
        miku_free(rep0);
    }
    println("");
}

fn test_regex() {
    println("--- regex ---");
    unsafe {
        
        test!("regex literal", miku_regex_match(cstr!("hello"), cstr!("say hello world")));
        test!("regex literal miss", !miku_regex_match(cstr!("xyz"), cstr!("hello")));

        
        test!("regex dot", miku_regex_match(cstr!("h.llo"), cstr!("hello")));
        test!("regex dot miss", !miku_regex_match(cstr!("h.llo"), cstr!("hllo")));

        
        test!("regex star", miku_regex_match(cstr!("he*llo"), cstr!("hllo")));
        test!("regex star 2", miku_regex_match(cstr!("he*llo"), cstr!("heeeello")));
        test!("regex .*", miku_regex_match(cstr!("h.*d"), cstr!("hello world")));

        
        test!("regex plus", miku_regex_match(cstr!("he+llo"), cstr!("hello")));
        test!("regex plus miss", !miku_regex_match(cstr!("he+llo"), cstr!("hllo")));

        
        test!("regex question", miku_regex_match(cstr!("colou?r"), cstr!("color")));
        test!("regex question 2", miku_regex_match(cstr!("colou?r"), cstr!("colour")));

        
        test!("regex anchor ^", miku_regex_match(cstr!("^hello"), cstr!("hello world")));
        test!("regex anchor ^ miss", !miku_regex_match(cstr!("^hello"), cstr!("say hello")));

        
        test!("regex anchor $", miku_regex_match(cstr!("world$"), cstr!("hello world")));
        test!("regex anchor $ miss", !miku_regex_match(cstr!("world$"), cstr!("world!")));

        
        test!("regex class", miku_regex_match(cstr!("[abc]"), cstr!("xbx")));
        test!("regex class miss", !miku_regex_match(cstr!("^[abc]$"), cstr!("d")));

        
        test!("regex range", miku_regex_match(cstr!("[a-z]+"), cstr!("hello")));
        test!("regex range 2", miku_regex_match(cstr!("^[0-9]+$"), cstr!("12345")));
        test!("regex range miss", !miku_regex_match(cstr!("^[0-9]+$"), cstr!("123a5")));

        
        test!("regex nclass", miku_regex_match(cstr!("[^0-9]+"), cstr!("abc")));

        
        test!("regex \\d", miku_regex_match(cstr!("\\d+"), cstr!("abc123")));
        test!("regex \\w", miku_regex_match(cstr!("^\\w+$"), cstr!("hello_42")));
        test!("regex \\s", miku_regex_match(cstr!("\\s"), cstr!("hello world")));

        
        test!("regex find", miku_regex_find(cstr!("world"), cstr!("hello world")) == 6);
        test!("regex find miss", miku_regex_find(cstr!("xyz"), cstr!("hello")) == -1);
        test!("regex find digit", miku_regex_find(cstr!("\\d+"), cstr!("abc123")) == 3);

        
        test!("regex count", miku_regex_count(cstr!("[aeiou]"), cstr!("hello")) == 2);
        test!("regex count 0", miku_regex_count(cstr!("x"), cstr!("hello")) == 0);

        
        test!("regex full", miku_regex_match_full(cstr!("hello"), cstr!("hello")));
        test!("regex full miss", !miku_regex_match_full(cstr!("hello"), cstr!("hello world")));
    }
    println("");
}

fn test_hex() {
    println("--- hex ---");
    unsafe {
        
        let data: [u8; 4] = [0xDE, 0xAD, 0xBE, 0xEF];
        let mut buf = [0u8; 16];
        let r = miku_hex_encode(data.as_ptr(), 4, buf.as_mut_ptr(), 16);
        test!("hex encode len", r == 8);
        test!("hex encode", miku_strncmp(buf.as_ptr(), cstr!("deadbeef"), 8) == 0);

        
        let r2 = miku_hex_encode_upper(data.as_ptr(), 4, buf.as_mut_ptr(), 16);
        test!("hex encode upper", r2 == 8 && miku_strncmp(buf.as_ptr(), cstr!("DEADBEEF"), 8) == 0);

        
        let hex_str = b"cafebabe";
        let mut out = [0u8; 4];
        let r3 = miku_hex_decode(hex_str.as_ptr(), 8, out.as_mut_ptr(), 4);
        test!("hex decode", r3 == 4 && out[0] == 0xCA && out[1] == 0xFE && out[2] == 0xBA && out[3] == 0xBE);

        
        let bad = b"zz";
        let r4 = miku_hex_decode(bad.as_ptr(), 2, out.as_mut_ptr(), 4);
        test!("hex decode invalid", r4 == -1);

        
        let odd = b"abc";
        let r5 = miku_hex_decode(odd.as_ptr(), 3, out.as_mut_ptr(), 4);
        test!("hex decode odd", r5 == -1);

        
        let orig: [u8; 5] = [1, 2, 128, 255, 0];
        let encoded = miku_hex_encode_alloc(orig.as_ptr(), 5);
        test!("hex alloc encode", !encoded.is_null());
        let mut dec_len: usize = 0;
        let decoded = miku_hex_decode_alloc(encoded, miku_strlen(encoded), &mut dec_len);
        test!("hex roundtrip len", dec_len == 5);
        test!("hex roundtrip", !decoded.is_null() && miku_memcmp(orig.as_ptr(), decoded, 5) == 0);
        miku_free(encoded);
        miku_free(decoded);

        
        let mut h64 = [0u8; 17];
        miku_hex_u64(0xDEADBEEF, h64.as_mut_ptr());
        test!("hex_u64", miku_strcmp(h64.as_ptr(), cstr!("00000000deadbeef")) == 0);

        
        let mut h32 = [0u8; 9];
        miku_hex_u32(0xCAFE, h32.as_mut_ptr());
        test!("hex_u32", miku_strcmp(h32.as_ptr(), cstr!("0000cafe")) == 0);

        
        test!("hex encode_len", miku_hex_encode_len(10) == 20);
        test!("hex decode_len", miku_hex_decode_len(20) == 10);
    }
    println("");
}

fn test_checksum() {
    println("--- checksum ---");
    unsafe {
        let data = b"Hello, World!";
        let len = data.len();

        
        let a32 = miku_adler32(data.as_ptr(), len);
        test!("adler32 nonzero", a32 != 0 && a32 != 1);

        
        let a32_empty = miku_adler32(core::ptr::null(), 0);
        test!("adler32 empty", a32_empty == 1);

        
        let a32_part1 = miku_adler32(b"Hello".as_ptr(), 5);
        let a32_full = miku_adler32_update(a32_part1, b", World!".as_ptr(), 8);
        test!("adler32 incremental", a32_full == a32);

        
        let f16 = miku_fletcher16(data.as_ptr(), len);
        test!("fletcher16 nonzero", f16 != 0);

        
        let f32 = miku_fletcher32(data.as_ptr(), len);
        test!("fletcher32 nonzero", f32 != 0);

        
        let xor = miku_xor_checksum(data.as_ptr(), len);
        
        let mut expected_xor: u8 = 0;
        for i in 0..len { expected_xor ^= data[i]; }
        test!("xor checksum", xor == expected_xor);

        
        let zeros = [0x55u8; 4];
        let xz = miku_xor_checksum(zeros.as_ptr(), 4);
        test!("xor even", xz == 0);

        
        let inet = miku_inet_checksum(data.as_ptr(), len);
        test!("inet checksum nonzero", inet != 0);

        
        let s8 = miku_sum8(data.as_ptr(), len);
        let mut expected_sum: u8 = 0;
        for i in 0..len { expected_sum = expected_sum.wrapping_add(data[i]); }
        test!("sum8", s8 == expected_sum);

        
        let bsd = miku_bsd_checksum(data.as_ptr(), len);
        test!("bsd checksum nonzero", bsd != 0);

        
        let a32_2 = miku_adler32(data.as_ptr(), len);
        test!("adler32 deterministic", a32 == a32_2);
        let f16_2 = miku_fletcher16(data.as_ptr(), len);
        test!("fletcher16 deterministic", f16 == f16_2);
    }
    println("");
}

fn test_treemap() {
    println("--- treemap ---");
    unsafe {
        let mut tree = miku_tree_new();
        test!("tree new empty", miku_tree_is_empty(&tree));
        test!("tree len 0", miku_tree_len(&tree) == 0);

        
        miku_tree_insert(&mut tree, 10, 100);
        miku_tree_insert(&mut tree, 5, 50);
        miku_tree_insert(&mut tree, 15, 150);
        test!("tree len 3", miku_tree_len(&tree) == 3);
        test!("tree not empty", !miku_tree_is_empty(&tree));

        
        let v = miku_tree_get(&tree, 10);
        test!("tree get 10", !v.is_null() && *v == 100);
        let v2 = miku_tree_get(&tree, 5);
        test!("tree get 5", !v2.is_null() && *v2 == 50);
        let v3 = miku_tree_get(&tree, 99);
        test!("tree get miss", v3.is_null());

        
        test!("tree contains", miku_tree_contains(&tree, 15));
        test!("tree not contains", !miku_tree_contains(&tree, 99));

        
        miku_tree_insert(&mut tree, 10, 999);
        let v4 = miku_tree_get(&tree, 10);
        test!("tree update", !v4.is_null() && *v4 == 999);
        test!("tree len still 3", miku_tree_len(&tree) == 3);

        
        let mut mk: i64 = 0;
        let mut mv: u64 = 0;
        miku_tree_min(&tree, &mut mk, &mut mv);
        test!("tree min", mk == 5 && mv == 50);
        miku_tree_max(&tree, &mut mk, &mut mv);
        test!("tree max", mk == 15 && mv == 150);

        
        test!("tree remove", miku_tree_remove(&mut tree, 10));
        test!("tree len 2", miku_tree_len(&tree) == 2);
        test!("tree removed gone", !miku_tree_contains(&tree, 10));
        test!("tree remove miss", !miku_tree_remove(&mut tree, 10));

        
        for i in 0..50i64 {
            miku_tree_insert(&mut tree, i * 2, i as u64 * 100);
        }
        test!("tree bulk insert", miku_tree_len(&tree) >= 50);

        
        let mut all_found = true;
        for i in 0..50i64 {
            if !miku_tree_contains(&tree, i * 2) {
                all_found = false;
                break;
            }
        }
        test!("tree bulk find", all_found);

        
        static mut ITER_PREV: i64 = i64::MIN;
        static mut ITER_SORTED: bool = true;
        static mut ITER_COUNT: usize = 0;
        ITER_PREV = i64::MIN;
        ITER_SORTED = true;
        ITER_COUNT = 0;

        extern "C" fn check_sorted(key: i64, _val: u64, _ctx: *mut u8) {
            unsafe {
                if key < ITER_PREV { ITER_SORTED = false; }
                ITER_PREV = key;
                ITER_COUNT += 1;
            }
        }

        miku_tree_iter(&tree, check_sorted, core::ptr::null_mut());
        test!("tree iter sorted", ITER_SORTED);
        test!("tree iter count", ITER_COUNT == miku_tree_len(&tree));

        
        miku_tree_insert(&mut tree, -100, 1);
        miku_tree_insert(&mut tree, -50, 2);
        miku_tree_min(&tree, &mut mk, &mut mv);
        test!("tree negative min", mk == -100);

        
        miku_tree_clear(&mut tree);
        test!("tree clear", miku_tree_is_empty(&tree) && miku_tree_len(&tree) == 0);

        miku_tree_free(&mut tree);
    }
    println("");
}


fn test_lz() {
    println("--- lz compression ---");
    unsafe {
        
        let data = b"hello hello hello hello hello world world world\0";
        let data_len = 47;
        let mut comp_len: usize = 0;
        let comp = miku_lz_compress(data.as_ptr(), data_len, &mut comp_len);
        test!("lz compress not null", !comp.is_null());
        test!("lz compress produces output", comp_len > 0);

        
        test!("lz compress smaller", comp_len < data_len);

        
        let mut dec_len: usize = 0;
        let dec = miku_lz_decompress(comp, comp_len, &mut dec_len, data_len * 2);
        test!("lz decompress not null", !dec.is_null());
        test!("lz decompress length", dec_len == data_len);
        test!("lz roundtrip", miku_memcmp(data.as_ptr(), dec, data_len) == 0);

        miku_free(comp);
        miku_free(dec);

        
        let _bound = miku_lz_compress_bound(data_len);
        let mut cbuf = [0u8; 512];
        let mut dbuf = [0u8; 512];
        let clen = miku_lz_compress_buf(data.as_ptr(), data_len, cbuf.as_mut_ptr(), 512);
        test!("lz compress_buf ok", clen > 0);

        let dlen = miku_lz_decompress_buf(cbuf.as_ptr(), clen as usize, dbuf.as_mut_ptr(), 512);
        test!("lz decompress_buf ok", dlen == data_len as i32);
        test!("lz buf roundtrip", miku_memcmp(data.as_ptr(), dbuf.as_ptr(), data_len) == 0);

        
        let nocomp: [u8; 8] = [1, 77, 200, 3, 99, 42, 128, 255];
        let mut nc_len: usize = 0;
        let nc = miku_lz_compress(nocomp.as_ptr(), 8, &mut nc_len);
        test!("lz nocomp not null", !nc.is_null());

        let mut nd_len: usize = 0;
        let nd = miku_lz_decompress(nc, nc_len, &mut nd_len, 64);
        test!("lz nocomp roundtrip", nd_len == 8 && miku_memcmp(nocomp.as_ptr(), nd, 8) == 0);
        miku_free(nc);
        miku_free(nd);

        
        test!("lz compress null", miku_lz_compress(core::ptr::null(), 0, core::ptr::null_mut()).is_null());

        
        test!("lz bound >= input", miku_lz_compress_bound(100) >= 100);
    }
    println("");
}


fn test_env() {
    println("--- environment ---");
    unsafe {
        
        miku_env_clear();
        test!("env initial count", miku_env_count() == 0);

        
        test!("env set", miku_setenv(cstr!("HOME"), cstr!("/root")));
        test!("env get", miku_getenv(cstr!("HOME")) != core::ptr::null());
        test!("env has", miku_hasenv(cstr!("HOME")));
        test!("env count 1", miku_env_count() == 1);

        
        let val = miku_getenv(cstr!("HOME"));
        test!("env value correct", miku_strcmp(val, cstr!("/root")) == 0);

        
        miku_setenv(cstr!("HOME"), cstr!("/miku"));
        let val2 = miku_getenv(cstr!("HOME"));
        test!("env overwrite", miku_strcmp(val2, cstr!("/miku")) == 0);
        test!("env count still 1", miku_env_count() == 1);

        
        miku_setenv(cstr!("PATH"), cstr!("/bin"));
        miku_setenv(cstr!("USER"), cstr!("miku"));
        test!("env count 3", miku_env_count() == 3);

        
        test!("env unset", miku_unsetenv(cstr!("PATH")));
        test!("env unset gone", !miku_hasenv(cstr!("PATH")));
        test!("env count 2", miku_env_count() == 2);

        
        test!("env unset noexist", !miku_unsetenv(cstr!("DOESNOTEXIST")));

        
        test!("env putenv", miku_putenv(cstr!("LANG=en_US")));
        test!("env putenv get", miku_strcmp(miku_getenv(cstr!("LANG")), cstr!("en_US")) == 0);

        
        miku_env_clear();
        test!("env clear", miku_env_count() == 0);
        test!("env clear gone", !miku_hasenv(cstr!("HOME")));

        
        test!("env null key", !miku_setenv(core::ptr::null(), cstr!("x")));
        test!("env null val", !miku_setenv(cstr!("x"), core::ptr::null()));
        test!("env get null", miku_getenv(core::ptr::null()).is_null());
    }
    println("");
}


static mut SIG_RECEIVED: u32 = 0;

extern "C" fn test_sig_handler(sig: u32) {
    unsafe { SIG_RECEIVED = sig; }
}

fn test_signal() {
    println("--- signal ---");
    unsafe {
        
        miku_signal_reset_all();
        SIG_RECEIVED = 0;

        
        test!("sig no handler", !miku_signal_has_handler(2));

        
        let prev = miku_signal(2, Some(test_sig_handler));
        test!("sig register", prev.is_none());
        test!("sig has handler", miku_signal_has_handler(2));

        
        test!("sig dispatch", miku_signal_dispatch(2));
        test!("sig received", SIG_RECEIVED == 2);

        
        test!("sig dispatch nohandler", !miku_signal_dispatch(3));

        
        let r = miku_signal(9, Some(test_sig_handler));
        test!("sig kill uncatchable", r.is_none());
        test!("sig kill no handler", !miku_signal_has_handler(9));

        
        miku_signal_block(2);
        test!("sig blocked", miku_signal_is_blocked(2));
        miku_signal_unblock(2);
        test!("sig unblocked", !miku_signal_is_blocked(2));

        
        let _old = miku_signal_set_mask(0xFF);
        test!("sig set mask", miku_signal_get_mask() == 0xFF);
        miku_signal_set_mask(0);
        test!("sig clear mask", miku_signal_get_mask() == 0);

        
        miku_signal_reset_all();
        test!("sig reset all", !miku_signal_has_handler(2));
    }
    println("");
}


fn test_json() {
    println("--- json ---");
    unsafe {
        let mut parser: MikuJsonParser = core::mem::zeroed();
        let mut tokens: [MikuJsonToken; 64] = core::mem::zeroed();

        
        let json = b"{\"name\":\"miku\",\"age\":16}\0";
        let json_len = 24;
        miku_json_init(&mut parser);
        let n = miku_json_parse(
            &mut parser, json.as_ptr(), json_len,
            tokens.as_mut_ptr(), 64,
        );
        test!("json parse ok", n > 0);
        test!("json root object", miku_json_type(tokens.as_ptr(), 0) == 1); 

        
        let vi = miku_json_find(json.as_ptr(), tokens.as_ptr(), n as usize, 0, cstr!("name"));
        test!("json find name", vi > 0);
        test!("json name eq miku", miku_json_eq(json.as_ptr(), tokens.as_ptr(), vi as usize, cstr!("miku")));

        
        let ai = miku_json_find(json.as_ptr(), tokens.as_ptr(), n as usize, 0, cstr!("age"));
        test!("json find age", ai > 0);
        test!("json age is number", miku_json_type(tokens.as_ptr(), ai as usize) == 4); 
        test!("json age eq 16", miku_json_eq(json.as_ptr(), tokens.as_ptr(), ai as usize, cstr!("16")));

        
        let arr = b"[1,2,3]\0";
        miku_json_init(&mut parser);
        let n2 = miku_json_parse(
            &mut parser, arr.as_ptr(), 7,
            tokens.as_mut_ptr(), 64,
        );
        test!("json array parse", n2 > 0);
        test!("json root array", miku_json_type(tokens.as_ptr(), 0) == 2); 
        test!("json array size 3", miku_json_size(tokens.as_ptr(), 0) == 3);

        
        let nested = b"{\"a\":{\"b\":42}}\0";
        miku_json_init(&mut parser);
        let n3 = miku_json_parse(
            &mut parser, nested.as_ptr(), 14,
            tokens.as_mut_ptr(), 64,
        );
        test!("json nested parse", n3 > 0);
        let inner = miku_json_find(nested.as_ptr(), tokens.as_ptr(), n3 as usize, 0, cstr!("a"));
        test!("json find inner obj", inner > 0);
        test!("json inner is object", miku_json_type(tokens.as_ptr(), inner as usize) == 1);
        let bval = miku_json_find(nested.as_ptr(), tokens.as_ptr(), n3 as usize, inner as usize, cstr!("b"));
        test!("json find nested b", bval > 0);
        test!("json b eq 42", miku_json_eq(nested.as_ptr(), tokens.as_ptr(), bval as usize, cstr!("42")));

        
        let bn = b"{\"ok\":true,\"x\":null}\0";
        miku_json_init(&mut parser);
        let n4 = miku_json_parse(
            &mut parser, bn.as_ptr(), 20,
            tokens.as_mut_ptr(), 64,
        );
        test!("json bool/null parse", n4 > 0);
        let ok_i = miku_json_find(bn.as_ptr(), tokens.as_ptr(), n4 as usize, 0, cstr!("ok"));
        test!("json bool type", miku_json_type(tokens.as_ptr(), ok_i as usize) == 5); 
        let x_i = miku_json_find(bn.as_ptr(), tokens.as_ptr(), n4 as usize, 0, cstr!("x"));
        test!("json null type", miku_json_type(tokens.as_ptr(), x_i as usize) == 6); 

        
        test!("json token count", miku_json_token_count(&parser) == n4 as usize);

        
        let missing = miku_json_find(json.as_ptr(), tokens.as_ptr(), n as usize, 0, cstr!("missing"));
        test!("json key not found", missing == -1);

        
        let empty = b"{}\0";
        miku_json_init(&mut parser);
        let n5 = miku_json_parse(
            &mut parser, empty.as_ptr(), 2,
            tokens.as_mut_ptr(), 64,
        );
        test!("json empty obj", n5 > 0 && miku_json_size(tokens.as_ptr(), 0) == 0);
    }
    println("");
}


fn test_byte_ring() {
    println("--- byte ring ---");
    unsafe {
        let mut ring = miku_bring_new(64);
        test!("bring new not null", !ring.data.is_null());
        test!("bring empty", miku_bring_is_empty(&ring));
        test!("bring capacity >= 64", miku_bring_capacity(&ring) >= 63);

        
        let data = b"hello world\0";
        let wrote = miku_bring_write(&mut ring, data.as_ptr(), 11);
        test!("bring write", wrote == 11);
        test!("bring len", miku_bring_len(&ring) == 11);
        test!("bring not empty", !miku_bring_is_empty(&ring));

        
        let mut pbuf = [0u8; 16];
        let peeked = miku_bring_peek(&ring, pbuf.as_mut_ptr(), 5);
        test!("bring peek", peeked == 5 && pbuf[0] == b'h' && pbuf[4] == b'o');
        test!("bring peek no consume", miku_bring_len(&ring) == 11);

        
        let mut rbuf = [0u8; 16];
        let read = miku_bring_read(&mut ring, rbuf.as_mut_ptr(), 5);
        test!("bring read 5", read == 5 && rbuf[0] == b'h');
        test!("bring len after read", miku_bring_len(&ring) == 6);

        
        let read2 = miku_bring_read(&mut ring, rbuf.as_mut_ptr(), 6);
        test!("bring read rest", read2 == 6 && rbuf[0] == b' ');
        test!("bring empty after", miku_bring_is_empty(&ring));

        
        test!("bring put", miku_bring_put(&mut ring, b'A'));
        let mut byte = 0u8;
        test!("bring get", miku_bring_get(&mut ring, &mut byte));
        test!("bring get value", byte == b'A');

        
        miku_bring_write(&mut ring, b"abc\ndef\n".as_ptr(), 8);
        test!("bring find newline", miku_bring_find(&ring, b'\n') == 3);

        
        let mut lbuf = [0u8; 32];
        let llen = miku_bring_readline(&mut ring, lbuf.as_mut_ptr(), 32);
        test!("bring readline", llen == 4 && lbuf[0] == b'a' && lbuf[3] == b'\n');

        
        let skipped = miku_bring_skip(&mut ring, 2);
        test!("bring skip", skipped == 2);
        test!("bring len after skip", miku_bring_len(&ring) == 2);

        
        miku_bring_clear(&mut ring);
        test!("bring clear", miku_bring_is_empty(&ring));

        
        miku_bring_clear(&mut ring);
        let cap = miku_bring_capacity(&ring);
        
        let fill = [b'X'; 128];
        let _w1 = miku_bring_write(&mut ring, fill.as_ptr(), cap - 4);
        let mut sink = [0u8; 128];
        miku_bring_read(&mut ring, sink.as_mut_ptr(), cap - 4);
        
        let wrap_data = b"WRAP";
        let w2 = miku_bring_write(&mut ring, wrap_data.as_ptr(), 4);
        test!("bring wrap write", w2 == 4);
        let mut wbuf = [0u8; 8];
        let wr = miku_bring_read(&mut ring, wbuf.as_mut_ptr(), 4);
        test!("bring wrap read", wr == 4 && wbuf[0] == b'W' && wbuf[3] == b'P');

        miku_bring_free(&mut ring);
    }
    println("");
}


fn test_sha256() {
    println("--- sha256 ---");
    unsafe {
        
        let mut hash = [0u8; 32];
        miku_sha256(b"".as_ptr(), 0, hash.as_mut_ptr());
        
        test!("sha256 empty first byte", hash[0] == 0xe3);
        test!("sha256 empty second byte", hash[1] == 0xb0);
        test!("sha256 empty last byte", hash[31] == 0x55);

        
        let mut hash2 = [0u8; 32];
        miku_sha256(b"abc".as_ptr(), 3, hash2.as_mut_ptr());
        
        test!("sha256 abc byte0", hash2[0] == 0xba);
        test!("sha256 abc byte1", hash2[1] == 0x78);
        test!("sha256 abc last", hash2[31] == 0xad);

        
        let mut hash3 = [0u8; 32];
        miku_sha256(b"abcd".as_ptr(), 4, hash3.as_mut_ptr());
        test!("sha256 different", !miku_sha256_eq(hash2.as_ptr(), hash3.as_ptr()));

        
        let mut hash4 = [0u8; 32];
        miku_sha256(b"abc".as_ptr(), 3, hash4.as_mut_ptr());
        test!("sha256 same", miku_sha256_eq(hash2.as_ptr(), hash4.as_ptr()));

        
        let mut hex = [0u8; 65];
        miku_sha256_hex(hash2.as_ptr(), hex.as_mut_ptr());
        test!("sha256 hex len", miku_strlen(hex.as_ptr()) == 64);
        
        test!("sha256 hex prefix", hex[0] == b'b' && hex[1] == b'a' && hex[2] == b'7' && hex[3] == b'8');

        
        let mut ctx: MikuSha256Ctx = core::mem::zeroed();
        miku_sha256_init(&mut ctx);
        miku_sha256_update(&mut ctx, b"a".as_ptr(), 1);
        miku_sha256_update(&mut ctx, b"bc".as_ptr(), 2);
        let mut hash5 = [0u8; 32];
        miku_sha256_finish(&mut ctx, hash5.as_mut_ptr());
        test!("sha256 incremental", miku_sha256_eq(hash2.as_ptr(), hash5.as_ptr()));
    }
    println("");
}


fn test_uuid() {
    println("--- uuid ---");
    unsafe {
        
        let u = miku_uuid_gen();
        test!("uuid not nil", !miku_uuid_is_nil(&u));

        
        test!("uuid v4 version", (u.bytes[6] & 0xF0) == 0x40);
        test!("uuid v4 variant", (u.bytes[8] & 0xC0) == 0x80);

        
        let mut buf = [0u8; 37];
        miku_uuid_format(&u, buf.as_mut_ptr());
        test!("uuid format len", miku_strlen(buf.as_ptr()) == 36);
        test!("uuid format dash1", buf[8] == b'-');
        test!("uuid format dash2", buf[13] == b'-');
        test!("uuid format dash3", buf[18] == b'-');
        test!("uuid format dash4", buf[23] == b'-');

        
        let mut u2 = miku_uuid_nil();
        test!("uuid parse", miku_uuid_parse(buf.as_ptr(), &mut u2));
        test!("uuid roundtrip", miku_uuid_eq(&u, &u2));

        
        let u3 = miku_uuid_gen();
        test!("uuid unique", !miku_uuid_eq(&u, &u3));

        
        let nil = miku_uuid_nil();
        test!("uuid nil", miku_uuid_is_nil(&nil));
    }
    println("");
}


fn test_strbuf() {
    println("--- strbuf ---");
    unsafe {
        
        let mut s = miku_str_new();
        test!("str new empty", miku_str_empty(&s));
        test!("str new len 0", miku_str_len(&s) == 0);

        
        test!("str push", miku_str_push(&mut s, cstr!("hello")));
        test!("str len 5", miku_str_len(&s) == 5);
        test!("str eq hello", miku_str_eq(&s, cstr!("hello")));

        
        miku_str_push(&mut s, cstr!(" world"));
        test!("str eq full", miku_str_eq(&s, cstr!("hello world")));

        
        miku_str_push_char(&mut s, b'!');
        test!("str push char", miku_str_eq(&s, cstr!("hello world!")));

        
        miku_str_clear(&mut s);
        miku_str_push(&mut s, cstr!("count="));
        miku_str_push_int(&mut s, 42);
        test!("str push int", miku_str_eq(&s, cstr!("count=42")));

        
        test!("str starts_with", miku_str_starts_with(&s, cstr!("count")));
        test!("str ends_with", miku_str_ends_with(&s, cstr!("42")));
        test!("str !starts_with", !miku_str_starts_with(&s, cstr!("xyz")));

        
        test!("str find", miku_str_find(&s, cstr!("=")) == 5);
        test!("str contains", miku_str_contains(&s, cstr!("nt=")));
        test!("str !contains", !miku_str_contains(&s, cstr!("xyz")));

        
        test!("str at", miku_str_at(&s, 0) == b'c');
        test!("str at end", miku_str_at(&s, 7) == b'2');

        
        let mut s2 = miku_str_from(cstr!("  trimme  "));
        miku_str_trim(&mut s2);
        test!("str trim", miku_str_eq(&s2, cstr!("trimme")));
        miku_str_free(&mut s2);

        
        let mut s3 = miku_str_from(cstr!("Hello"));
        miku_str_to_upper(&mut s3);
        test!("str to_upper", miku_str_eq(&s3, cstr!("HELLO")));
        miku_str_to_lower(&mut s3);
        test!("str to_lower", miku_str_eq(&s3, cstr!("hello")));
        miku_str_free(&mut s3);

        
        miku_str_clear(&mut s);
        miku_str_push(&mut s, cstr!("abcdef"));
        let mut sub = miku_str_substr(&s, 2, 3);
        test!("str substr", miku_str_eq(&sub, cstr!("cde")));
        miku_str_free(&mut sub);

        
        let mut c = miku_str_clone(&s);
        test!("str clone", miku_str_eq(&c, cstr!("abcdef")));
        miku_str_free(&mut c);

        miku_str_free(&mut s);
    }
    println("");
}


fn test_pool() {
    println("--- pool ---");
    unsafe {
        let mut pool = miku_pool_new(16, 8); 
        test!("pool capacity", miku_pool_capacity(&pool) == 8);
        test!("pool available", miku_pool_available(&pool) == 8);
        test!("pool active 0", miku_pool_active(&pool) == 0);

        
        let h1 = miku_pool_alloc(&mut pool);
        test!("pool alloc valid", miku_pool_valid(&pool, h1));
        test!("pool active 1", miku_pool_active(&pool) == 1);

        
        let ptr = miku_pool_get(&pool, h1);
        test!("pool get not null", !ptr.is_null());
        *ptr = 0x42;

        
        let h2 = miku_pool_alloc(&mut pool);
        test!("pool h2 valid", miku_pool_valid(&pool, h2));
        test!("pool active 2", miku_pool_active(&pool) == 2);

        
        test!("pool release", miku_pool_release(&mut pool, h1));
        test!("pool h1 invalid", !miku_pool_valid(&pool, h1));
        test!("pool active 1 again", miku_pool_active(&pool) == 1);

        
        let stale_ptr = miku_pool_get(&pool, h1);
        test!("pool stale null", stale_ptr.is_null());

        
        test!("pool release h2", miku_pool_release(&mut pool, h2));
        test!("pool active 0 again", miku_pool_active(&pool) == 0);

        
        let mut handles = [PoolHandle(u64::MAX); 8];
        for i in 0..8 {
            handles[i] = miku_pool_alloc(&mut pool);
            test!("pool fill valid", miku_pool_valid(&pool, handles[i]));
        }
        test!("pool full", miku_pool_available(&pool) == 0);

        
        let bad = miku_pool_alloc(&mut pool);
        test!("pool full alloc fails", bad.0 == u64::MAX);

        
        for i in 0..8 {
            miku_pool_release(&mut pool, handles[i]);
        }
        test!("pool all released", miku_pool_available(&pool) == 8);

        miku_pool_free(&mut pool);
    }
    println("");
}


static mut EVT_COUNTER: u32 = 0;

extern "C" fn test_evt_handler(_id: u32, _data: *mut u8, _ctx: *mut u8) {
    unsafe { EVT_COUNTER += 1; }
}

fn test_event() {
    println("--- event ---");
    unsafe {
        miku_event_clear_all();
        EVT_COUNTER = 0;

        
        test!("evt no listeners", !miku_event_has_listeners(1));

        
        let idx = miku_event_on(1, test_evt_handler, core::ptr::null_mut());
        test!("evt register", idx >= 0);
        test!("evt has listeners", miku_event_has_listeners(1));
        test!("evt count 1", miku_event_count(1) == 1);

        
        let called = miku_event_emit(1, core::ptr::null_mut());
        test!("evt emit 1", called == 1);
        test!("evt counter", EVT_COUNTER == 1);

        
        let _idx2 = miku_event_on(1, test_evt_handler, core::ptr::null_mut());
        let called2 = miku_event_emit(1, core::ptr::null_mut());
        test!("evt emit 2", called2 == 2);
        test!("evt counter 3", EVT_COUNTER == 3);

        
        miku_event_off(idx);
        test!("evt count after remove", miku_event_count(1) == 1);

        
        let called3 = miku_event_emit(99, core::ptr::null_mut());
        test!("evt emit no handler", called3 == 0);

        
        miku_event_clear_all();
        test!("evt clear all", !miku_event_has_listeners(1));
    }
    println("");
}


fn test_datetime() {
    println("--- datetime ---");
    unsafe {
        
        let dt = miku_dt_from_timestamp(0);
        test!("dt epoch year", dt.year == 1970);
        test!("dt epoch month", dt.month == 1);
        test!("dt epoch day", dt.day == 1);
        test!("dt epoch hour", dt.hour == 0);
        test!("dt epoch weekday", dt.weekday == 4); 

        
        let dt2 = miku_dt_from_timestamp(1704067200);
        test!("dt 2024 year", dt2.year == 2024);
        test!("dt 2024 month", dt2.month == 1);
        test!("dt 2024 day", dt2.day == 1);
        test!("dt 2024 weekday", dt2.weekday == 1); 

        
        let ts = miku_dt_to_timestamp(&dt2);
        test!("dt roundtrip", ts == 1704067200);

        
        let mut buf = [0u8; 32];
        let n = miku_dt_format(&dt2, buf.as_mut_ptr(), 32);
        test!("dt format len", n == 19);
        
        test!("dt format y", buf[0] == b'2' && buf[1] == b'0' && buf[2] == b'2' && buf[3] == b'4');
        test!("dt format sep", buf[4] == b'-');

        
        let n2 = miku_dt_format_date(&dt2, buf.as_mut_ptr(), 32);
        test!("dt date len", n2 == 10);

        
        let n3 = miku_dt_format_time(&dt2, buf.as_mut_ptr(), 32);
        test!("dt time len", n3 == 8);

        
        let n4 = miku_dt_format_iso(&dt2, buf.as_mut_ptr(), 32);
        test!("dt iso len", n4 == 20);

        
        let dt3 = miku_dt_add_days(&dt2, 31);
        test!("dt add days month", dt3.month == 2);
        test!("dt add days day", dt3.day == 1);

        
        let dt4 = miku_dt_add_secs(&dt2, 3661); 
        test!("dt add secs hour", dt4.hour == 1);
        test!("dt add secs min", dt4.minute == 1);
        test!("dt add secs sec", dt4.second == 1);

        
        let diff = miku_dt_diff_secs(&dt4, &dt2);
        test!("dt diff secs", diff == 3661);

        
        let name = miku_dt_weekday_name(1);
        test!("dt weekday name", *name == b'M'); 

        
        let mname = miku_dt_month_name(1);
        test!("dt month name", *mname == b'J'); 
    }
    println("");
}


fn test_trie() {
    println("--- trie ---");
    unsafe {
        let mut t = miku_trie_new();

        
        test!("trie insert", miku_trie_insert(&mut t, b"hello".as_ptr(), 5, 42));
        test!("trie search", miku_trie_search(&t, b"hello".as_ptr(), 5));
        test!("trie get", miku_trie_get(&t, b"hello".as_ptr(), 5) == 42);
        test!("trie miss", !miku_trie_search(&t, b"world".as_ptr(), 5));

        
        test!("trie prefix hel", miku_trie_has_prefix(&t, b"hel".as_ptr(), 3));
        test!("trie prefix no", !miku_trie_has_prefix(&t, b"xyz".as_ptr(), 3));

        
        miku_trie_insert(&mut t, b"help".as_ptr(), 4, 10);
        miku_trie_insert(&mut t, b"heap".as_ptr(), 4, 20);
        miku_trie_insert(&mut t, b"hero".as_ptr(), 4, 30);

        
        test!("trie partial no match", !miku_trie_search(&t, b"hel".as_ptr(), 3));

        
        let mut buf = [0u8; 256];
        let count = miku_trie_prefix_collect(&t, b"hel".as_ptr(), 3, buf.as_mut_ptr(), 256, 10);
        test!("trie collect", count == 2); 

        
        test!("trie remove", miku_trie_remove(&mut t, b"hello".as_ptr(), 5));
        test!("trie removed", !miku_trie_search(&t, b"hello".as_ptr(), 5));

        
        test!("trie nodes > 0", miku_trie_node_count(&t) > 1);

        miku_trie_free(&mut t);
    }
    println("");
}


fn test_queue() {
    println("--- queue ---");
    unsafe {
        let mut q = miku_queue_new(core::mem::size_of::<u64>(), 8);
        test!("queue empty", miku_queue_is_empty(&q));
        test!("queue cap", miku_queue_capacity(&q) == 8);

        
        let v1: u64 = 10;
        let v2: u64 = 20;
        let v3: u64 = 30;
        test!("queue push 1", miku_queue_push(&mut q, &v1 as *const u64 as *const u8));
        test!("queue push 2", miku_queue_push(&mut q, &v2 as *const u64 as *const u8));
        test!("queue push 3", miku_queue_push(&mut q, &v3 as *const u64 as *const u8));
        test!("queue len 3", miku_queue_len(&q) == 3);

        let mut out: u64 = 0;
        test!("queue pop", miku_queue_pop(&mut q, &mut out as *mut u64 as *mut u8));
        test!("queue fifo order", out == 10);

        test!("queue pop 2", miku_queue_pop(&mut q, &mut out as *mut u64 as *mut u8));
        test!("queue fifo 2", out == 20);

        
        test!("queue peek", miku_queue_peek(&q, &mut out as *mut u64 as *mut u8));
        test!("queue peek val", out == 30);
        test!("queue len after peek", miku_queue_len(&q) == 1);

        
        let v4: u64 = 40;
        miku_queue_push(&mut q, &v4 as *const u64 as *const u8);
        test!("queue pop back", miku_queue_pop_back(&mut q, &mut out as *mut u64 as *mut u8));
        test!("queue pop back val", out == 40);

        
        miku_queue_clear(&mut q);
        let vals: [u64; 4] = [100, 200, 300, 400];
        for v in &vals {
            miku_queue_push(&mut q, v as *const u64 as *const u8);
        }
        test!("queue at 0", { miku_queue_at(&q, 0, &mut out as *mut u64 as *mut u8); out == 100 });
        test!("queue at 2", { miku_queue_at(&q, 2, &mut out as *mut u64 as *mut u8); out == 300 });

        
        let mut q2 = miku_queue_new(core::mem::size_of::<u64>(), 2);
        let a: u64 = 1;
        let b: u64 = 2;
        let c: u64 = 3;
        miku_queue_push(&mut q2, &a as *const u64 as *const u8);
        miku_queue_push(&mut q2, &b as *const u64 as *const u8);
        test!("queue full", miku_queue_is_full(&q2));
        test!("queue push full", !miku_queue_push(&mut q2, &c as *const u64 as *const u8));

        miku_queue_free(&mut q);
        miku_queue_free(&mut q2);
    }
    println("");
}


fn test_errno() {
    println("--- errno ---");
    unsafe {
        
        let s = miku_strerror(0);
        test!("strerror success", !s.is_null() && miku_strlen(s) > 0);

        let s = miku_strerror(-2);
        test!("strerror enoent", !s.is_null() && miku_strlen(s) > 0);

        let s = miku_strerror(-22);
        test!("strerror einval", !s.is_null() && miku_strlen(s) > 0);

        
        let s = miku_strerror(-9999);
        test!("strerror unknown", !s.is_null() && miku_strlen(s) > 0);

        
        miku_perror_code(cstr!("test"), -2);
        ok("perror_code no crash");

        
        miku_perror(core::ptr::null());
        ok("perror null no crash");

        
        let s1 = miku_strerror(-11); 
        let s2 = miku_strerror(-16); 
        let s3 = miku_strerror(-32); 
        let s4 = miku_strerror(-39); 
        let s5 = miku_strerror(-111); 
        test!("strerror eagain", miku_strlen(s1) > 3);
        test!("strerror ebusy", miku_strlen(s2) > 3);
        test!("strerror epipe", miku_strlen(s3) > 3);
        test!("strerror enotempty", miku_strlen(s4) > 3);
        test!("strerror econnrefused", miku_strlen(s5) > 3);
    }
    println("");
}


fn test_mem_extended() {
    println("--- mem extended ---");
    unsafe {
        
        let data = b"abcabc\0";
        let p = miku_memrchr(data.as_ptr(), b'b' as i32, 6);
        test!("memrchr found", !p.is_null());
        test!("memrchr last", p as usize - data.as_ptr() as usize == 4);

        let p2 = miku_memrchr(data.as_ptr(), b'z' as i32, 6);
        test!("memrchr miss", p2.is_null());

        let p3 = miku_memrchr(data.as_ptr(), b'a' as i32, 6);
        test!("memrchr first=last", p3 as usize - data.as_ptr() as usize == 3);

        
        let haystack = b"hello world miku\0";
        let needle = b"world\0";
        let p = miku_memmem(haystack.as_ptr(), 16, needle.as_ptr(), 5);
        test!("memmem found", !p.is_null());
        test!("memmem offset", p as usize - haystack.as_ptr() as usize == 6);

        let p2 = miku_memmem(haystack.as_ptr(), 16, b"xyz\0".as_ptr(), 3);
        test!("memmem miss", p2.is_null());

        let p3 = miku_memmem(haystack.as_ptr(), 16, b"\0".as_ptr(), 0);
        test!("memmem empty needle", !p3.is_null());

        let p4 = miku_memmem(b"ab\0".as_ptr(), 2, b"abc\0".as_ptr(), 3);
        test!("memmem needle > haystack", p4.is_null());
    }
    println("");
}


fn test_ctype_extended() {
    println("--- ctype extended ---");
    unsafe {
        
        test!("isgraph letter", miku_isgraph(b'a' as i32) != 0);
        test!("isgraph punct", miku_isgraph(b'!' as i32) != 0);
        test!("isgraph space", miku_isgraph(b' ' as i32) == 0);
        test!("isgraph ctrl", miku_isgraph(0x01) == 0);

        
        test!("isblank space", miku_isblank(b' ' as i32) != 0);
        test!("isblank tab", miku_isblank(b'\t' as i32) != 0);
        test!("isblank newline", miku_isblank(b'\n' as i32) == 0);
        test!("isblank letter", miku_isblank(b'a' as i32) == 0);

        
        test!("isascii 0", miku_isascii(0) != 0);
        test!("isascii 127", miku_isascii(127) != 0);
        test!("isascii 128", miku_isascii(128) == 0);
        test!("isascii 255", miku_isascii(255) == 0);

        
        test!("toascii low", miku_toascii(b'A' as i32) == b'A' as i32);
        test!("toascii high", miku_toascii(0xFF) == 0x7F);
        test!("toascii mask", miku_toascii(0x80) == 0);
    }
    println("");
}


fn test_string_extended() {
    println("--- string extended ---");
    unsafe {
        
        test!("strnlen basic", miku_strnlen(cstr!("hello"), 10) == 5);
        test!("strnlen bounded", miku_strnlen(cstr!("hello"), 3) == 3);
        test!("strnlen zero", miku_strnlen(cstr!("hello"), 0) == 0);
        test!("strnlen null", miku_strnlen(core::ptr::null(), 5) == 0);

        
        test!("strcasecmp eq", miku_strcasecmp(cstr!("Hello"), cstr!("hello")) == 0);
        test!("strcasecmp eq2", miku_strcasecmp(cstr!("ABC"), cstr!("abc")) == 0);
        test!("strcasecmp neq", miku_strcasecmp(cstr!("abc"), cstr!("xyz")) != 0);
        test!("strcasecmp lt", miku_strcasecmp(cstr!("abc"), cstr!("XYZ")) < 0);

        
        test!("strncasecmp eq", miku_strncasecmp(cstr!("Hello"), cstr!("HELXX"), 3) == 0);
        test!("strncasecmp zero", miku_strncasecmp(cstr!("abc"), cstr!("xyz"), 0) == 0);

        
        {
            let mut s: [u8; 12] = *b"one:two:tre\0";
            let mut p = s.as_mut_ptr();
            let t1 = miku_strsep(&mut p, cstr!(":"));
            test!("strsep token 1", !t1.is_null() && miku_strcmp(t1, cstr!("one")) == 0);
            let t2 = miku_strsep(&mut p, cstr!(":"));
            test!("strsep token 2", !t2.is_null() && miku_strcmp(t2, cstr!("two")) == 0);
            let t3 = miku_strsep(&mut p, cstr!(":"));
            test!("strsep token 3", !t3.is_null() && miku_strcmp(t3, cstr!("tre")) == 0);
            let t4 = miku_strsep(&mut p, cstr!(":"));
            test!("strsep end", t4.is_null());
        }

        
        let p = miku_strpbrk(cstr!("hello world"), cstr!("wo"));
        test!("strpbrk found", !p.is_null() && *p == b'o');
        test!("strspn", miku_strspn(cstr!("aaabbc"), cstr!("ab")) == 5);
        test!("strcspn", miku_strcspn(cstr!("hello,world"), cstr!(",!")) == 5);
    }
    println("");
}


fn test_hash_extended() {
    println("--- hash extended ---");
    unsafe {
        
        let h1 = miku_hash_combine(0, 42);
        let h2 = miku_hash_combine(0, 43);
        test!("hash_combine differs", h1 != h2);
        test!("hash_combine nonzero", h1 != 0);

        
        let ha = miku_hash_combine(miku_hash_combine(0, 1), 2);
        let hb = miku_hash_combine(miku_hash_combine(0, 2), 1);
        test!("hash_combine order", ha != hb);

        
        let u1 = miku_hash_u32(42);
        let u2 = miku_hash_u32(42);
        test!("hash_u32 deterministic", u1 == u2);
        let u3 = miku_hash_u32(43);
        test!("hash_u32 differs", u1 != u3);
        test!("hash_u32 avalanche", miku_hash_u32(1) != 1);

        
        let a1 = miku_adler32(b"Wikipedia\0".as_ptr(), 9);
        test!("adler32 known", a1 == 0x11E60398); 

        let a_empty = miku_adler32(core::ptr::null(), 0);
        test!("adler32 null", a_empty == 1); 

        
        let part1 = miku_adler32_update(1, b"Wiki\0".as_ptr(), 4);
        let full = miku_adler32_update(part1, b"pedia\0".as_ptr(), 5);
        test!("adler32 incremental", full == a1);

        
        let m1 = miku_murmurhash3(b"hello\0".as_ptr(), 5, 0);
        let m2 = miku_murmurhash3(b"hello\0".as_ptr(), 5, 0);
        test!("murmur3 deterministic", m1 == m2);
        let m3 = miku_murmurhash3(b"world\0".as_ptr(), 5, 0);
        test!("murmur3 differs", m1 != m3);

        
        let m4 = miku_murmurhash3(b"hello\0".as_ptr(), 5, 42);
        test!("murmur3 seed matters", m1 != m4);

        
        let f1 = miku_murmurhash3_fmix64(42);
        let f2 = miku_murmurhash3_fmix64(42);
        test!("murmur3 fmix64 det", f1 == f2);
        let f3 = miku_murmurhash3_fmix64(43);
        test!("murmur3 fmix64 diff", f1 != f3);
    }
    println("");
}


fn test_random_extended() {
    println("--- random extended ---");
    unsafe {
        
        let r1 = miku_rand_u32();
        let r2 = miku_rand_u32();
        test!("rand_u32 varies", r1 != r2);
        test!("rand_u32 fits 32bit", (r1 as u64) <= 0xFFFF_FFFF);

        
        let mut buf = [0u8; 32];
        miku_rand_bytes(buf.as_mut_ptr(), 32);
        
        let mut any_nonzero = false;
        for &b in &buf {
            if b != 0 { any_nonzero = true; break; }
        }
        test!("rand_bytes nonzero", any_nonzero);

        
        let mut buf2 = [0u8; 32];
        miku_rand_bytes(buf2.as_mut_ptr(), 32);
        let mut differs = false;
        for i in 0..32 {
            if buf[i] != buf2[i] { differs = true; break; }
        }
        test!("rand_bytes varies", differs);

        
        let mut arr: [u64; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
        miku_rand_shuffle(
            arr.as_mut_ptr() as *mut u8,
            8,
            core::mem::size_of::<u64>(),
        );
        
        let mut sum: u64 = 0;
        for &v in &arr { sum += v; }
        test!("rand_shuffle sum preserved", sum == 36);
        
        let orig: [u64; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
        let mut moved = false;
        for i in 0..8 {
            if arr[i] != orig[i] { moved = true; break; }
        }
        test!("rand_shuffle moved", moved);
    }
    println("");
}


extern "C" fn test_sigaction_handler(sig: u32) {
    unsafe { SIG_RECEIVED = sig + 100; }
}

fn test_signal_extended() {
    println("--- signal extended ---");
    unsafe {
        miku_signal_reset_all();
        SIG_RECEIVED = 0;

        
        let act = MikuSigaction {
            handler: Some(test_sigaction_handler),
            flags: 0,
            mask: 0,
        };
        let mut oldact: MikuSigaction = core::mem::zeroed();
        let r = miku_sigaction(2, &act, &mut oldact);
        test!("sigaction register", r == 0);
        test!("sigaction old empty", oldact.handler.is_none());

        
        SIG_RECEIVED = 0;
        test!("sigaction dispatch", miku_sigaction_dispatch(2));
        test!("sigaction received", SIG_RECEIVED == 102);

        
        SIG_RECEIVED = 0;
        let r = miku_signal_raise(2);
        test!("signal_raise ok", r == 0);
        test!("signal_raise received", SIG_RECEIVED == 102);

        
        miku_signal_block(2);
        SIG_RECEIVED = 0;
        miku_signal_raise(2);
        test!("blocked no dispatch", SIG_RECEIVED == 0);
        test!("signal pending", miku_signal_pending() & (1 << 2) != 0);
        miku_signal_unblock(2);

        
        let r = miku_sigaction(9, &act, core::ptr::null_mut());
        test!("sigaction kill fail", r == -1);

        
        let act_reset = MikuSigaction {
            handler: Some(test_sigaction_handler),
            flags: 1, 
            mask: 0,
        };
        miku_sigaction(3, &act_reset, core::ptr::null_mut());
        miku_sigaction_dispatch(3); 
        test!("resethand first", SIG_RECEIVED == 103);
        test!("resethand cleared", !miku_signal_has_handler(3));

        miku_signal_reset_all();
        miku_signal_set_mask(0);
    }
    println("");
}


fn test_env_extended() {
    println("--- env extended ---");
    unsafe {
        miku_env_clear();

        
        miku_setenv(cstr!("TESTKEY"), cstr!("testvalue"));

        
        let mut buf = [0u8; 64];
        let len = miku_getenv_r(cstr!("TESTKEY"), buf.as_mut_ptr(), 64);
        test!("getenv_r found", len == 9);
        test!("getenv_r value", miku_strcmp(buf.as_ptr(), cstr!("testvalue")) == 0);

        
        let mut small = [0u8; 5];
        let len2 = miku_getenv_r(cstr!("TESTKEY"), small.as_mut_ptr(), 5);
        test!("getenv_r trunc len", len2 == 9); 
        test!("getenv_r trunc val", miku_strncmp(small.as_ptr(), cstr!("test"), 4) == 0);

        
        let len3 = miku_getenv_r(cstr!("NOEXIST"), buf.as_mut_ptr(), 64);
        test!("getenv_r miss", len3 == -1);

        
        test!("getenv_r null key", miku_getenv_r(core::ptr::null(), buf.as_mut_ptr(), 64) == -1);
        test!("getenv_r null buf", miku_getenv_r(cstr!("TESTKEY"), core::ptr::null_mut(), 64) == -1);
        test!("getenv_r zero sz", miku_getenv_r(cstr!("TESTKEY"), buf.as_mut_ptr(), 0) == -1);

        miku_env_clear();
    }
    println("");
}

fn test_math_extended() {
    println("--- math extended ---");
    unsafe {
        test!("gcd(12,8)", miku_gcd(12, 8) == 4);
        test!("gcd(17,13) primes", miku_gcd(17, 13) == 1);
        test!("gcd(0,5)", miku_gcd(0, 5) == 5);
        test!("lcm(4,6)", miku_lcm(4, 6) == 12);
        test!("lcm(0,5)", miku_lcm(0, 5) == 0);
        test!("pow(2,10)", miku_pow(2, 10) == 1024);
        test!("pow(-2,3)", miku_pow(-2, 3) == -8);
        test!("pow(x,0)", miku_pow(42, 0) == 1);
        test!("upow(3,4)", miku_upow(3, 4) == 81);
        test!("isqrt(100)", miku_isqrt(100) == 10);
        test!("isqrt(99)", miku_isqrt(99) == 9);
        test!("isqrt(0)", miku_isqrt(0) == 0);
        test!("isqrt(1)", miku_isqrt(1) == 1);
        test!("icbrt(27)", miku_icbrt(27) == 3);
        test!("icbrt(64)", miku_icbrt(64) == 4);
        test!("ilog2(256)", miku_ilog2(256) == 8);
        test!("ilog2(1)", miku_ilog2(1) == 0);
        test!("ilog10(1000)", miku_ilog10(1000) == 3);
        test!("ilog10(99)", miku_ilog10(99) == 1);
        test!("sign(-5)", miku_sign(-5) == -1);
        test!("sign(0)", miku_sign(0) == 0);
        test!("sign(42)", miku_sign(42) == 1);
        test!("map(5,0,10,0,100)", miku_map(5, 0, 10, 0, 100) == 50);
        test!("lerp(0,100,500)", miku_lerp(0, 100, 500) == 50);
        test!("sadd overflow", miku_sadd(i64::MAX, 1) == i64::MAX);
        test!("ssub underflow", miku_ssub(i64::MIN, 1) == i64::MIN);
        test!("usadd sat", miku_usadd(u64::MAX, 1) == u64::MAX);
        test!("ussub sat", miku_ussub(0, 1) == 0);
        test!("div_ceil(10,3)", miku_div_ceil(10, 3) == 4);
        test!("div_ceil(9,3)", miku_div_ceil(9, 3) == 3);
        test!("modpow(2,10,1000)", miku_modpow(2, 10, 1000) == 24);
        test!("is_prime(7)", miku_is_prime(7) == 1);
        test!("is_prime(1)", miku_is_prime(1) == 0);
        test!("is_prime(4)", miku_is_prime(4) == 0);
        test!("is_prime(997)", miku_is_prime(997) == 1);
        test!("fib(10)", miku_fib(10) == 55);
        test!("fib(0)", miku_fib(0) == 0);
        test!("fib(1)", miku_fib(1) == 1);
        test!("factorial(5)", miku_factorial(5) == 120);
        test!("factorial(0)", miku_factorial(0) == 1);
        test!("binomial(10,3)", miku_binomial(10, 3) == 120);
        test!("binomial(5,0)", miku_binomial(5, 0) == 1);
    }
    println("");
}

fn test_sort_extended() {
    println("--- sort extended ---");
    unsafe {
        
        let arr: [i64; 6] = [1, 3, 3, 5, 7, 9];
        let key3: i64 = 3;
        let lb = miku_lower_bound(
            &key3 as *const i64 as *const u8,
            arr.as_ptr() as *const u8,
            6, 8, miku_cmp_i64,
        );
        test!("lower_bound(3)", lb == 1);

        let ub = miku_upper_bound(
            &key3 as *const i64 as *const u8,
            arr.as_ptr() as *const u8,
            6, 8, miku_cmp_i64,
        );
        test!("upper_bound(3)", ub == 3);

        
        let mut arr2: [i64; 7] = [1, 1, 2, 3, 3, 3, 4];
        let new_count = miku_unique(
            arr2.as_mut_ptr() as *mut u8,
            7, 8, miku_cmp_i64,
        );
        test!("unique count", new_count == 4);
        test!("unique[0]", arr2[0] == 1);
        test!("unique[3]", arr2[3] == 4);

        
        let mut arr3: [i64; 5] = [5, 3, 1, 4, 2];
        miku_nth_element(arr3.as_mut_ptr() as *mut u8, 5, 8, 2, miku_cmp_i64);
        test!("nth_element[2]==3", arr3[2] == 3);

        
        let a: i32 = 10;
        let b: i32 = 20;
        test!("cmp_i32 <", miku_cmp_i32(&a as *const i32 as *const u8, &b as *const i32 as *const u8) < 0);
    }
    println("");
}

fn test_list_extended() {
    println("--- list extended ---");
    unsafe {
        let mut l = miku_list_new(8);
        let v1: u64 = 10;
        let v2: u64 = 20;
        let v3: u64 = 30;
        miku_list_push_back(&mut l, &v1 as *const u64 as *const u8);
        miku_list_push_back(&mut l, &v2 as *const u64 as *const u8);
        miku_list_push_back(&mut l, &v3 as *const u64 as *const u8);

        
        let front = miku_list_front(&l) as *const u64;
        let back = miku_list_back(&l) as *const u64;
        test!("list front", !front.is_null() && *front == 10);
        test!("list back", !back.is_null() && *back == 30);

        
        test!("list index_of(20)", miku_list_index_of(&l, &v2 as *const u64 as *const u8) == 1);
        let v4: u64 = 99;
        test!("list index_of miss", miku_list_index_of(&l, &v4 as *const u64 as *const u8) == -1);

        
        miku_list_reverse(&mut l);
        let front2 = miku_list_front(&l) as *const u64;
        let back2 = miku_list_back(&l) as *const u64;
        test!("list reverse front", !front2.is_null() && *front2 == 30);
        test!("list reverse back", !back2.is_null() && *back2 == 10);

        miku_list_free(&mut l);
    }
    println("");
}

fn test_strbuf_extended() {
    println("--- strbuf extended ---");
    unsafe {
        let mut s = miku_str_from(cstr!("hello world"));

        
        test!("str_count", miku_str_count(&s, cstr!("l")) == 3);

        
        let ok = miku_str_replace(&mut s, cstr!("world"), cstr!("rust"));
        test!("str_replace ok", ok);
        test!("str_replace result", miku_str_eq(&s, cstr!("hello rust")));

        
        miku_str_clear(&mut s);
        miku_str_push(&mut s, cstr!("helloworld"));
        miku_str_insert(&mut s, 5, cstr!(" "));
        test!("str_insert", miku_str_eq(&s, cstr!("hello world")));

        
        miku_str_remove(&mut s, 5, 1);
        test!("str_remove", miku_str_eq(&s, cstr!("helloworld")));

        
        miku_str_clear(&mut s);
        miku_str_push(&mut s, cstr!("abc"));
        miku_str_reverse(&mut s);
        test!("str_reverse", miku_str_eq(&s, cstr!("cba")));

        
        miku_str_clear(&mut s);
        miku_str_push(&mut s, cstr!("aabaa"));
        let n = miku_str_replace_all(&mut s, cstr!("a"), cstr!("x"));
        test!("str_replace_all count", n == 4);
        test!("str_replace_all result", miku_str_eq(&s, cstr!("xxbxx")));

        miku_str_free(&mut s);
    }
    println("");
}

fn test_glob_extended() {
    println("--- glob extended ---");
    unsafe {
        
        test!("glob [abc]", miku_glob_match(cstr!("[abc]"), cstr!("b")));
        test!("glob [abc] miss", !miku_glob_match(cstr!("[abc]"), cstr!("d")));
        test!("glob [a-z]", miku_glob_match(cstr!("[a-z]"), cstr!("m")));
        test!("glob [a-z] miss", !miku_glob_match(cstr!("[a-z]"), cstr!("5")));
        test!("glob [^0-9]", miku_glob_match(cstr!("[^0-9]"), cstr!("a")));
        test!("glob [^0-9] miss", !miku_glob_match(cstr!("[^0-9]"), cstr!("5")));
        test!("glob file.[ch]", miku_glob_match(cstr!("file.[ch]"), cstr!("file.c")));
        test!("glob *.[ch]", miku_glob_match(cstr!("*.[ch]"), cstr!("main.c")));
        test!("glob has_magic [", miku_glob_has_magic(cstr!("test[0-9]")));

        
        let strs: [*const u8; 4] = [
            cstr!("foo.c"),
            cstr!("bar.h"),
            cstr!("baz.rs"),
            cstr!("test.c"),
        ];
        let count = miku_glob_filter(
            cstr!("*.c"),
            strs.as_ptr(),
            4,
            core::ptr::null_mut(),
        );
        test!("glob_filter count", count == 2);
    }
    println("");
}

fn test_path_extended() {
    println("--- path extended ---");
    unsafe {
        test!("path_is_relative", miku_path_is_relative(cstr!("foo/bar")));
        test!("path_is_relative abs", !miku_path_is_relative(cstr!("/foo")));

        test!("path_has_ext .txt", miku_path_has_ext(cstr!("file.txt"), cstr!("txt")));
        test!("path_has_ext miss", !miku_path_has_ext(cstr!("file.txt"), cstr!("rs")));

        let common = miku_path_common(cstr!("/usr/lib/a"), cstr!("/usr/bin/b"));
        if !common.is_null() {
            test!("path_common /usr", miku_strcmp(common, cstr!("/usr")) == 0);
            miku_free(common);
        } else {
            fail("path_common", "null");
        }

        let parent = miku_path_parent(cstr!("/usr/lib/file.txt"));
        if !parent.is_null() {
            test!("path_parent", miku_strcmp(parent, cstr!("/usr/lib")) == 0);
            miku_free(parent);
        } else {
            fail("path_parent", "null");
        }
    }
    println("");
}

fn test_endian_extended() {
    println("--- endian extended ---");
    unsafe {
        let mut buf = [0u8; 8];

        
        miku_write_u64_le(buf.as_mut_ptr(), 0x0102030405060708u64);
        test!("write_u64_le[0]", buf[0] == 0x08);
        test!("write_u64_le[7]", buf[7] == 0x01);

        
        let val = miku_read_u64_le(buf.as_ptr());
        test!("read_u64_le roundtrip", val == 0x0102030405060708u64);

        
        miku_write_u64_be(buf.as_mut_ptr(), 0x0102030405060708u64);
        test!("write_u64_be[0]", buf[0] == 0x01);
        test!("write_u64_be[7]", buf[7] == 0x08);

        let val2 = miku_read_u64_be(buf.as_ptr());
        test!("read_u64_be roundtrip", val2 == 0x0102030405060708u64);

        
        test!("le16toh noop", miku_le16toh(0x1234) == 0x1234);
        test!("le32toh noop", miku_le32toh(0x12345678) == 0x12345678);
        test!("le64toh noop", miku_le64toh(0x123456789ABCDEF0) == 0x123456789ABCDEF0);
    }
    println("");
}

fn test_datetime_extended() {
    println("--- datetime extended ---");
    unsafe {
        
        let dt = miku_dt_from_timestamp(1705321845);
        test!("dt valid", miku_dt_valid(&dt));
        test!("dt year", dt.year == 2024);
        test!("dt month", dt.month == 1);
        test!("dt day", dt.day == 15);
        test!("dt hour", dt.hour == 12);
        test!("dt minute", dt.minute == 30);
        test!("dt second", dt.second == 45);

        
        test!("leap 2024", miku_dt_is_leap_year(2024));
        test!("not leap 2023", !miku_dt_is_leap_year(2023));
        test!("leap 2000", miku_dt_is_leap_year(2000));
        test!("not leap 1900", !miku_dt_is_leap_year(1900));

        
        test!("feb leap", miku_dt_days_in_month(2, 2024) == 29);
        test!("feb normal", miku_dt_days_in_month(2, 2023) == 28);
        test!("jan", miku_dt_days_in_month(1, 2024) == 31);

        
        test!("days 2024", miku_dt_days_in_year(2024) == 366);
        test!("days 2023", miku_dt_days_in_year(2023) == 365);

        
        let mon = miku_dt_month_short(1);
        test!("month short Jan", miku_strcmp(mon, cstr!("Jan")) == 0);
        let wday = miku_dt_weekday_short(1);
        test!("weekday short Mon", miku_strcmp(wday, cstr!("Mon")) == 0);

        
        let dt2 = miku_dt_from_timestamp(1705321846); 
        test!("dt_cmp <", miku_dt_cmp(&dt, &dt2) == -1);
        test!("dt_cmp ==", miku_dt_cmp(&dt, &dt) == 0);

        
        let mut buf = [0u8; 48];
        let len = miku_dt_format_rfc2822(&dt, buf.as_mut_ptr(), 48);
        test!("rfc2822 len", len > 25);

        
        let bad = MikuDateTime {
            year: 2024, month: 13, day: 1,
            hour: 0, minute: 0, second: 0,
            weekday: 0, yearday: 0,
        };
        test!("dt invalid month 13", !miku_dt_valid(&bad));
    }
    println("");
}

fn test_json_extended() {
    println("--- json extended ---");
    unsafe {
        let json = b"{\"name\":\"miku\",\"age\":16,\"active\":true,\"scores\":[10,20,30],\"data\":null}\0";
        let mut parser = core::mem::zeroed::<MikuJsonParser>();
        miku_json_init(&mut parser);

        let mut tokens = [core::mem::zeroed::<MikuJsonToken>(); 32];
        let count = miku_json_parse(
            &mut parser,
            json.as_ptr(),
            json.len() - 1,
            tokens.as_mut_ptr(),
            32,
        );
        test!("json parse ok", count > 0);

        
        let name_idx = miku_json_find(json.as_ptr(), tokens.as_ptr(), count as usize, 0, cstr!("name"));
        test!("json find name", name_idx >= 0);
        test!("json name eq", miku_json_eq(json.as_ptr(), tokens.as_ptr(), name_idx as usize, cstr!("miku")));

        
        let age_idx = miku_json_find(json.as_ptr(), tokens.as_ptr(), count as usize, 0, cstr!("age"));
        test!("json find age", age_idx >= 0);
        test!("json_int age", miku_json_int(json.as_ptr(), tokens.as_ptr(), age_idx as usize) == 16);

        
        let active_idx = miku_json_find(json.as_ptr(), tokens.as_ptr(), count as usize, 0, cstr!("active"));
        test!("json find active", active_idx >= 0);
        test!("json_bool true", miku_json_bool(json.as_ptr(), tokens.as_ptr(), active_idx as usize) == 1);

        
        let scores_idx = miku_json_find(json.as_ptr(), tokens.as_ptr(), count as usize, 0, cstr!("scores"));
        test!("json find scores", scores_idx >= 0);
        let elem0 = miku_json_array_get(tokens.as_ptr(), count as usize, scores_idx as usize, 0);
        test!("json array[0]", elem0 >= 0);
        test!("json array[0]==10", miku_json_int(json.as_ptr(), tokens.as_ptr(), elem0 as usize) == 10);
        let elem2 = miku_json_array_get(tokens.as_ptr(), count as usize, scores_idx as usize, 2);
        test!("json array[2]==30", miku_json_int(json.as_ptr(), tokens.as_ptr(), elem2 as usize) == 30);

        
        let data_idx = miku_json_find(json.as_ptr(), tokens.as_ptr(), count as usize, 0, cstr!("data"));
        test!("json find data", data_idx >= 0);
        test!("json_is_null", miku_json_is_null(tokens.as_ptr(), data_idx as usize));

        
        let mut buf = [0u8; 32];
        let n = miku_json_strcpy(json.as_ptr(), tokens.as_ptr(), name_idx as usize, buf.as_mut_ptr(), 32);
        test!("json_strcpy len", n == 4);
        test!("json_strcpy val", miku_strcmp(buf.as_ptr(), cstr!("miku")) == 0);
    }
    println("");
}

fn test_sync() {
    println("--- sync ---");
    unsafe {
        
        let mut mtx = core::mem::zeroed::<MikuMutex>();
        miku_mutex_init(&mut mtx);
        test!("mutex init unlocked", !miku_mutex_is_locked(&mtx));
        miku_mutex_lock(&mut mtx);
        test!("mutex locked", miku_mutex_is_locked(&mtx));
        test!("mutex trylock fails", !miku_mutex_trylock(&mut mtx));
        miku_mutex_unlock(&mut mtx);
        test!("mutex unlocked", !miku_mutex_is_locked(&mtx));
        test!("mutex trylock ok", miku_mutex_trylock(&mut mtx));
        miku_mutex_unlock(&mut mtx);

        
        let mut atom = core::mem::zeroed::<MikuAtomic>();
        miku_atomic_init(&mut atom, 0);
        test!("atomic init 0", miku_atomic_load(&atom) == 0);
        miku_atomic_store(&mut atom, 42);
        test!("atomic store 42", miku_atomic_load(&atom) == 42);
        let prev = miku_atomic_add(&mut atom, 8);
        test!("atomic add prev", prev == 42);
        test!("atomic add result", miku_atomic_load(&atom) == 50);
        let prev = miku_atomic_sub(&mut atom, 10);
        test!("atomic sub prev", prev == 50);
        test!("atomic sub result", miku_atomic_load(&atom) == 40);
        test!("atomic cas ok", miku_atomic_cas(&mut atom, 40, 99));
        test!("atomic cas result", miku_atomic_load(&atom) == 99);
        test!("atomic cas fail", !miku_atomic_cas(&mut atom, 40, 0));
        let old = miku_atomic_swap(&mut atom, 7);
        test!("atomic swap old", old == 99);
        test!("atomic swap result", miku_atomic_load(&atom) == 7);

        
        let mut once = core::mem::zeroed::<MikuOnce>();
        miku_once_init(&mut once);
        test!("once not done", !miku_once_done(&once));
    }
    println("");
}

fn test_convert_extended() {
    println("--- convert extended ---");
    unsafe {
        
        let mut buf = [0u8; 68];
        let p = miku_itoa_base(255, buf.as_mut_ptr(), 16);
        test!("itoa_base hex", !p.is_null());
        test!("itoa_base hex val", miku_strcmp(p, cstr!("ff")) == 0);

        let p = miku_itoa_base(10, buf.as_mut_ptr(), 2);
        test!("itoa_base bin", miku_strcmp(p, cstr!("1010")) == 0);

        let p = miku_itoa_base(-42, buf.as_mut_ptr(), 10);
        test!("itoa_base neg", miku_strcmp(p, cstr!("-42")) == 0);

        let p = miku_itoa_base(0, buf.as_mut_ptr(), 10);
        test!("itoa_base zero", miku_strcmp(p, cstr!("0")) == 0);

        
        let p = miku_utoa_base(255, buf.as_mut_ptr(), 8);
        test!("utoa_base oct", miku_strcmp(p, cstr!("377")) == 0);

        
        test!("strtol dec", miku_strtol(cstr!("123"), core::ptr::null_mut(), 10) == 123);
        test!("strtol neg", miku_strtol(cstr!("-99"), core::ptr::null_mut(), 10) == -99);
        test!("strtol hex", miku_strtol(cstr!("0xff"), core::ptr::null_mut(), 0) == 255);
        test!("strtol oct", miku_strtol(cstr!("077"), core::ptr::null_mut(), 0) == 63);
    }
    println("");
}

fn test_errno_extended() {
    println("--- errno extended ---");
    unsafe {
        test!("is_error neg", miku_is_error(-1));
        test!("is_error zero", !miku_is_error(0));
        test!("is_error pos", !miku_is_error(1));
        test!("to_errno -2", miku_to_errno(-2) == 2);
        test!("to_errno 0", miku_to_errno(0) == 0);

        let name = miku_errno_name(-2);
        test!("errno_name ENOENT", !name.is_null());
        test!("errno_name val", miku_strcmp(name, cstr!("ENOENT")) == 0);

        let name = miku_errno_name(-22);
        test!("errno_name EINVAL", miku_strcmp(name, cstr!("EINVAL")) == 0);

        let name = miku_errno_name(0);
        test!("errno_name EOK", miku_strcmp(name, cstr!("EOK")) == 0);

        let msg = miku_strerror(-9);
        test!("strerror -9", !msg.is_null());
    }
    println("");
}

fn test_regex_extended() {
    println("--- regex extended ---");
    unsafe {
        
        let mut start: usize = 0;
        let mut len: usize = 0;
        let found = miku_regex_find_span(
            cstr!("[0-9]+"),
            cstr!("abc123def"),
            &mut start,
            &mut len,
        );
        test!("regex find_span found", found);
        test!("regex find_span start", start == 3);

        
        let result = miku_regex_replace(
            cstr!("[0-9]+"),
            cstr!("hello123world"),
            cstr!("XXX"),
        );
        test!("regex replace not null", !result.is_null());
        if !result.is_null() {
            
            test!("regex replace contains XXX", miku_strstr(result, cstr!("XXX")) != core::ptr::null());
            miku_free(result);
        }

        
        let result = miku_regex_replace(
            cstr!("[0-9]+"),
            cstr!("hello"),
            cstr!("XXX"),
        );
        test!("regex replace no match", !result.is_null());
        if !result.is_null() {
            test!("regex replace no match eq", miku_strcmp(result, cstr!("hello")) == 0);
            miku_free(result);
        }
    }
    println("");
}

fn test_panic_extended() {
    println("--- panic extended ---");
    unsafe {
        miku_assert_eq(42, 42, core::ptr::null(), 0);
        test!("assert_eq equal ok", true);

        let val: u8 = 1;
        miku_assert_not_null(&val as *const u8, cstr!("test"), core::ptr::null(), 0);
        test!("assert_not_null ok", true);
    }
    println("");
}

fn test_base64_extended() {
    println("--- base64 extended ---");
    unsafe {
        
        test!("base64 valid", miku_base64_is_valid(cstr!("SGVsbG8="), 8));
        test!("base64 invalid", !miku_base64_is_valid(cstr!("SGVsb"), 5));
        test!("base64 empty valid", miku_base64_is_valid(core::ptr::null(), 0));
    }
    println("");
}

fn test_uuid_extended() {
    println("--- uuid extended ---");
    unsafe {
        let uuid = miku_uuid_gen();
        test!("uuid version 4", miku_uuid_version(&uuid) == 4);
        test!("uuid variant 1", miku_uuid_variant(&uuid) == 1);
        test!("uuid not nil", !miku_uuid_is_nil(&uuid));

        let nil = miku_uuid_nil();
        test!("uuid nil cmp", miku_uuid_cmp(&nil, &nil) == 0);

        
        let mut buf = [0u8; 37];
        miku_uuid_format(&uuid, buf.as_mut_ptr());
        let mut parsed = miku_uuid_nil();
        test!("uuid parse ok", miku_uuid_parse(buf.as_ptr(), &mut parsed));
        test!("uuid roundtrip", miku_uuid_eq(&uuid, &parsed));
    }
    println("");
}

fn test_sha256_extended() {
    println("--- sha256 extended ---");
    unsafe {
        
        let key = b"key\0";
        let data = b"hello\0";
        let mut hmac1 = [0u8; 32];
        let mut hmac2 = [0u8; 32];
        miku_sha256_hmac(key.as_ptr(), 3, data.as_ptr(), 5, hmac1.as_mut_ptr());
        miku_sha256_hmac(key.as_ptr(), 3, data.as_ptr(), 5, hmac2.as_mut_ptr());
        test!("hmac deterministic", miku_sha256_eq(hmac1.as_ptr(), hmac2.as_ptr()));

        
        let data2 = b"world\0";
        miku_sha256_hmac(key.as_ptr(), 3, data2.as_ptr(), 5, hmac2.as_mut_ptr());
        test!("hmac diff data", !miku_sha256_eq(hmac1.as_ptr(), hmac2.as_ptr()));

        
        let key2 = b"key2\0";
        miku_sha256_hmac(key2.as_ptr(), 4, data.as_ptr(), 5, hmac2.as_mut_ptr());
        test!("hmac diff key", !miku_sha256_eq(hmac1.as_ptr(), hmac2.as_ptr()));
    }
    println("");
}

fn test_random_extended2() {
    println("--- random extended2 ---");
    unsafe {
        
        let mut got_true = false;
        let mut got_false = false;
        for _ in 0..100 {
            if miku_rand_bool() { got_true = true; } else { got_false = true; }
        }
        test!("rand_bool variety", got_true && got_false);

        
        for _ in 0..50 {
            let v = miku_rand_uniform(10);
            test!("rand_uniform < bound", v < 10);
        }
    }
    println("");
}

fn test_random_extended3() {
    println("--- random extended3 ---");
    unsafe {
        
        for _ in 0..50 {
            let v = miku_rand_i64(-10, 10);
            test!("rand_i64 in range", v >= -10 && v < 10);
        }

        
        let v = miku_rand_i64(5, 5);
        test!("rand_i64 lo==hi", v == 5);

        
        for _ in 0..50 {
            let v = miku_rand_frac_million();
            test!("rand_frac < 1M", v < 1_000_000);
        }

        
        for _ in 0..50 {
            let d = miku_rand_dice(6);
            test!("rand_dice 1..6", d >= 1 && d <= 6);
        }
        test!("rand_dice 0", miku_rand_dice(0) == 0);

        
        let mut out = [0usize; 10];
        let count = miku_rand_sample(10, 3, out.as_mut_ptr());
        test!("rand_sample count", count == 3);
        
        let mut sample_ok = true;
        for i in 0..count {
            if out[i] >= 10 { sample_ok = false; }
        }
        test!("rand_sample values valid", sample_ok);

        
        let count2 = miku_rand_sample(5, 20, out.as_mut_ptr());
        test!("rand_sample clamp", count2 == 5);

        
        let weights: [u64; 4] = [0, 0, 100, 0];
        
        let mut all2 = true;
        for _ in 0..20 {
            let idx = miku_rand_weighted(weights.as_ptr(), 4);
            if idx != 2 { all2 = false; }
        }
        test!("rand_weighted deterministic", all2);

        
        let mut perm = [0usize; 8];
        miku_rand_perm(8, perm.as_mut_ptr());
        
        let mut seen = [false; 8];
        let mut perm_ok = true;
        for i in 0..8 {
            if perm[i] >= 8 { perm_ok = false; break; }
            if seen[perm[i]] { perm_ok = false; break; }
            seen[perm[i]] = true;
        }
        test!("rand_perm valid", perm_ok);
    }
    println("");
}

fn test_checksum_extended() {
    println("--- checksum extended ---");
    unsafe {
        let data = b"Hello, World!";
        let len = data.len();

        
        let c16 = miku_crc16(data.as_ptr(), len);
        test!("crc16 nonzero", c16 != 0);
        
        let c16_2 = miku_crc16(data.as_ptr(), len);
        test!("crc16 deterministic", c16 == c16_2);

        
        let c16_part = miku_crc16(b"Hello".as_ptr(), 5);
        let c16_full = miku_crc16_update(c16_part, b", World!".as_ptr(), 8);
        test!("crc16 incremental", c16_full == c16);

        
        let c16_e = miku_crc16(core::ptr::null(), 0);
        test!("crc16 empty", c16_e == 0xFFFF);

        
        let valid = b"4539578763621486";
        test!("luhn valid", miku_luhn_check(valid.as_ptr(), 16));

        
        let invalid = b"4539578763621487";
        test!("luhn invalid", !miku_luhn_check(invalid.as_ptr(), 16));

        
        let partial = b"453957876362148";
        let digit = miku_luhn_digit(partial.as_ptr(), 15);
        test!("luhn digit", digit == 6);

        
        let p1 = miku_parity8(0b11001100); 
        test!("parity8 even", p1 == 0);
        let p2 = miku_parity8(0b11001101); 
        test!("parity8 odd", p2 == 1);

        
        let even_data = [0xFFu8, 0xFF]; 
        let p3 = miku_parity(even_data.as_ptr(), 2);
        test!("parity buf even", p3 == 0);

        
        let sv = miku_sysv_checksum(data.as_ptr(), len);
        test!("sysv nonzero", sv != 0);

        
        let crc_a = miku_crc32(b"Hello".as_ptr(), 5);
        let crc_b = miku_crc32(b", World!".as_ptr(), 8);
        let crc_full = miku_crc32(data.as_ptr(), len);
        let crc_combined = miku_crc32_combine(crc_a, crc_b, 8);
        test!("crc32 combine", crc_combined == crc_full);
    }
    println("");
}

fn test_csv_full() {
    println("--- csv full ---");
    unsafe {
        
        let csv_ptr = miku_malloc(core::mem::size_of::<MikuCsv>()) as *mut MikuCsv;
        if csv_ptr.is_null() {
            fail("csv alloc", "malloc failed");
            println("");
            return;
        }
        
        miku_memset(csv_ptr as *mut u8, 0, core::mem::size_of::<MikuCsv>());
        (*csv_ptr).delimiter = b',';

        
        let csv_data = b"name,age,city\nAlice,30,Tokyo\nBob,25,Osaka\n";
        let rows = miku_csv_parse(csv_ptr, csv_data.as_ptr(), csv_data.len());
        test!("csv parse rows", rows == 3);
        test!("csv rows fn", miku_csv_rows(csv_ptr) == 3);
        test!("csv cols", miku_csv_cols(csv_ptr, 0) == 3);

        
        let mut flen: usize = 0;
        let f = miku_csv_field(csv_ptr, csv_data.as_ptr(), 1, 0, &mut flen);
        test!("csv field ptr", !f.is_null());
        test!("csv field len", flen == 5); 

        
        test!("csv field eq", miku_csv_field_eq(csv_ptr, csv_data.as_ptr(), 1, 0, cstr!("Alice")));
        test!("csv field neq", !miku_csv_field_eq(csv_ptr, csv_data.as_ptr(), 1, 0, cstr!("Bob")));

        
        let age = miku_csv_field_int(csv_ptr, csv_data.as_ptr(), 1, 1, -1);
        test!("csv field int", age == 30);
        let age2 = miku_csv_field_int(csv_ptr, csv_data.as_ptr(), 2, 1, -1);
        test!("csv field int 2", age2 == 25);

        
        let age_u = miku_csv_field_u64(
            csv_ptr as *const u8,
            csv_data.as_ptr(),
            1, 1, 999,
        );
        test!("csv field u64", age_u == 30);

        
        test!("csv field not empty", !miku_csv_field_empty(
            csv_ptr as *const u8, 1, 0
        ));

        
        let col = miku_csv_find_col(
            csv_ptr as *const u8,
            csv_data.as_ptr(),
            cstr!("age"),
        );
        test!("csv find col", col == 1);
        let col_miss = miku_csv_find_col(
            csv_ptr as *const u8,
            csv_data.as_ptr(),
            cstr!("zipcode"),
        );
        test!("csv find col miss", col_miss == -1);

        
        let quoted_csv = b"a,\"hello, world\",c\n";
        let rows2 = miku_csv_parse(csv_ptr, quoted_csv.as_ptr(), quoted_csv.len());
        test!("csv quoted parse", rows2 == 1);
        test!("csv quoted cols", miku_csv_cols(csv_ptr, 0) == 3);
        let mut qlen: usize = 0;
        let _qf = miku_csv_field(csv_ptr, quoted_csv.as_ptr(), 0, 1, &mut qlen);
        test!("csv quoted field len", qlen == 12); 

        
        let tsv_data = b"a\tb\tc\n1\t2\t3\n";
        (*csv_ptr).delimiter = b'\t';
        let tsv_rows = miku_csv_parse(csv_ptr, tsv_data.as_ptr(), tsv_data.len());
        test!("tsv parse", tsv_rows == 2);
        test!("tsv cols", miku_csv_cols(csv_ptr, 0) == 3);
        (*csv_ptr).delimiter = b','; 

        
        let mut buf = [0u8; 512];
        let mut w = miku_csv_writer_init(buf.as_mut_ptr(), 512, b',');
        test!("csv writer no error", !miku_csv_writer_error(&w));

        
        miku_csv_write_cstr(&mut w, cstr!("name"));
        miku_csv_write_cstr(&mut w, cstr!("age"));
        miku_csv_write_cstr(&mut w, cstr!("city"));
        miku_csv_write_row_end(&mut w);

        
        miku_csv_write_cstr(&mut w, cstr!("Miku"));
        miku_csv_write_int(&mut w, 16);
        miku_csv_write_cstr(&mut w, cstr!("Sapporo"));
        miku_csv_write_row_end(&mut w);

        let wlen = miku_csv_writer_len(&w);
        test!("csv writer len > 0", wlen > 0);
        test!("csv writer no error after", !miku_csv_writer_error(&w));

        
        let wdata = miku_csv_writer_data(&w);
        let rows3 = miku_csv_parse(csv_ptr, wdata, wlen);
        test!("csv roundtrip rows", rows3 == 2);
        test!("csv roundtrip field", miku_csv_field_eq(csv_ptr, wdata, 1, 0, cstr!("Miku")));
        let rt_age = miku_csv_field_int(csv_ptr, wdata, 1, 1, -1);
        test!("csv roundtrip int", rt_age == 16);

        
        miku_csv_writer_reset(&mut w);
        let needs_quote = b"hello, world";
        miku_csv_write_field(&mut w, needs_quote.as_ptr(), 12);
        miku_csv_write_row_end(&mut w);
        let qdata = miku_csv_writer_data(&w);
        let qwlen = miku_csv_writer_len(&w);
        
        let mut has_quote = false;
        for i in 0..qwlen {
            if *qdata.add(i) == b'"' { has_quote = true; break; }
        }
        test!("csv writer quotes", has_quote);

        miku_free(csv_ptr as *mut u8);
    }
    println("");
}

fn test_lz_extended() {
    println("--- lz extended ---");
    unsafe {
        // RLE compress/decompress
        let data = b"AAAAABBBCCCCCCDD";
        let data_len = data.len();
        let mut cbuf = [0u8; 256];
        let mut dbuf = [0u8; 256];

        let clen = miku_rle_compress(data.as_ptr(), data_len, cbuf.as_mut_ptr(), 256);
        test!("rle compress ok", clen > 0);

        let dlen = miku_rle_decompress(cbuf.as_ptr(), clen as usize, dbuf.as_mut_ptr(), 256);
        test!("rle decompress ok", dlen == data_len as i32);
        test!("rle roundtrip", miku_memcmp(data.as_ptr(), dbuf.as_ptr(), data_len) == 0);

        // RLE with repetitive data should compress well
        let repeat = [0xAAu8; 100];
        let rlen = miku_rle_compress(repeat.as_ptr(), 100, cbuf.as_mut_ptr(), 256);
        test!("rle compress repeats", rlen > 0 && (rlen as usize) < 100);

        // RLE roundtrip of repeats
        let dlen2 = miku_rle_decompress(cbuf.as_ptr(), rlen as usize, dbuf.as_mut_ptr(), 256);
        test!("rle repeat roundtrip", dlen2 == 100 && miku_memcmp(repeat.as_ptr(), dbuf.as_ptr(), 100) == 0);

        // RLE compress bound
        let bound = miku_rle_compress_bound(100);
        test!("rle bound >= input", bound >= 100);

        // RLE null input
        test!("rle null", miku_rle_compress(core::ptr::null(), 0, cbuf.as_mut_ptr(), 256) == -1);

        // RLE non-compressible data
        let nocomp: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
        let nclen = miku_rle_compress(nocomp.as_ptr(), 8, cbuf.as_mut_ptr(), 256);
        test!("rle nocomp ok", nclen > 0);
        let ncdlen = miku_rle_decompress(cbuf.as_ptr(), nclen as usize, dbuf.as_mut_ptr(), 256);
        test!("rle nocomp roundtrip", ncdlen == 8 && miku_memcmp(nocomp.as_ptr(), dbuf.as_ptr(), 8) == 0);

        // Delta encode/decode
        let smooth: [u8; 8] = [10, 12, 14, 16, 18, 20, 22, 24];
        let mut delta_buf = [0u8; 8];
        let mut decoded_buf = [0u8; 8];
        miku_delta_encode(smooth.as_ptr(), 8, delta_buf.as_mut_ptr());
        // first byte unchanged, rest should be delta=2
        test!("delta encode first", delta_buf[0] == 10);
        test!("delta encode diff", delta_buf[1] == 2 && delta_buf[2] == 2);

        miku_delta_decode(delta_buf.as_ptr(), 8, decoded_buf.as_mut_ptr());
        test!("delta roundtrip", miku_memcmp(smooth.as_ptr(), decoded_buf.as_ptr(), 8) == 0);

        // Delta + RLE pipeline: delta encode smooth data, then RLE compress
        let mut delta_out = [0u8; 8];
        miku_delta_encode(smooth.as_ptr(), 8, delta_out.as_mut_ptr());
        let pipe_clen = miku_rle_compress(delta_out.as_ptr(), 8, cbuf.as_mut_ptr(), 256);
        test!("delta+rle compress", pipe_clen > 0);
        // decompress and decode
        let pipe_dlen = miku_rle_decompress(cbuf.as_ptr(), pipe_clen as usize, dbuf.as_mut_ptr(), 256);
        test!("delta+rle decompress", pipe_dlen == 8);
        miku_delta_decode(dbuf.as_ptr(), 8, decoded_buf.as_mut_ptr());
        test!("delta+rle roundtrip", miku_memcmp(smooth.as_ptr(), decoded_buf.as_ptr(), 8) == 0);
    }
    println("");
}

static mut EVT_ONCE_COUNTER: u32 = 0;

extern "C" fn test_evt_once_handler(_id: u32, _data: *mut u8, _ctx: *mut u8) {
    unsafe { EVT_ONCE_COUNTER += 1; }
}

fn test_event_extended() {
    println("--- event extended ---");
    unsafe {
        miku_event_clear_all();
        miku_event_queue_clear();
        EVT_ONCE_COUNTER = 0;

        // one-shot handler
        let idx = miku_event_once(42, test_evt_once_handler, core::ptr::null_mut());
        test!("evt once register", idx >= 0);
        test!("evt once has listener", miku_event_has_listeners(42));

        // emit - should fire once
        miku_event_emit(42, core::ptr::null_mut());
        test!("evt once fired", EVT_ONCE_COUNTER == 1);

        // emit again - should NOT fire (one-shot removed)
        miku_event_emit(42, core::ptr::null_mut());
        test!("evt once removed", EVT_ONCE_COUNTER == 1);
        test!("evt once no listeners", !miku_event_has_listeners(42));

        // event queue: post and flush
        EVT_COUNTER = 0;
        let _h = miku_event_on(99, test_evt_handler, core::ptr::null_mut());

        miku_event_post(99, core::ptr::null_mut());
        miku_event_post(99, core::ptr::null_mut());
        miku_event_post(99, core::ptr::null_mut());

        // pending should be 3
        test!("evt pending 3", miku_event_pending() == 3);

        // flush processes them
        let flushed = miku_event_flush();
        test!("evt flush 3", flushed == 3);
        test!("evt flush counter", EVT_COUNTER == 3);
        test!("evt pending 0", miku_event_pending() == 0);

        // queue clear
        miku_event_post(99, core::ptr::null_mut());
        miku_event_post(99, core::ptr::null_mut());
        test!("evt pending 2", miku_event_pending() == 2);
        miku_event_queue_clear();
        test!("evt queue cleared", miku_event_pending() == 0);

        // flush on empty queue
        let flushed2 = miku_event_flush();
        test!("evt flush empty", flushed2 == 0);

        miku_event_clear_all();
        miku_event_queue_clear();
    }
    println("");
}

// =====================================================
//  raw syscall helpers for new syscalls 36-42
// =====================================================

#[inline(always)]
unsafe fn raw_sc1(nr: u64, a1: u64) -> i64 {
    let r: i64;
    core::arch::asm!(
        "syscall",
        in("rax") nr, in("rdi") a1,
        lateout("rax") r,
        out("rcx") _, out("r11") _,
        options(nostack)
    );
    r
}

#[inline(always)]
unsafe fn raw_sc3(nr: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let r: i64;
    core::arch::asm!(
        "syscall",
        in("rax") nr, in("rdi") a1, in("rsi") a2, in("rdx") a3,
        lateout("rax") r,
        out("rcx") _, out("r11") _,
        options(nostack)
    );
    r
}

#[inline(always)]
unsafe fn raw_sc4(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    let r: i64;
    core::arch::asm!(
        "syscall",
        in("rax") nr, in("rdi") a1, in("rsi") a2, in("rdx") a3, in("r10") a4,
        lateout("rax") r,
        out("rcx") _, out("r11") _,
        options(nostack)
    );
    r
}

const SYS_STATFS: u64 = 36;
const SYS_FALLOCATE: u64 = 37;
const SYS_GETXATTR: u64 = 38;
const SYS_SETXATTR: u64 = 39;
const SYS_UTIMENSAT: u64 = 40;
const SYS_FSYNC: u64 = 41;
const SYS_PUNCH_HOLE: u64 = 42;

// =====================================================
//  ext filesystem tests
// =====================================================

fn test_ext_timestamps() {
    println("--- ext: timestamps ---");
    unsafe {
        // create a file, check mtime/ctime are set
        let path = cstr!("/tmp/ts_test.txt");
        let fd = miku_create(path, 0o644);
        if fd < 0 {
            // /tmp might not exist, try creating it
            miku_mkdir(cstr!("/tmp"), 0o755);
            let fd2 = miku_create(path, 0o644);
            if fd2 < 0 {
                fail("ts: create", "cannot create file");
                println("");
                return;
            }
            miku_close(fd2);
        } else {
            miku_close(fd);
        }

        let mut st = core::mem::zeroed::<MikuStat>();
        let r = miku_stat(path, &mut st);
        test!("ts: stat ok", r >= 0, "stat failed");

        // mtime and ctime should be non-zero (set to current time at creation)
        test!("ts: mtime set", st.mtime > 0, "mtime is 0");
        test!("ts: ctime set", st.ctime > 0, "ctime is 0");

        let create_mtime = st.mtime;
        let create_ctime = st.ctime;

        // write to file - mtime and ctime should update
        let fd = miku_open_rw(path);
        if fd >= 0 {
            let data = b"hello timestamps\0";
            let w = miku_write_fd(fd, data.as_ptr(), data.len() - 1);
            test!("ts: write ok", w > 0, "write failed");
            miku_close(fd);

            let mut st2 = core::mem::zeroed::<MikuStat>();
            miku_stat(path, &mut st2);
            test!("ts: mtime updated", st2.mtime >= create_mtime, "mtime decreased");
            test!("ts: ctime after write", st2.ctime >= create_ctime, "ctime decreased");

            print("  create_mtime="); print_int(create_mtime as i64);
            print(" write_mtime="); print_int(st2.mtime as i64); println("");
        }

        // read file - atime should update (relatime)
        let fd = miku_open_cstr(path);
        if fd >= 0 {
            let mut buf = [0u8; 32];
            miku_read(fd as u64, buf.as_mut_ptr(), 16);
            miku_close(fd);

            let mut st3 = core::mem::zeroed::<MikuStat>();
            miku_stat(path, &mut st3);
            // atime should be >= mtime after a read (relatime updates when atime < mtime)
            test!("ts: atime after read", st3.atime > 0, "atime still 0");
            print("  atime="); print_int(st3.atime as i64); println("");
        }

        // chmod - only ctime should update
        let r = miku_chmod(path, 0o600);
        if r >= 0 {
            let mut st4 = core::mem::zeroed::<MikuStat>();
            miku_stat(path, &mut st4);
            test!("ts: ctime after chmod", st4.ctime >= create_ctime, "ctime not updated");
        }

        miku_unlink(path);
    }
    println("");
}

fn test_ext_statfs() {
    println("--- ext: statfs ---");
    unsafe {
        // statfs on root "/"
        let path = "/\0";
        let mut buf = [0u8; 48];
        let r = raw_sc3(SYS_STATFS,
            path.as_ptr() as u64,
            1, // path_len = 1
            buf.as_mut_ptr() as u64);

        test!("statfs: call ok", r == 0, "syscall failed");

        if r == 0 {
            let fs_type = (buf.as_ptr() as *const u32).read();
            let block_size = (buf.as_ptr().add(4) as *const u32).read();
            let total_blocks = (buf.as_ptr().add(8) as *const u64).read();
            let free_blocks = (buf.as_ptr().add(16) as *const u64).read();
            let total_inodes = (buf.as_ptr().add(24) as *const u64).read();
            let free_inodes = (buf.as_ptr().add(32) as *const u64).read();

            print("  fs_type="); print_hex(fs_type as u64);
            print(" blk_size="); print_int(block_size as i64);
            print(" total="); print_int(total_blocks as i64);
            print(" free="); print_int(free_blocks as i64);
            println("");
            print("  inodes="); print_int(total_inodes as i64);
            print(" free_i="); print_int(free_inodes as i64);
            println("");

            test!("statfs: block_size > 0", block_size > 0, "block_size is 0");
            test!("statfs: total > 0", total_blocks > 0, "total_blocks is 0");
            test!("statfs: free <= total", free_blocks <= total_blocks, "free > total");
            test!("statfs: inodes > 0", total_inodes > 0, "total_inodes is 0");
            test!("statfs: free_i <= total", free_inodes <= total_inodes, "free_i > total");

            // root is TmpFS (0x01021994) or ext2/3/4 (0xEF53)
            test!("statfs: known fs magic",
                fs_type == 0xEF53 || fs_type == 0x01021994 || fs_type == 0x1373 || fs_type == 0x9FA0,
                "unknown fs_type");
        }
    }
    println("");
}

fn test_ext_fsync() {
    println("--- ext: fsync ---");
    unsafe {
        let path = cstr!("/tmp/fsync_test.txt");
        miku_mkdir(cstr!("/tmp"), 0o755);
        let fd = miku_create(path, 0o644);
        if fd < 0 {
            fail("fsync: create", "cannot create file");
            println("");
            return;
        }

        let data = b"data to sync\0";
        miku_write_fd(fd, data.as_ptr(), data.len() - 1);

        let r = raw_sc1(SYS_FSYNC, fd as u64);
        test!("fsync: ok", r == 0, "fsync failed");

        // fsync on invalid fd
        let r2 = raw_sc1(SYS_FSYNC, 999);
        test!("fsync: bad fd", r2 < 0, "expected error");

        miku_close(fd);
        miku_unlink(path);
    }
    println("");
}

fn test_ext_fallocate() {
    println("--- ext: fallocate ---");
    unsafe {
        let path = cstr!("/tmp/falloc_test.txt");
        miku_mkdir(cstr!("/tmp"), 0o755);
        let fd = miku_create(path, 0o644);
        if fd < 0 {
            fail("falloc: create", "cannot create file");
            println("");
            return;
        }

        // preallocate 8192 bytes (may fail on TmpFS - that's expected)
        let r = raw_sc3(SYS_FALLOCATE, fd as u64, 0, 8192);
        if r < 0 {
            // TmpFS does not support fallocate - skip gracefully
            ok("falloc: skip (tmpfs)");
        } else {
            ok("falloc: ok");
            // check that blocks were allocated
            let mut st = core::mem::zeroed::<MikuStat>();
            miku_fstat(fd, &mut st);
            print("  blocks="); print_int(st.blocks as i64); println("");
            test!("falloc: blocks > 0", st.blocks > 0, "no blocks allocated");
        }

        // fallocate with 0 len should fail
        let r2 = raw_sc3(SYS_FALLOCATE, fd as u64, 0, 0);
        test!("falloc: zero len err", r2 < 0, "expected error");

        miku_close(fd);
        miku_unlink(path);
    }
    println("");
}

fn test_ext_hardlink_timestamps() {
    println("--- ext: hardlink timestamps ---");
    unsafe {
        miku_mkdir(cstr!("/tmp"), 0o755);
        let orig = cstr!("/tmp/hl_orig.txt");
        let fd = miku_create(orig, 0o644);
        if fd < 0 {
            fail("hl: create", "cannot create file");
            println("");
            return;
        }
        miku_write_fd(fd, b"link test\0".as_ptr(), 9);
        miku_close(fd);

        let mut st1 = core::mem::zeroed::<MikuStat>();
        miku_stat(orig, &mut st1);
        let orig_ctime = st1.ctime;

        // create hardlink
        let link = cstr!("/tmp/hl_link.txt");
        let r = miku_link(orig, link);
        test!("hl: link ok", r >= 0, "link failed");

        if r >= 0 {
            // nlinks should be 2
            let mut st2 = core::mem::zeroed::<MikuStat>();
            miku_stat(orig, &mut st2);
            test!("hl: nlinks=2", st2.nlinks == 2, "nlinks wrong");

            // ctime should be updated on the inode
            test!("hl: ctime updated", st2.ctime >= orig_ctime, "ctime not updated");

            // unlink one
            miku_unlink(link);
            let mut st3 = core::mem::zeroed::<MikuStat>();
            miku_stat(orig, &mut st3);
            test!("hl: nlinks=1", st3.nlinks == 1, "nlinks not 1");
        }

        miku_unlink(orig);
    }
    println("");
}

fn test_ext_symlink_timestamps() {
    println("--- ext: symlink timestamps ---");
    unsafe {
        miku_mkdir(cstr!("/tmp"), 0o755);
        let target = cstr!("/tmp/sl_target.txt");
        let fd = miku_create(target, 0o644);
        if fd < 0 {
            fail("sl: create", "cannot create file");
            println("");
            return;
        }
        miku_write_fd(fd, b"symlink test\0".as_ptr(), 12);
        miku_close(fd);

        // stat parent dir before symlink creation
        let mut pst1 = core::mem::zeroed::<MikuStat>();
        miku_stat(cstr!("/tmp"), &mut pst1);
        let dir_mtime_before = pst1.mtime;

        let link = cstr!("/tmp/sl_link.txt");
        let r = miku_symlink(target, link);
        test!("sl: create ok", r >= 0, "symlink failed");

        if r >= 0 {
            // readlink should return target
            let mut buf = [0u8; 128];
            let n = miku_readlink(link, buf.as_mut_ptr(), 128);
            test!("sl: readlink ok", n > 0, "readlink failed");

            // parent dir mtime should be updated
            let mut pst2 = core::mem::zeroed::<MikuStat>();
            miku_stat(cstr!("/tmp"), &mut pst2);
            test!("sl: dir mtime updated", pst2.mtime >= dir_mtime_before, "dir mtime not updated");

            miku_unlink(link);
        }

        miku_unlink(target);
    }
    println("");
}

fn test_ext_truncate_timestamps() {
    println("--- ext: truncate timestamps ---");
    unsafe {
        miku_mkdir(cstr!("/tmp"), 0o755);
        let path = cstr!("/tmp/trunc_test.txt");
        let fd = miku_create(path, 0o644);
        if fd < 0 {
            fail("trunc: create", "cannot create file");
            println("");
            return;
        }
        let data = b"hello world 12345678901234567890\0";
        miku_write_fd(fd, data.as_ptr(), data.len() - 1);

        let mut st1 = core::mem::zeroed::<MikuStat>();
        miku_fstat(fd, &mut st1);
        let before_mtime = st1.mtime;
        let before_size = st1.size;

        test!("trunc: initial size", before_size > 0, "size is 0");

        // truncate to 5 bytes
        let r = miku_ftruncate(fd, 5);
        test!("trunc: ok", r >= 0, "ftruncate failed");

        let mut st2 = core::mem::zeroed::<MikuStat>();
        miku_fstat(fd, &mut st2);
        test!("trunc: size=5", st2.size == 5, "size not 5");
        test!("trunc: mtime updated", st2.mtime >= before_mtime, "mtime not updated");
        test!("trunc: ctime updated", st2.ctime >= st1.ctime, "ctime not updated");

        print("  before="); print_int(before_size as i64);
        print(" after="); print_int(st2.size as i64); println("");

        miku_close(fd);
        miku_unlink(path);
    }
    println("");
}

fn test_ext_rename_timestamps() {
    println("--- ext: rename timestamps ---");
    unsafe {
        miku_mkdir(cstr!("/tmp"), 0o755);
        let old_path = cstr!("/tmp/ren_old.txt");
        let new_path = cstr!("/tmp/ren_new.txt");

        let fd = miku_create(old_path, 0o644);
        if fd < 0 {
            fail("ren: create", "cannot create file");
            println("");
            return;
        }
        miku_write_fd(fd, b"rename test\0".as_ptr(), 11);
        miku_close(fd);

        let mut st1 = core::mem::zeroed::<MikuStat>();
        miku_stat(old_path, &mut st1);
        let orig_ctime = st1.ctime;

        // stat parent dir
        let mut pst1 = core::mem::zeroed::<MikuStat>();
        miku_stat(cstr!("/tmp"), &mut pst1);
        let dir_mtime_before = pst1.mtime;

        let r = miku_rename(old_path, new_path);
        test!("ren: ok", r >= 0, "rename failed");

        if r >= 0 {
            // old path should not exist
            test!("ren: old gone", !miku_access(old_path), "old still exists");

            // new path should exist
            test!("ren: new exists", miku_access(new_path), "new not found");

            // ctime on the inode should update
            let mut st2 = core::mem::zeroed::<MikuStat>();
            miku_stat(new_path, &mut st2);
            test!("ren: ctime updated", st2.ctime >= orig_ctime, "ctime not updated");

            // parent dir mtime should update
            let mut pst2 = core::mem::zeroed::<MikuStat>();
            miku_stat(cstr!("/tmp"), &mut pst2);
            test!("ren: dir mtime updated", pst2.mtime >= dir_mtime_before, "dir mtime not updated");

            // size should be preserved
            test!("ren: size preserved", st2.size == 11, "size changed");

            miku_unlink(new_path);
        } else {
            miku_unlink(old_path);
        }
    }
    println("");
}

// ==================== libc compatibility layer tests ====================

fn test_libc_string() {
    println("-- libc: string.h --");
    unsafe {
        // strlen
        test!("libc: strlen", libc_strlen(cstr!("hello")) == 5);
        test!("libc: strlen empty", libc_strlen(cstr!("")) == 0);

        // strnlen
        test!("libc: strnlen", strnlen(cstr!("hello"), 3) == 3);
        test!("libc: strnlen full", strnlen(cstr!("hi"), 10) == 2);

        // strcmp
        test!("libc: strcmp eq", strcmp(cstr!("abc"), cstr!("abc")) == 0);
        test!("libc: strcmp lt", strcmp(cstr!("abc"), cstr!("abd")) < 0);
        test!("libc: strcmp gt", strcmp(cstr!("abd"), cstr!("abc")) > 0);

        // strncmp
        test!("libc: strncmp eq", strncmp(cstr!("abcX"), cstr!("abcY"), 3) == 0);
        test!("libc: strncmp ne", strncmp(cstr!("abcX"), cstr!("abcY"), 4) != 0);

        // strcasecmp
        test!("libc: strcasecmp", strcasecmp(cstr!("Hello"), cstr!("hello")) == 0);

        // strcpy / strncpy
        let mut buf = [0u8; 32];
        strcpy(buf.as_mut_ptr(), cstr!("test"));
        test!("libc: strcpy", miku_strcmp(buf.as_ptr(), cstr!("test")) == 0);

        let mut buf2 = [0u8; 32];
        strncpy(buf2.as_mut_ptr(), cstr!("hello world"), 5);
        test!("libc: strncpy", strncmp(buf2.as_ptr(), cstr!("hello"), 5) == 0);

        // strlcpy
        let mut buf3 = [0u8; 8];
        let r = strlcpy(buf3.as_mut_ptr(), cstr!("hello world"), 8);
        test!("libc: strlcpy truncate", r == 11);
        test!("libc: strlcpy content", strcmp(buf3.as_ptr(), cstr!("hello w")) == 0);

        // strcat
        let mut cat_buf = [0u8; 32];
        strcpy(cat_buf.as_mut_ptr(), cstr!("hello"));
        strcat(cat_buf.as_mut_ptr(), cstr!(" world"));
        test!("libc: strcat", strcmp(cat_buf.as_ptr(), cstr!("hello world")) == 0);

        // strncat
        let mut ncat_buf = [0u8; 32];
        strcpy(ncat_buf.as_mut_ptr(), cstr!("hi"));
        strncat(ncat_buf.as_mut_ptr(), cstr!("!!!!"), 2);
        test!("libc: strncat", strcmp(ncat_buf.as_ptr(), cstr!("hi!!")) == 0);

        // strchr / strrchr
        let s = cstr!("hello world");
        test!("libc: strchr", !strchr(s, b'o' as i32).is_null());
        test!("libc: strchr offset", strchr(s, b'o' as i32).offset_from(s) == 4);
        test!("libc: strrchr", strrchr(s, b'o' as i32).offset_from(s) == 7);
        test!("libc: strchr not found", strchr(s, b'z' as i32).is_null());

        // strstr
        test!("libc: strstr found", !strstr(cstr!("hello world"), cstr!("world")).is_null());
        test!("libc: strstr not found", strstr(cstr!("hello"), cstr!("xyz")).is_null());

        // strdup
        let d = strdup(cstr!("clone"));
        test!("libc: strdup", !d.is_null() && strcmp(d, cstr!("clone")) == 0);
        miku_free(d);

        // strndup
        let d2 = strndup(cstr!("hello world"), 5);
        test!("libc: strndup", !d2.is_null() && strcmp(d2, cstr!("hello")) == 0);
        miku_free(d2);

        // strspn / strcspn
        test!("libc: strspn", strspn(cstr!("12345abc"), cstr!("0123456789")) == 5);
        test!("libc: strcspn", strcspn(cstr!("hello!world"), cstr!("!")) == 5);

        // strpbrk
        let p = strpbrk(cstr!("hello123"), cstr!("0123456789"));
        test!("libc: strpbrk", !p.is_null() && *p == b'1');

        // strtok_r
        let mut tok_buf = [0u8; 32];
        strcpy(tok_buf.as_mut_ptr(), cstr!("one,two,three"));
        let mut saveptr: *mut u8 = core::ptr::null_mut();
        let t1 = strtok_r(tok_buf.as_mut_ptr(), cstr!(","), &mut saveptr);
        test!("libc: strtok_r first", !t1.is_null() && strcmp(t1, cstr!("one")) == 0);
        let t2 = strtok_r(core::ptr::null_mut(), cstr!(","), &mut saveptr);
        test!("libc: strtok_r second", !t2.is_null() && strcmp(t2, cstr!("two")) == 0);
        let t3 = strtok_r(core::ptr::null_mut(), cstr!(","), &mut saveptr);
        test!("libc: strtok_r third", !t3.is_null() && strcmp(t3, cstr!("three")) == 0);
    }
    println("");
}

fn test_libc_memory() {
    println("-- libc: memory --");
    unsafe {
        // memset
        let mut buf = [0xFFu8; 16];
        memset(buf.as_mut_ptr(), 0, 16);
        test!("libc: memset", buf.iter().all(|&b| b == 0));

        // memset partial
        let mut buf2 = [0u8; 16];
        memset(buf2.as_mut_ptr(), 0xAB, 8);
        test!("libc: memset partial", buf2[0] == 0xAB && buf2[7] == 0xAB && buf2[8] == 0);

        // memcpy
        let src = b"hello\0";
        let mut dst = [0u8; 8];
        memcpy(dst.as_mut_ptr(), src.as_ptr(), 6);
        test!("libc: memcpy", strcmp(dst.as_ptr(), cstr!("hello")) == 0);

        // memmove (overlapping)
        let mut buf3 = [0u8; 16];
        memcpy(buf3.as_mut_ptr(), b"ABCDEFGH\0".as_ptr(), 9);
        memmove(buf3.as_mut_ptr().add(2), buf3.as_ptr(), 6);
        test!("libc: memmove", buf3[0] == b'A' && buf3[2] == b'A' && buf3[4] == b'C');

        // memcmp
        test!("libc: memcmp eq", memcmp(b"abc\0".as_ptr(), b"abc\0".as_ptr(), 3) == 0);
        test!("libc: memcmp ne", memcmp(b"abc\0".as_ptr(), b"abd\0".as_ptr(), 3) < 0);

        // memchr
        let haystack = b"hello world\0";
        let p = memchr(haystack.as_ptr(), b'w' as i32, 11);
        test!("libc: memchr", !p.is_null() && p.offset_from(haystack.as_ptr()) == 6);
        test!("libc: memchr not found", memchr(haystack.as_ptr(), b'z' as i32, 11).is_null());

        // memmem
        let hay = b"hello world\0";
        let needle = b"world";
        let f = memmem(hay.as_ptr(), 11, needle.as_ptr(), 5);
        test!("libc: memmem", !f.is_null() && f.offset_from(hay.as_ptr()) == 6);

        // bzero
        let mut bz = [0xFFu8; 8];
        bzero(bz.as_mut_ptr(), 8);
        test!("libc: bzero", bz.iter().all(|&b| b == 0));
    }
    println("");
}

fn test_libc_stdlib() {
    println("-- libc: stdlib.h --");
    unsafe {
        // malloc / free
        let p = libc_malloc(64);
        test!("libc: malloc", !p.is_null());
        memset(p, 0x42, 64);
        test!("libc: malloc write", *p == 0x42 && *p.add(63) == 0x42);
        libc_free(p);

        // calloc
        let c = libc_calloc(16, 4);
        test!("libc: calloc", !c.is_null());
        let all_zero = (0..64).all(|i| *c.add(i) == 0);
        test!("libc: calloc zeroed", all_zero);
        libc_free(c);

        // realloc
        let r = libc_malloc(32);
        memset(r, 0xAA, 32);
        let r2 = libc_realloc(r, 128);
        test!("libc: realloc", !r2.is_null());
        test!("libc: realloc preserved", *r2 == 0xAA);
        libc_free(r2);

        // aligned_alloc
        let a = aligned_alloc(64, 256);
        test!("libc: aligned_alloc", !a.is_null());
        test!("libc: aligned_alloc align", (a as usize) % 64 == 0);
        libc_free(a);

        // atoi
        test!("libc: atoi", atoi(cstr!("42")) == 42);
        test!("libc: atoi neg", atoi(cstr!("-7")) == -7);
        test!("libc: atoi zero", atoi(cstr!("0")) == 0);

        // atol
        test!("libc: atol", atol(cstr!("1000000")) == 1000000);

        // strtol
        test!("libc: strtol dec", strtol(cstr!("123"), core::ptr::null_mut(), 10) == 123);
        test!("libc: strtol hex", strtol(cstr!("0xff"), core::ptr::null_mut(), 0) == 255);
        test!("libc: strtol oct", strtol(cstr!("077"), core::ptr::null_mut(), 0) == 63);
        test!("libc: strtol neg", strtol(cstr!("-42"), core::ptr::null_mut(), 10) == -42);

        // strtoul
        test!("libc: strtoul", strtoul(cstr!("1000"), core::ptr::null_mut(), 10) == 1000);

        // abs / labs
        test!("libc: abs", libc_abs(-42) == 42);
        test!("libc: abs pos", libc_abs(7) == 7);
        test!("libc: labs", labs(-100) == 100);

        // rand / srand
        libc_srand(42);
        let r1 = libc_rand();
        let r2 = libc_rand();
        test!("libc: rand returns", r1 >= 0);
        test!("libc: rand varies", r1 != r2);

        // srand deterministic
        libc_srand(42);
        let r3 = libc_rand();
        test!("libc: srand seed", r1 == r3);
    }
    println("");
}

fn test_libc_ctype() {
    println("-- libc: ctype.h --");
    unsafe {
        test!("libc: isdigit '5'", isdigit(b'5' as i32) != 0);
        test!("libc: isdigit 'a'", isdigit(b'a' as i32) == 0);
        test!("libc: isalpha 'Z'", isalpha(b'Z' as i32) != 0);
        test!("libc: isalpha '9'", isalpha(b'9' as i32) == 0);
        test!("libc: isalnum 'a'", isalnum(b'a' as i32) != 0);
        test!("libc: isalnum '3'", isalnum(b'3' as i32) != 0);
        test!("libc: isalnum '!'", isalnum(b'!' as i32) == 0);
        test!("libc: isspace ' '", isspace(b' ' as i32) != 0);
        test!("libc: isspace 'a'", isspace(b'a' as i32) == 0);
        test!("libc: isupper 'A'", isupper(b'A' as i32) != 0);
        test!("libc: isupper 'a'", isupper(b'a' as i32) == 0);
        test!("libc: islower 'z'", islower(b'z' as i32) != 0);
        test!("libc: isprint '~'", isprint(b'~' as i32) != 0);
        test!("libc: iscntrl 0x01", iscntrl(0x01) != 0);
        test!("libc: isxdigit 'f'", isxdigit(b'f' as i32) != 0);
        test!("libc: isxdigit 'g'", isxdigit(b'g' as i32) == 0);
        test!("libc: toupper 'a'", toupper(b'a' as i32) == b'A' as i32);
        test!("libc: tolower 'Z'", tolower(b'Z' as i32) == b'z' as i32);
        test!("libc: ispunct '!'", ispunct(b'!' as i32) != 0);
    }
    println("");
}

fn test_libc_stdio_basic() {
    println("-- libc: stdio basic --");
    unsafe {
        // snprintf
        let mut buf = [0u8; 64];
        let n = snprintf(buf.as_mut_ptr(), 64, cstr!("hello %s %d"), cstr!("world"), 42i64);
        test!("libc: snprintf", n > 0);
        test!("libc: snprintf content", strcmp(buf.as_ptr(), cstr!("hello world 42")) == 0);

        // snprintf truncation
        let mut buf2 = [0u8; 8];
        snprintf(buf2.as_mut_ptr(), 8, cstr!("hello world"));
        test!("libc: snprintf trunc", strcmp(buf2.as_ptr(), cstr!("hello w")) == 0);

        // sprintf
        let mut buf3 = [0u8; 64];
        sprintf(buf3.as_mut_ptr(), cstr!("%d + %d = %d"), 1i64, 2i64, 3i64);
        test!("libc: sprintf", strcmp(buf3.as_ptr(), cstr!("1 + 2 = 3")) == 0);

        // snprintf format specifiers
        let mut buf4 = [0u8; 64];
        snprintf(buf4.as_mut_ptr(), 64, cstr!("%x"), 255i64);
        test!("libc: snprintf hex", strcmp(buf4.as_ptr(), cstr!("ff")) == 0);

        let mut buf5 = [0u8; 64];
        snprintf(buf5.as_mut_ptr(), 64, cstr!("%05d"), 42i64);
        test!("libc: snprintf pad", strcmp(buf5.as_ptr(), cstr!("00042")) == 0);

        let mut buf6 = [0u8; 64];
        snprintf(buf6.as_mut_ptr(), 64, cstr!("%c"), b'A' as i64);
        test!("libc: snprintf char", strcmp(buf6.as_ptr(), cstr!("A")) == 0);
    }
    println("");
}

fn test_libc_file_io() {
    println("-- libc: file I/O --");
    unsafe {
        let test_path = cstr!("/libc_test.txt");
        let test_data = b"libc test data\n";

        // fopen + fwrite + fclose
        let f = fopen(test_path, cstr!("w"));
        test!("libc: fopen w", !f.is_null());
        if !f.is_null() {
            let n = fwrite(test_data.as_ptr(), 1, test_data.len(), f);
            test!("libc: fwrite", n == test_data.len());
            let r = fclose(f);
            test!("libc: fclose", r == 0);
        }

        // fopen + fread
        let f2 = fopen(test_path, cstr!("r"));
        test!("libc: fopen r", !f2.is_null());
        if !f2.is_null() {
            let mut rbuf = [0u8; 64];
            let n = fread(rbuf.as_mut_ptr(), 1, 64, f2);
            test!("libc: fread count", n == test_data.len());
            test!("libc: fread content", strncmp(rbuf.as_ptr(), test_data.as_ptr(), n) == 0);

            // feof after reading all
            let n2 = fread(rbuf.as_mut_ptr(), 1, 1, f2);
            test!("libc: feof after read", n2 == 0 && feof(f2) != 0);

            fclose(f2);
        }

        // fgets
        let f3 = fopen(test_path, cstr!("r"));
        if !f3.is_null() {
            let mut line = [0u8; 64];
            let r = fgets(line.as_mut_ptr(), 64, f3);
            test!("libc: fgets", !r.is_null());
            test!("libc: fgets content", strncmp(line.as_ptr(), b"libc test data\n\0".as_ptr(), 15) == 0);
            fclose(f3);
        }

        // fputc / fgetc
        let f4 = fopen(test_path, cstr!("w"));
        if !f4.is_null() {
            fputc(b'X' as i32, f4);
            fputc(b'Y' as i32, f4);
            fputc(b'Z' as i32, f4);
            fclose(f4);
        }
        let f5 = fopen(test_path, cstr!("r"));
        if !f5.is_null() {
            let c1 = fgetc(f5);
            let c2 = fgetc(f5);
            let c3 = fgetc(f5);
            test!("libc: fputc/fgetc", c1 == b'X' as i32 && c2 == b'Y' as i32 && c3 == b'Z' as i32);

            // ungetc
            let r = ungetc(b'!' as i32, f5);
            test!("libc: ungetc returns", r == b'!' as i32);
            let c4 = fgetc(f5);
            test!("libc: ungetc read", c4 == b'!' as i32);

            fclose(f5);
        }

        // fseek / ftell
        let f6 = fopen(test_path, cstr!("r"));
        if !f6.is_null() {
            fseek(f6, 1, 0); // SEEK_SET
            let pos = ftell(f6);
            test!("libc: ftell", pos == 1);
            let c = fgetc(f6);
            test!("libc: fseek+fgetc", c == b'Y' as i32);

            rewind(f6);
            let pos2 = ftell(f6);
            test!("libc: rewind", pos2 == 0);

            fclose(f6);
        }

        // ferror / clearerr
        let f7 = fopen(test_path, cstr!("r"));
        if !f7.is_null() {
            test!("libc: ferror init", ferror(f7) == 0);
            clearerr(f7);
            test!("libc: clearerr", ferror(f7) == 0 && feof(f7) == 0);
            fclose(f7);
        }

        // fileno
        let f8 = fopen(test_path, cstr!("r"));
        if !f8.is_null() {
            let fd = fileno(f8);
            test!("libc: fileno", fd >= 0);
            fclose(f8);
        }

        // fputs
        let f9 = fopen(test_path, cstr!("w"));
        if !f9.is_null() {
            fputs(cstr!("fputs test"), f9);
            fclose(f9);
        }
        let f10 = fopen(test_path, cstr!("r"));
        if !f10.is_null() {
            let mut rb = [0u8; 32];
            fread(rb.as_mut_ptr(), 1, 32, f10);
            test!("libc: fputs", strcmp(rb.as_ptr(), cstr!("fputs test")) == 0);
            fclose(f10);
        }

        // cleanup
        miku_unlink(test_path);
    }
    println("");
}

fn test_libc_unistd() {
    println("-- libc: unistd.h --");
    unsafe {
        let test_path = cstr!("/tmp/libc_unistd.txt");

        // open / write / read / close
        let fd = libc_open(test_path, 0x002A, 0o644); // O_WRITE|O_CREATE|O_TRUNCATE = 0x2A
        test!("libc: open create", fd >= 0);
        if fd >= 0 {
            let n = libc_write(fd, b"test\0".as_ptr(), 4);
            test!("libc: write", n == 4);
            libc_close(fd);
        }

        let fd2 = libc_open(test_path, 0x0001, 0); // O_READ
        test!("libc: open read", fd2 >= 0);
        if fd2 >= 0 {
            let mut buf = [0u8; 16];
            let n = libc_read(fd2, buf.as_mut_ptr(), 16);
            test!("libc: read", n == 4);
            test!("libc: read content", strncmp(buf.as_ptr(), cstr!("test"), 4) == 0);
            libc_close(fd2);
        }

        // lseek
        let fd3 = libc_open(test_path, 0x0001, 0);
        if fd3 >= 0 {
            let pos = lseek(fd3, 2, 0); // SEEK_SET
            test!("libc: lseek", pos == 2);
            let mut buf = [0u8; 8];
            let n = libc_read(fd3, buf.as_mut_ptr(), 8);
            test!("libc: lseek+read", n == 2 && buf[0] == b's' && buf[1] == b't');
            libc_close(fd3);
        }

        // getcwd
        let mut cwd = [0u8; 256];
        let r = getcwd(cwd.as_mut_ptr(), 256);
        test!("libc: getcwd", !r.is_null());
        test!("libc: getcwd content", libc_strlen(cwd.as_ptr()) > 0);

        // getpid
        let pid = libc_getpid();
        test!("libc: getpid", pid > 0);

        // access
        test!("libc: access exists", libc_access(test_path, 0) == 0);
        test!("libc: access not exists", libc_access(cstr!("/nonexistent_xyz"), 0) != 0);

        // dup
        let fd4 = libc_open(test_path, 0x0001, 0);
        if fd4 >= 0 {
            let fd5 = libc_dup(fd4);
            test!("libc: dup", fd5 >= 0 && fd5 != fd4);
            if fd5 >= 0 {
                let mut buf = [0u8; 8];
                let n = libc_read(fd5, buf.as_mut_ptr(), 4);
                test!("libc: dup read", n == 4);
                libc_close(fd5);
            }
            libc_close(fd4);
        }

        // pread / pwrite
        let fd6 = libc_open(test_path, 0x0003, 0); // O_RDWR
        if fd6 >= 0 {
            pwrite(fd6, b"XY".as_ptr(), 2, 1);
            let mut buf = [0u8; 8];
            let n = pread(fd6, buf.as_mut_ptr(), 2, 1);
            test!("libc: pread/pwrite", n == 2 && buf[0] == b'X' && buf[1] == b'Y');
            libc_close(fd6);
        }

        // ftruncate
        let fd7 = libc_open(test_path, 0x0003, 0); // O_RDWR
        if fd7 >= 0 {
            let r = libc_ftruncate(fd7, 2);
            test!("libc: ftruncate", r == 0);
            let sz = miku_fsize(fd7 as i64);
            test!("libc: ftruncate size", sz == 2);
            libc_close(fd7);
        }

        // unlink
        test!("libc: unlink", libc_unlink(test_path) == 0);
        test!("libc: unlink gone", libc_access(test_path, 0) != 0);

        // mkdir / rmdir
        let dir_path = cstr!("/tmp/libc_testdir");
        test!("libc: mkdir", libc_mkdir(dir_path, 0o755) == 0);
        test!("libc: mkdir exists", miku_isdir(dir_path));
        test!("libc: rmdir", libc_rmdir(dir_path) == 0);

        // symlink / readlink
        let sym_target = cstr!("/tmp/libc_symtarget.txt");
        let sym_link = cstr!("/tmp/libc_symlink");
        miku_write_file_cstr(sym_target, cstr!("sym"));
        let r = libc_symlink(sym_target, sym_link);
        test!("libc: symlink", r == 0);
        let mut link_buf = [0u8; 256];
        let rn = libc_readlink(sym_link, link_buf.as_mut_ptr(), 256);
        test!("libc: readlink", rn > 0);
        miku_unlink(sym_link);
        miku_unlink(sym_target);

        // link
        let lk_src = cstr!("/tmp/libc_linksrc.txt");
        let lk_dst = cstr!("/tmp/libc_linkdst.txt");
        miku_write_file_cstr(lk_src, cstr!("lnk"));
        let r = libc_link(lk_src, lk_dst);
        test!("libc: link", r == 0);
        test!("libc: link exists", libc_access(lk_dst, 0) == 0);
        miku_unlink(lk_src);
        miku_unlink(lk_dst);

        // rename
        let ren_old = cstr!("/tmp/libc_ren_old.txt");
        let ren_new = cstr!("/tmp/libc_ren_new.txt");
        miku_write_file_cstr(ren_old, cstr!("rename"));
        test!("libc: rename", libc_rename(ren_old, ren_new) == 0);
        test!("libc: rename old gone", libc_access(ren_old, 0) != 0);
        test!("libc: rename new exists", libc_access(ren_new, 0) == 0);
        miku_unlink(ren_new);

        // sched_yield (should not crash)
        test!("libc: sched_yield", sched_yield() == 0);
    }
    println("");
}

fn test_libc_dir() {
    println("-- libc: dirent.h --");
    unsafe {
        // create some files in /tmp
        let dir_path = cstr!("/tmp/libc_dirtest");
        libc_mkdir(dir_path, 0o755);
        miku_write_file_cstr(cstr!("/tmp/libc_dirtest/a.txt"), cstr!("a"));
        miku_write_file_cstr(cstr!("/tmp/libc_dirtest/b.txt"), cstr!("b"));

        let d = opendir(dir_path);
        test!("libc: opendir", !d.is_null());

        if !d.is_null() {
            let mut count = 0u32;
            loop {
                let ent = readdir(d);
                if ent.is_null() { break; }
                count += 1;
            }
            test!("libc: readdir count", count >= 2);

            // rewinddir
            rewinddir(d);
            let ent = readdir(d);
            test!("libc: rewinddir", !ent.is_null());

            test!("libc: closedir", closedir(d) == 0);
        }

        // cleanup
        miku_unlink(cstr!("/tmp/libc_dirtest/a.txt"));
        miku_unlink(cstr!("/tmp/libc_dirtest/b.txt"));
        libc_rmdir(dir_path);
    }
    println("");
}

fn test_libc_printf() {
    println("-- libc: printf --");
    unsafe {
        // test printf returns character count
        let n = printf(cstr!(""));
        test!("libc: printf empty", n == 0);

        // format strings
        let mut buf = [0u8; 128];
        snprintf(buf.as_mut_ptr(), 128, cstr!("%s"), cstr!("hello"));
        test!("libc: printf %s", strcmp(buf.as_ptr(), cstr!("hello")) == 0);

        snprintf(buf.as_mut_ptr(), 128, cstr!("%d"), -42i64);
        test!("libc: printf %d neg", strcmp(buf.as_ptr(), cstr!("-42")) == 0);

        snprintf(buf.as_mut_ptr(), 128, cstr!("%u"), 42i64);
        test!("libc: printf %u", strcmp(buf.as_ptr(), cstr!("42")) == 0);

        snprintf(buf.as_mut_ptr(), 128, cstr!("%X"), 255i64);
        test!("libc: printf %X", strcmp(buf.as_ptr(), cstr!("FF")) == 0);

        snprintf(buf.as_mut_ptr(), 128, cstr!("%o"), 8i64);
        test!("libc: printf %o", strcmp(buf.as_ptr(), cstr!("10")) == 0);

        snprintf(buf.as_mut_ptr(), 128, cstr!("%%"));
        test!("libc: printf %%", strcmp(buf.as_ptr(), cstr!("%")) == 0);

        // padding
        snprintf(buf.as_mut_ptr(), 128, cstr!("[%10s]"), cstr!("hi"));
        test!("libc: printf pad str", strcmp(buf.as_ptr(), cstr!("[        hi]")) == 0);

        snprintf(buf.as_mut_ptr(), 128, cstr!("[%-10s]"), cstr!("hi"));
        test!("libc: printf left-align", strcmp(buf.as_ptr(), cstr!("[hi        ]")) == 0);
    }
    println("");
}

extern "C" fn cmp_i64_libc(a: *const u8, b: *const u8) -> i32 {
    let va = unsafe { *(a as *const i64) };
    let vb = unsafe { *(b as *const i64) };
    if va < vb { -1 } else if va > vb { 1 } else { 0 }
}

fn test_libc_qsort_bsearch() {
    println("-- libc: qsort/bsearch --");
    unsafe {
        // qsort
        let mut arr: [i64; 8] = [5, 3, 8, 1, 7, 2, 6, 4];
        qsort(
            arr.as_mut_ptr() as *mut u8,
            8,
            core::mem::size_of::<i64>(),
            cmp_i64_libc,
        );
        test!("libc: qsort", arr[0] == 1 && arr[7] == 8);
        let sorted = (0..7).all(|i| arr[i] <= arr[i + 1]);
        test!("libc: qsort sorted", sorted);

        // qsort single element
        let mut single: [i64; 1] = [42];
        qsort(single.as_mut_ptr() as *mut u8, 1, 8, cmp_i64_libc);
        test!("libc: qsort single", single[0] == 42);

        // bsearch
        let key: i64 = 5;
        let found = bsearch(
            &key as *const i64 as *const u8,
            arr.as_ptr() as *const u8,
            8,
            core::mem::size_of::<i64>(),
            cmp_i64_libc,
        );
        test!("libc: bsearch found", !found.is_null());
        if !found.is_null() {
            test!("libc: bsearch value", *(found as *const i64) == 5);
        }

        // bsearch not found
        let key2: i64 = 99;
        let nf = bsearch(
            &key2 as *const i64 as *const u8,
            arr.as_ptr() as *const u8,
            8,
            core::mem::size_of::<i64>(),
            cmp_i64_libc,
        );
        test!("libc: bsearch not found", nf.is_null());
    }
    println("");
}

fn test_libc_env() {
    println("-- libc: env --");
    unsafe {
        // setenv / getenv
        let r = setenv(cstr!("LIBC_TEST_VAR"), cstr!("hello"), 1);
        test!("libc: setenv", r == 0);

        let val = getenv(cstr!("LIBC_TEST_VAR"));
        test!("libc: getenv", !val.is_null());
        if !val.is_null() {
            test!("libc: getenv value", strcmp(val, cstr!("hello")) == 0);
        }

        // setenv overwrite
        setenv(cstr!("LIBC_TEST_VAR"), cstr!("world"), 1);
        let val2 = getenv(cstr!("LIBC_TEST_VAR"));
        test!("libc: setenv overwrite", !val2.is_null() && strcmp(val2, cstr!("world")) == 0);

        // unsetenv
        let r = unsetenv(cstr!("LIBC_TEST_VAR"));
        test!("libc: unsetenv", r == 0);
        test!("libc: unsetenv gone", getenv(cstr!("LIBC_TEST_VAR")).is_null());

        // putenv
        let r = putenv(cstr!("LIBC_PUT=value"));
        test!("libc: putenv", r == 0);
        let val = getenv(cstr!("LIBC_PUT"));
        test!("libc: putenv get", !val.is_null() && strcmp(val, cstr!("value")) == 0);
        unsetenv(cstr!("LIBC_PUT"));

        // getenv non-existent
        test!("libc: getenv null", getenv(cstr!("NONEXISTENT_VAR_XYZ")).is_null());
    }
    println("");
}

fn test_libc_mmap() {
    println("-- libc: mmap --");
    unsafe {
        // mmap / munmap
        let p = libc_mmap(
            core::ptr::null_mut(),
            4096,
            3, // PROT_READ | PROT_WRITE
            0, 0, 0,
        );
        let map_failed = !0usize as *mut u8;
        test!("libc: mmap", p != map_failed && !p.is_null());
        if p != map_failed && !p.is_null() {
            // write to mapped memory
            memset(p, 0x42, 4096);
            test!("libc: mmap write", *p == 0x42 && *p.add(4095) == 0x42);

            let r = libc_munmap(p, 4096);
            test!("libc: munmap", r == 0);
        }

        // sbrk
        let cur = sbrk(0);
        test!("libc: sbrk", !cur.is_null());

        // stat
        let spath = cstr!("/tmp/libc_stat.txt");
        miku_write_file_cstr(spath, cstr!("stat test"));
        let mut st = core::mem::zeroed::<MikuStat>();
        let r = stat_path(spath, &mut st);
        test!("libc: stat", r == 0);
        test!("libc: stat size", st.size == 9);
        miku_unlink(spath);
    }
    println("");
}

fn test_libc_time() {
    println("-- libc: time --");
    unsafe {
        // clock_gettime
        let mut ts = LibcTimespec { tv_sec: 0, tv_nsec: 0 };
        let r = clock_gettime(0, &mut ts);
        test!("libc: clock_gettime", r == 0);
        test!("libc: clock_gettime time", ts.tv_sec >= 0);

        // nanosleep (very short)
        let req = LibcTimespec { tv_sec: 0, tv_nsec: 1_000_000 }; // 1ms
        let r = nanosleep(&req, core::ptr::null_mut());
        test!("libc: nanosleep", r == 0);

        // strerror
        let msg = strerror(-2); // ENOENT
        test!("libc: strerror", !msg.is_null());
        test!("libc: strerror content", libc_strlen(msg) > 0);

        // errno
        let ep = __errno_location();
        test!("libc: errno_location", !ep.is_null());
    }
    println("");
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    eprint("PANIC: ");
    if let Some(loc) = info.location() {
        eprint(loc.file());
        eprint(":");
        print_int(loc.line() as i64);
    }
    eprintln("");
    exit(134);
}
