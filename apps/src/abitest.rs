#![no_std]
#![no_main]

extern crate alloc;

pub use miku;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

static mut PASSED: u32 = 0;
static mut FAILED: u32 = 0;

fn check(name: &str, ok: bool) {
    unsafe {
        if ok {
            PASSED += 1;
        } else {
            FAILED += 1;
        }
    }
    miku::print(if ok { "  ok    " } else { "  fail  " });
    miku::println(name);
}

fn say_num(label: &str, v: u64) {
    let mut buf = [0u8; 24];
    let n = miku::fmt_dec(&mut buf, v);
    miku::print(label);
    miku::print(core::str::from_utf8(&buf[..n]).unwrap_or("?"));
    miku::println("");
}

fn slurp(path: &str) -> Option<Vec<u8>> {
    let (ptr, size) = miku::read_file(path)?;
    let out = unsafe { core::slice::from_raw_parts(ptr, size) }.to_vec();
    unsafe { miku::miku_free(ptr) };
    Some(out)
}

// Reads in tiny chunks to test offset handling on generator-backed files
fn slurp_in_chunks(path: &str) -> Option<Vec<u8>> {
    let fd = miku::open(path).ok()?;
    let mut out = Vec::new();
    let mut chunk = [0u8; 32];
    loop {
        let n = miku::read(fd, &mut chunk);
        if n <= 0 {
            break;
        }
        out.extend_from_slice(&chunk[..n as usize]);
    }
    miku::close(fd);
    Some(out)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| w == needle)
}

// Forces miku_memalign (over 16-byte alignment) to test its free path
#[repr(align(64))]
struct Aligned64([u8; 64]);

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    miku::println("abitest: alloc, entropy, the wall clock and /proc from ring 3");

    // 32 KB of data to ensure multiple reallocations.
    {
        let mut v: Vec<u64> = Vec::new();
        for i in 0..4096u64 {
            v.push(i * 3);
        }
        let mut ok = v.len() == 4096;
        for i in 0..4096u64 {
            if v[i as usize] != i * 3 {
                ok = false;
                break;
            }
        }
        check("vec/grow-and-keep", ok);
    }

    {
        let mut s = String::new();
        for _ in 0..500 {
            s.push_str("miku");
        }
        check(
            "string/grow",
            s.len() == 2000 && s.ends_with("miku") && s.starts_with("miku"),
        );
    }

    {
        let b = Box::new([7u8; 1024]);
        let ok = b[0] == 7 && b[1023] == 7;
        drop(b);
        check("box/alloc-and-drop", ok);
    }

    {
        let mut outer: Vec<Vec<u8>> = Vec::new();
        for i in 0..64u8 {
            let mut inner = Vec::new();
            for j in 0..64u8 {
                inner.push(i.wrapping_add(j));
            }
            outer.push(inner);
        }
        let ok = outer.len() == 64 && outer[63][63] == 63u8.wrapping_add(63);
        check("vec/nested", ok);
    }

    {
        let mut m: BTreeMap<u64, u64> = BTreeMap::new();
        for i in 0..1000u64 {
            m.insert(i.wrapping_mul(2654435761) % 4096, i);
        }
        let count = m.len();
        let mut ok = count > 0 && count <= 1000;
        for (k, _) in m.iter() {
            if *k >= 4096 {
                ok = false;
                break;
            }
        }
        check("btreemap/insert-iterate", ok);
    }

    {
        let a = Box::new(Aligned64([9u8; 64]));
        let addr = (&*a as *const Aligned64) as usize;
        let ok = addr % 64 == 0 && a.0[63] == 9;
        drop(a);
        check("align64/box", ok);
    }

    {
        let mut ok = true;
        for round in 0..64u64 {
            let mut v: Vec<u64> = Vec::with_capacity(8192);
            for i in 0..8192u64 {
                v.push(i ^ round);
            }
            if v[8191] != (8191u64 ^ round) {
                ok = false;
                break;
            }
        }
        check("heap/reuse-after-free", ok);
    }

    {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        let got_a = miku::getrandom(&mut a);
        let got_b = miku::getrandom(&mut b);
        check("getrandom/fills", got_a && got_b);
        check("getrandom/differs", a != b);
        check("getrandom/not-zero", a.iter().any(|&x| x != 0));
    }

    {
        let mut guard = [0xAAu8; 16];
        let ok = miku::getrandom(&mut guard[..5]) && guard[5..] == [0xAAu8; 11];
        check("getrandom/respects-length", ok);
    }

    // Wall clock, used for certificate validity
    {
        match miku::realtime() {
            // Before 2020-01-01 means the RTC was never read
            Some(secs) => check("realtime/plausible", secs > 1_577_836_800),
            None => check("realtime/plausible (clock not set - run ntp)", false),
        }
    }

    // Reads a synthetic file with `st_size` 0, such as procfs entries
    {
        let net = slurp("/proc/net");
        check(
            "procfs/net-readable",
            net.as_ref().map_or(false, |v| !v.is_empty()),
        );

        // The `dns:` line means the whole entry arrived
        check(
            "procfs/net-complete",
            net.as_ref().map_or(false, |v| contains(v, b"dns: ")),
        );

        // Another entry, to cover synthetic files as a class
        let mem = slurp("/proc/meminfo");
        check(
            "procfs/meminfo-readable",
            mem.as_ref().map_or(false, |v| !v.is_empty()),
        );

        // The same file through small chunked reads must match
        let by_hand = slurp_in_chunks("/proc/net");
        check(
            "procfs/both-paths-agree",
            match (&net, &by_hand) {
                (Some(a), Some(b)) => a == b && !a.is_empty(),
                _ => false,
            },
        );
    }

    // vDSO `clock_gettime` must agree with the syscall and be faster
    {
        use miku::time;

        let available = time::available();
        check("vdso/present", available);

        if available {
            // The vDSO's answer must fall between two surrounding syscall
            // answers
            let mut before = time::Timespec::default();
            let mut vdso = time::Timespec::default();
            let mut after = time::Timespec::default();

            let r1 = time::clock_gettime(time::CLOCK_MONOTONIC, &mut before);
            let r2 = time::clock_gettime(time::CLOCK_MONOTONIC, &mut vdso);
            let r3 = time::clock_gettime(time::CLOCK_MONOTONIC, &mut after);

            let ns = |t: &time::Timespec| (t.tv_sec as i128) * 1_000_000_000 + t.tv_nsec as i128;
            check("vdso/clock-ok", r1 == 0 && r2 == 0 && r3 == 0);
            check(
                "vdso/clock-monotonic",
                ns(&before) <= ns(&vdso) && ns(&vdso) <= ns(&after),
            );

            // The monotonic clock must advance
            let t0 = time::monotonic_ms();
            miku::sleep_ms(30);
            let t1 = time::monotonic_ms();
            check("vdso/clock-advances", t1 > t0 && t1 - t0 < 5000);

            // Current CPU from RDTSCP's auxiliary register. No answer is valid:
            // QEMU's `qemu64` lacks RDTSCP (CPUID.80000001H:EDX[27]), and
            // asking must return, not #UD
            match time::getcpu() {
                Some(c) => check("vdso/getcpu", c < 64),
                None => check("vdso/getcpu (no RDTSCP - answered, not faulted)", true),
            }
        }
    }

    // Exceeds former fixed limits, so a regression fails rather than slows

    // Over-aligned allocations past the old 16-entry table
    {
        #[repr(align(64))]
        #[derive(Clone, Copy)]
        struct Padded(u64);

        let mut held: Vec<Box<Padded>> = Vec::new();
        for i in 0..256u64 {
            held.push(Box::new(Padded(i)));
        }
        let mut ok = held.len() == 256;
        for (i, b) in held.iter().enumerate() {
            if b.0 != i as u64 {
                ok = false;
                break;
            }
            if (&**b as *const Padded as usize) & 63 != 0 {
                ok = false;
                break;
            }
        }
        check("heap/256-live-over-aligned", ok);
        // Freeing them must not corrupt the heap
        drop(held);
        let after: Vec<u64> = (0..1024u64).collect();
        check(
            "heap/usable-after-aligned-free",
            after.len() == 1024 && after[1023] == 1023,
        );
    }

    // Environment past the old limits of 128 entries, 63-byte keys, and
    // 255-byte values
    {
        let mut ok = true;
        // Past the old entry ceiling
        for i in 0..300u32 {
            let mut key = [0u8; 24];
            let n = miku::fmt_dec(&mut key, i as u64);
            let mut k = [0u8; 32];
            k[0] = b'T';
            k[1] = b'_';
            k[2..2 + n].copy_from_slice(&key[..n]);
            k[2 + n] = 0;
            if !unsafe { miku::miku_setenv(k.as_ptr(), b"v\0".as_ptr()) } {
                ok = false;
                break;
            }
        }
        check("env/300-variables", ok);

        // Past the old value ceiling, and read back whole
        let mut long = [b'x'; 1024];
        long[1023] = 0;
        unsafe { miku::miku_setenv(b"LONG\0".as_ptr(), long.as_ptr()) };
        let got = unsafe { miku::miku_getenv(b"LONG\0".as_ptr()) };
        let glen = if got.is_null() {
            0
        } else {
            unsafe { miku::miku_strlen(got) }
        };
        check("env/1023-byte-value", glen == 1023);

        // Two `getenv` answers alive at once
        unsafe {
            miku::miku_setenv(b"A\0".as_ptr(), b"alpha\0".as_ptr());
            miku::miku_setenv(b"B\0".as_ptr(), b"beta\0".as_ptr());
            let a = miku::miku_getenv(b"A\0".as_ptr());
            let b = miku::miku_getenv(b"B\0".as_ptr());
            let a_ok = !a.is_null() && *a == b'a';
            let b_ok = !b.is_null() && *b == b'b';
            check("env/two-answers-at-once", a_ok && b_ok);
        }
    }

    // Reads a directory past its 64th name
    {
        const DIR: &[u8] = b"/tmp/abitest.d\0";
        unsafe { miku::miku_mkdir(DIR.as_ptr(), 0o755) };
        let mut made = 0u32;
        for i in 0..100u32 {
            let mut num = [0u8; 24];
            let n = miku::fmt_dec(&mut num, i as u64);
            let mut path = [0u8; 64];
            let base = DIR.len() - 1;
            path[..base].copy_from_slice(&DIR[..base]);
            path[base] = b'/';
            path[base + 1..base + 1 + n].copy_from_slice(&num[..n]);
            path[base + 1 + n] = 0;
            let fd = unsafe { miku::miku_create(path.as_ptr(), 0o644) };
            if fd >= 0 {
                unsafe { miku::miku_close(fd) };
                made += 1;
            }
        }
        // Only if the files exist
        if made >= 70 {
            let listed = match miku_common_read_dir(DIR) {
                Some(n) => n,
                None => 0,
            };
            check("getdents/past-64-entries", listed >= made as usize);
        } else {
            check("getdents/past-64-entries (files not created)", false);
        }
    }

    // `lstat` describes the link, `stat` its target
    {
        const TARGET: &[u8] = b"/tmp/abitest.target\0";
        const LINK: &[u8] = b"/tmp/abitest.link\0";
        let fd = unsafe { miku::miku_create(TARGET.as_ptr(), 0o644) };
        if fd >= 0 {
            unsafe { miku::miku_close(fd) };
        }
        let made = unsafe { miku::miku_symlink(TARGET.as_ptr(), LINK.as_ptr()) };

        let mut st_follow: miku::MikuStat = unsafe { core::mem::zeroed() };
        let mut st_link: miku::MikuStat = unsafe { core::mem::zeroed() };
        let a = unsafe { miku::miku_stat(LINK.as_ptr(), &mut st_follow) };
        let b = unsafe { miku::miku_lstat(LINK.as_ptr(), &mut st_link) };
        // One is a regular file and the other a link, so they must differ
        check(
            "lstat/differs-from-stat",
            made >= 0 && a == 0 && b == 0 && st_follow.kind != st_link.kind,
        );
    }

    // `O_NONBLOCK` on an empty pipe
    {
        let mut fds = [0i64; 2];
        let r = unsafe { miku::miku_pipe(fds.as_mut_ptr()) };
        if r >= 0 {
            let rd = fds[0] as i64;
            let set = unsafe { miku::miku_set_nonblocking(rd, true) };
            let mut buf = [0u8; 8];
            let n = unsafe { miku::miku_read(rd as u64, buf.as_mut_ptr(), buf.len()) };
            // -EAGAIN is -11; a hang here is the failure
            check("fcntl/nonblocking-read-returns", set >= 0 && n == -11);
            unsafe {
                miku::miku_close(fds[0]);
                miku::miku_close(fds[1])
            };
        } else {
            check("fcntl/nonblocking-read-returns (no pipe)", false);
        }
    }

    // `O_NONBLOCK` set by `fcntl` on a socket (above `SOCK_FD_BASE`, outside
    // the VFS table). A socketpair needs no network. The read must return
    // `-EAGAIN` at once rather than wait 30 s
    {
        let mut pair = [0u64; 2];
        let made = unsafe { miku::miku_socketpair(pair.as_mut_ptr()) };
        if made >= 0 {
            let rd = pair[0] as i64;
            let before = miku::uptime_ms();
            let set = unsafe { miku::miku_set_nonblocking(rd, true) };
            let mut buf = [0u8; 8];
            let n = unsafe { miku::miku_recv(rd, buf.as_mut_ptr(), buf.len()) };
            let took = miku::uptime_ms().wrapping_sub(before);
            check(
                "fcntl/socket-nonblocking-recv-returns",
                set >= 0 && n == -11,
            );
            // Checks it did not wait
            check("fcntl/socket-nonblocking-does-not-wait", took < 1000);
            unsafe {
                miku::miku_close(pair[0] as i64);
                miku::miku_close(pair[1] as i64)
            };
        } else {
            check(
                "fcntl/socket-nonblocking-recv-returns (no socketpair)",
                false,
            );
        }
    }

    miku::println("");
    unsafe {
        say_num("passed: ", PASSED as u64);
        say_num("failed: ", FAILED as u64);
        miku::exit(if FAILED == 0 { 0 } else { 1 });
    }
}

/// Counts entries in as many passes as needed, ending only when the kernel says
/// there are no more
fn miku_common_read_dir(path: &[u8]) -> Option<usize> {
    // O_READ | O_DIRECTORY
    let fd = unsafe { miku::miku_open(path.as_ptr(), path.len() - 1, 0x0001 | 0x0040, 0) };
    if fd < 0 {
        return None;
    }
    const BATCH: usize = 16;
    let mut ents: Vec<miku::MikuDirent> = Vec::with_capacity(BATCH);
    for _ in 0..BATCH {
        ents.push(miku::MikuDirent {
            name: [0; 256],
            inode_id: 0,
            kind: 0,
            name_len: 0,
            _reserved: [0; 6],
        });
    }
    let mut total = 0usize;
    let mut offset = 0usize;
    loop {
        let n = unsafe { miku::miku_getdents(fd, ents.as_mut_ptr(), BATCH, offset) };
        if n <= 0 {
            break;
        }
        total += n as usize;
        offset += n as usize;
    }
    unsafe { miku::miku_close(fd) };
    // `.` and `..` are entries too, and the caller asked about files
    Some(total.saturating_sub(2))
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::eprintln("abitest: panic");
    miku::exit(1);
}
