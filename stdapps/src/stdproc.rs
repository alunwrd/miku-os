//! `std::process::Command`: `fork`(43), `execve`(67), `wait4`(44). Checks that
//! a child runs, its exit code returns, non-zero is not success, output can be
//! captured, arguments and environment arrive, and a missing program is an
//! error

use std::process::{Command, Stdio};

fn check(name: &str, ok: bool, failed: &mut i32) {
    println!("  {}  {name}", if ok { "ok  " } else { "FAIL" });
    if !ok {
        *failed += 1;
    }
}

/// String comparison that prints both values on failure
fn check_eq(name: &str, got: &str, want: &str, failed: &mut i32) {
    let ok = got == want;
    if ok {
        println!("  ok    {name}");
    } else {
        println!("  FAIL  {name}: got {got:?}, wanted {want:?}");
        *failed += 1;
    }
}

/// `stdproc --prot-none-child`: writes a page, closes it with `PROT_NONE`, then
/// reads it. The read must kill the process
fn prot_none_child() -> ! {
    unsafe {
        let p = libc::mmap(std::ptr::null_mut(), 4096, libc::PROT_READ | libc::PROT_WRITE,
                           libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) as *mut u8;
        if p as *mut libc::c_void == libc::MAP_FAILED {
            std::process::exit(2);
        }
        p.write_volatile(42);
        libc::mprotect(p as *mut _, 4096, libc::PROT_NONE);
        let v = p.read_volatile();
        std::process::exit(if v == 42 { 3 } else { 4 });
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--prot-none-child") {
        prot_none_child();
    }
    // `stdproc --fd-probe N...`: exits 0 if none of those descriptors is open,
    // 1 otherwise
    if std::env::args().nth(1).as_deref() == Some("--fd-probe") {
        let open = std::env::args().skip(2)
            .filter_map(|a| a.parse::<i32>().ok())
            .any(|fd| unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0);
        std::process::exit(if open { 1 } else { 0 });
    }
    // `stdproc --churn N [fork]`: N short-lived threads doing heap work, with
    // two forking spawners when `fork` is given. Stress mode for the storm
    // harness
    if std::env::args().nth(1).as_deref() == Some("--churn") {
        let n: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(2000);
        let forking = std::env::args().nth(3).as_deref() == Some("fork");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let spawners: Vec<_> = (0..if forking { 2 } else { 0 })
            .map(|_| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    let mut told = 0;
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        match Command::new("/bin/echo").stdout(Stdio::null()).status() {
                            Ok(s) if s.success() => {}
                            other if told < 3 => { told += 1; eprintln!("stdproc: churn spawn {other:?}"); }
                            _ => {}
                        }
                    }
                })
            })
            .collect();
        for i in 0..n {
            let t = std::thread::spawn(move || {
                let v: Vec<String> = (0..16).map(|k| format!("{i}-{k}")).collect();
                v.iter().map(|s| s.len()).sum::<usize>()
            });
            std::hint::black_box(t.join().ok());
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for s in spawners { let _ = s.join(); }
        println!("stdproc: churn {n} done");
        std::process::exit(0);
    }
    // `stdproc --cow-iso N`: parent threads check a pattern in a static buffer
    // while N children overwrite their copy between `fork` and `exec`
    if std::env::args().nth(1).as_deref() == Some("--cow-iso") {
        use std::os::unix::process::CommandExt;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        static mut BUF: [u64; 16384] = [0; 16384];
        let n: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(200);
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let bad = std::sync::Arc::new(AtomicUsize::new(0));
        let checkers: Vec<_> = (0..3u64)
            .map(|id| {
                let (stop, bad) = (stop.clone(), bad.clone());
                std::thread::spawn(move || {
                    // Each checker owns a third of the buffer
                    let (lo, hi) = (id as usize * 5000, id as usize * 5000 + 5000);
                    let mut gen = 0u64;
                    while !stop.load(Ordering::Relaxed) {
                        gen += 1;
                        let seed = (id << 60) | gen;
                        unsafe {
                            let b = &mut *core::ptr::addr_of_mut!(BUF);
                            for i in lo..hi { b[i] = seed ^ i as u64; }
                            std::thread::yield_now();
                            if (lo..hi).any(|i| std::ptr::read_volatile(&b[i]) != seed ^ i as u64) {
                                bad.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                })
            })
            .collect();
        let spawners: Vec<_> = (0..2)
            .map(|_| std::thread::spawn(move || {
                for _ in 0..n / 2 {
                    let mut c = Command::new("/bin/echo");
                    c.stdout(Stdio::null());
                    unsafe {
                        c.pre_exec(|| {
                            let b = &mut *core::ptr::addr_of_mut!(BUF);
                            for w in b.iter_mut() { std::ptr::write_volatile(w, 0xC4C4_C4C4_C4C4_C4C4); }
                            Ok(())
                        });
                    }
                    let _ = c.status();
                }
            }))
            .collect();
        for s in spawners { let _ = s.join(); }
        stop.store(true, Ordering::Relaxed);
        for c in checkers { let _ = c.join(); }
        let bad = bad.load(Ordering::Relaxed);
        println!("stdproc: cow-iso {n} children, {bad} rounds saw a child's write");
        std::process::exit(if bad == 0 { 0 } else { 1 });
    }
    println!("stdproc: Command through std");
    let mut failed = 0;

    // A child runs and its status returns, using `echo` since there is no
    // `/bin/true`
    {
        let st = Command::new("/bin/echo").arg("x").stdout(Stdio::null()).status();
        check(
            "spawn a program and get its status",
            st.map(|s| s.success()).unwrap_or(false),
            &mut failed,
        );
    }

    // Its output, through a pipe this process reads
    {
        let out = Command::new("/bin/echo").arg("miku").output();
        let got = out
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        check_eq("capture stdout through a pipe", &got, "miku", &mut failed);
    }

    // Arguments arrive as arguments, in order
    {
        let out = Command::new("/bin/echo").arg("a").arg("b").arg("c").output();
        let got = out
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        check_eq("three arguments arrive in order", &got, "a b c", &mut failed);
    }

    // A missing program is an error from `spawn`, through errno on a
    // close-on-exec pipe
    {
        let st = Command::new("/bin/definitely-not-a-program").status();
        let kind = st.as_ref().err().map(|e| e.kind());
        check_eq(
            "a missing program is NotFound from spawn",
            &format!("{kind:?}"),
            "Some(NotFound)",
            &mut failed,
        );
    }

    // A bare name is looked up in PATH, as execvp does
    {
        let out = Command::new("echo").arg("found").output();
        let got = out
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|e| format!("error: {e}"));
        check_eq("a bare name is found through PATH", &got, "found", &mut failed);
    }

    // Missing from every PATH entry is NotFound, not the first directory's
    // error
    {
        let st = Command::new("definitely-not-a-program").env("PATH", "/nope:/bin").status();
        let kind = st.as_ref().err().map(|e| e.kind());
        check_eq(
            "a bare name nowhere on PATH is NotFound",
            &format!("{kind:?}"),
            "Some(NotFound)",
            &mut failed,
        );
    }

    // `env::split_paths` and `join_paths`
    {
        let parts: Vec<_> = std::env::split_paths("/bin:/usr/bin::/opt").collect();
        check_eq(
            "split_paths splits at colons",
            &format!("{parts:?}"),
            r#"["/bin", "/usr/bin", "", "/opt"]"#,
            &mut failed,
        );
        let joined = std::env::join_paths(["/a", "/b"]).map(|s| s.into_string().unwrap_or_default());
        check_eq("join_paths joins with a colon", &format!("{joined:?}"), r#"Ok("/a:/b")"#, &mut failed);
        check("join_paths refuses a colon inside a part", std::env::join_paths(["/a:b"]).is_err(), &mut failed);
    }

    // The environment the child is given is the one it sees
    {
        let out = Command::new("/bin/env").env("MIKU_TEST", "42").output();
        let got = out.map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
        check("an env var set here reaches the child", got.contains("MIKU_TEST=42"), &mut failed);
    }

    // Spawning while other threads write memory: `fork` makes pages
    // copy-on-write, and writes from other CPUs must not reach the child's
    // frame. Each thread has its own slot in one page and a register copy of
    // its expected value
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        #[repr(align(4096))]
        struct Page([u64; 8]);
        let page = Box::new(Page([0; 8]));
        let slots = Box::into_raw(page) as usize;
        let stop = Arc::new(AtomicBool::new(false));
        let deadline = Instant::now() + Duration::from_secs(20);

        let counters: Vec<_> = (0..3)
            .map(|i| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    let slot = (slots + i * 8) as *mut u64;
                    let mut n = 0u64;
                    while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                        for _ in 0..1000 {
                            // SAFETY: this thread's own slot in a page
                            // outliving the threads
                            unsafe { slot.write_volatile(slot.read_volatile() + 1) };
                            n += 1;
                        }
                    }
                    (n, unsafe { slot.read_volatile() })
                })
            })
            .collect();
        // A thread writing the same page between sleeps: its idle CPU dropped
        // the translation, so its write faults and copies the page while the
        // others keep writing
        let poker = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let slot = (slots + 7 * 8) as *mut u64;
                while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                    unsafe { slot.write_volatile(slot.read_volatile() + 1) };
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
        };
        let spawner = std::thread::spawn(|| {
            use std::os::unix::process::CommandExt;
            (0..20)
                .filter(|_| {
                    let mut cmd = Command::new("/bin/echo");
                    cmd.stdout(Stdio::null());
                    // Keeps the window between `fork` and `exec` open
                    unsafe {
                        cmd.pre_exec(|| {
                            std::thread::sleep(Duration::from_millis(20));
                            Ok(())
                        });
                    }
                    cmd.status().map(|s| s.success()).unwrap_or(false)
                })
                .count()
        });
        let spawned = spawner.join().unwrap_or(0);
        stop.store(true, Ordering::Relaxed);
        let _ = poker.join();
        let counts: Vec<(u64, u64)> = counters.into_iter().map(|t| t.join().unwrap_or((1, 0))).collect();
        drop(unsafe { Box::from_raw(slots as *mut Page) });
        check_eq("twenty spawns from a thread", &spawned.to_string(), "20", &mut failed);
        check_eq(
            "no write lost to a fork on another thread",
            &format!("{:?}", counts.iter().map(|&(n, s)| n == s).collect::<Vec<_>>()),
            "[true, true, true]",
            &mut failed,
        );
        if counts.iter().any(|&(n, s)| n != s) {
            println!("        (counted, stored) = {counts:?}");
        }
    }

    // Files, pipes, and duplicates from `std` are close-on-exec from creation
    {
        use std::os::fd::AsRawFd;
        let f = std::fs::File::open("/bin/stdproc");
        let (r, w) = std::io::pipe().expect("pipe");
        let dup = f.as_ref().ok().and_then(|f| f.try_clone().ok());
        let mut fds = vec![r.as_raw_fd(), w.as_raw_fd()];
        if let Ok(f) = &f { fds.push(f.as_raw_fd()); }
        if let Some(d) = &dup { fds.push(d.as_raw_fd()); }
        let me = std::env::current_exe().unwrap_or_else(|_| "/bin/stdproc".into());
        let st = Command::new(me).arg("--fd-probe").args(fds.iter().map(|n| n.to_string())).status();
        check(
            &format!("a child inherits none of the file, pipe or duplicate ({fds:?})"),
            st.map(|s| s.success()).unwrap_or(false) && fds.len() == 4,
            &mut failed,
        );
    }

    // SIGPIPE is ignored in a std program, so writing to a closed pipe is an
    // error. Restored to default in children
    {
        use std::io::{Read, Write};
        let (r, mut w) = std::io::pipe().expect("pipe");
        drop(r);
        let kind = w.write(b"x").err().map(|e| e.kind());
        check_eq("a write to a pipe with no reader is BrokenPipe", &format!("{kind:?}"),
                 "Some(BrokenPipe)", &mut failed);

        use std::os::unix::process::ExitStatusExt;
        let big = "/tmp/stdproc-big";
        let _ = std::fs::write(big, vec![b'a'; 1 << 20]);
        let how = match Command::new("/bin/cat").arg(big).stdout(Stdio::piped()).spawn() {
            Ok(mut child) => {
                let mut one = [0u8; 1];
                let _ = child.stdout.as_mut().map(|o| o.read(&mut one));
                drop(child.stdout.take());
                child.wait().map(|s| match (s.code(), s.signal()) {
                    (_, Some(sig)) => format!("signal {sig}"),
                    (Some(c), None) => format!("exit {c}"),
                    _ => "?".into(),
                }).unwrap_or_else(|e| format!("error: {e}"))
            }
            Err(e) => format!("error: {e}"),
        };
        let _ = std::fs::remove_file(big);
        check_eq("a child gets SIGPIPE back at its default", &how, "signal 13", &mut failed);
    }

    // `mprotect(PROT_NONE)` on a resident page also blocks reads
    {
        use std::os::unix::process::ExitStatusExt;
        let me = std::env::current_exe().unwrap_or_else(|_| "/bin/stdproc".into());
        let st = Command::new(me).arg("--prot-none-child").status();
        let how = st.map(|s| match (s.code(), s.signal()) {
            (_, Some(sig)) => format!("signal {sig}"),
            (Some(c), None) => format!("exit {c}"),
            _ => "?".into(),
        }).unwrap_or_else(|e| format!("error: {e}"));
        check_eq("a read of a PROT_NONE page is SIGSEGV", &how, "signal 11", &mut failed);
    }

    // Children exiting while the parent's threads come and go. A zombie is DEAD
    // before it leaves its CPU, so `wait4` must not free its stack early.
    // Counted in exits to catch lost ones
    {
        let spawners: Vec<_> = (0..4)
            .map(|_| std::thread::spawn(|| {
                let mut ok = 0;
                let mut first_bad = None;
                for _ in 0..40 {
                    match Command::new("/bin/echo").stdout(Stdio::null()).status() {
                        Ok(s) if s.success() => ok += 1,
                        other => { first_bad.get_or_insert(format!("{other:?}")); }
                    }
                }
                (ok, first_bad)
            }))
            .collect();
        let mut churned = 0;
        while !spawners.iter().all(|t| t.is_finished()) {
            std::thread::spawn(|| std::hint::black_box(0u64)).join().ok();
            churned += 1;
        }
        let mut exits = 0;
        let mut bad = None;
        for t in spawners {
            let (ok, b) = t.join().unwrap_or((0, Some("spawner panicked".into())));
            exits += ok;
            if bad.is_none() { bad = b; }
        }
        check(&format!("160 children from 4 threads over {churned} short-lived ones, every exit seen ({exits}, {bad:?})"),
            exits == 160, &mut failed);
    }

    // A thread's stack stays intact while siblings fork, since the child writes
    // its copy of the forking thread's stack
    {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::Arc;
        let stop = Arc::new(AtomicBool::new(false));
        let bad = Arc::new(AtomicUsize::new(0));
        let rounds = Arc::new(AtomicUsize::new(0));
        let checkers: Vec<_> = (0..4u64)
            .map(|id| {
                let (stop, bad, rounds) = (stop.clone(), bad.clone(), rounds.clone());
                std::thread::spawn(move || {
                    let mut buf = [0u64; 8192];
                    let mut gen = 0u64;
                    while !stop.load(Ordering::Relaxed) {
                        gen += 1;
                        let seed = (id << 56) ^ gen.wrapping_mul(0x9E37_79B9_7F4A_7C15);
                        for (i, w) in buf.iter_mut().enumerate() {
                            *w = seed ^ i as u64;
                        }
                        std::hint::black_box(&mut buf);
                        std::thread::yield_now();
                        if buf.iter().enumerate().any(|(i, &w)| w != seed ^ i as u64) {
                            bad.fetch_add(1, Ordering::Relaxed);
                        }
                        rounds.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect();
        // SIGCHLD handler, as `async-process` installs: each exit puts a signal
        // frame on some thread's stack
        static CHLD: AtomicUsize = AtomicUsize::new(0);
        extern "C" fn on_chld(_: i32) { CHLD.fetch_add(1, Ordering::Relaxed); }
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_chld as usize;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigaction(libc::SIGCHLD, &sa, std::ptr::null_mut());
        }
        let spawners: Vec<_> = (0..2)
            .map(|_| std::thread::spawn(|| {
                for _ in 0..40 {
                    let _ = Command::new("/bin/echo").stdout(Stdio::null()).status();
                }
            }))
            .collect();
        for t in spawners { let _ = t.join(); }
        stop.store(true, Ordering::Relaxed);
        for t in checkers { let _ = t.join(); }
        unsafe { libc::signal(libc::SIGCHLD, libc::SIG_DFL); }
        let chld = CHLD.load(Ordering::Relaxed);
        let (bad, rounds) = (bad.load(Ordering::Relaxed), rounds.load(Ordering::Relaxed));
        check(&format!("stacks of 4 threads intact across 80 forks and {chld} SIGCHLDs ({bad} bad of {rounds} rounds)"),
            bad == 0 && rounds > 0, &mut failed);
    }

    println!("stdproc: {}", if failed == 0 { "all passed" } else { "FAILURES" });
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
