//! `std::thread`, `std::sync`, and `std::time`. `spawn` is
//! `thread_create`(106), `join` a futex word in its own mapping, and `Mutex`
//! and `Condvar` are upstream's `futex.rs`. Checks that a thread runs, four
//! share an `Arc`, a `Mutex` excludes, a `Condvar` wakes, thread-locals are per
//! thread, and `Instant` advances.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn check(name: &str, ok: bool) -> bool {
    println!("  {}  {name}", if ok { "ok  " } else { "FAIL" });
    ok
}

thread_local! {
    static MINE: std::cell::Cell<u64> = const { std::cell::Cell::new(0xA11CE) };
}

/// Uses `kib` KiB of stack a frame at a time
fn burn(kib: usize) -> usize {
    let mut frame = [0u8; 1024];
    std::hint::black_box(&mut frame);
    if kib == 0 { 0 } else { std::hint::black_box(burn(kib - 1)) + frame[kib % 1024] as usize }
}

fn main() {
    // `stdthreads --overflow`: unbounded recursion in a thread; the process
    // must die
    if std::env::args().nth(1).as_deref() == Some("--overflow") {
        let _ = thread::spawn(|| burn(usize::MAX)).join();
        println!("stdthreads: the overflow came back");
        std::process::exit(0);
    }
    // `stdthreads --swarm <threads> <yields>`: run queue load
    // (`dev/kernel-core.md`, "The run queue")
    let num = |i: usize, default: usize| {
        std::env::args().nth(i).and_then(|s| s.parse().ok()).unwrap_or(default)
    };
    match std::env::args().nth(1).as_deref() {
        Some("--swarm") => swarm(num(2, 1000), num(3, 100)),
        Some("--swarm-child") => swarm_child(num(2, 1000), num(3, 100)),
        _ => {}
    }

    println!("stdthreads: threads, locks and time through std");
    let mut failed = 0;

    // A thread runs, and its return value comes back through `join`
    {
        let h = thread::spawn(|| 40 + 2);
        let got = h.join();
        failed += !check("spawn/join returns the value", matches!(got, Ok(42))) as i32;
    }

    // Four threads, one counter
    {
        let n = Arc::new(AtomicU64::new(0));
        let mut hs = Vec::new();
        for _ in 0..4 {
            let n = Arc::clone(&n);
            hs.push(thread::spawn(move || {
                for _ in 0..1000 {
                    n.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        for h in hs {
            let _ = h.join();
        }
        failed += !check("four threads, one Arc", n.load(Ordering::SeqCst) == 4000) as i32;
    }

    // A non-atomic increment protected only by the lock
    {
        let m = Arc::new(Mutex::new(0u64));
        let mut hs = Vec::new();
        for _ in 0..4 {
            let m = Arc::clone(&m);
            hs.push(thread::spawn(move || {
                for _ in 0..500 {
                    let mut g = m.lock().unwrap();
                    let v = *g;
                    thread::yield_now();
                    *g = v + 1;
                }
            }));
        }
        for h in hs {
            let _ = h.join();
        }
        let got = *m.lock().unwrap();
        failed += !check("mutex excludes (exact count)", got == 2000) as i32;
    }

    // A condvar wakes a waiter. Without a futex this is a spin or a lie
    {
        let pair = Arc::new((Mutex::new(false), Condvar::new()));
        let them = Arc::clone(&pair);
        let h = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            let (lock, cv) = &*them;
            *lock.lock().unwrap() = true;
            cv.notify_one();
        });
        let (lock, cv) = &*pair;
        let mut ready = lock.lock().unwrap();
        while !*ready {
            ready = cv.wait(ready).unwrap();
        }
        drop(ready);
        let _ = h.join();
        failed += !check("condvar wakes its waiter", true) as i32;
    }

    // Per-thread locals, in `PT_TLS`
    {
        let main_before = MINE.with(|c| c.get());
        let h = thread::spawn(|| {
            let started_at = MINE.with(|c| c.get());
            MINE.with(|c| c.set(7));
            (started_at, MINE.with(|c| c.get()))
        });
        let (started_at, after) = h.join().unwrap();
        let main_after = MINE.with(|c| c.get());
        failed += !check("thread-local starts at its initialiser", started_at == 0xA11CE) as i32;
        failed += !check("thread-local is the thread's own", after == 7) as i32;
        failed += !check("and the first thread's is untouched",
                         main_before == 0xA11CE && main_after == 0xA11CE) as i32;
    }

    // Monotonic clock: `clock_gettime`(70)
    {
        let t0 = Instant::now();
        thread::sleep(Duration::from_millis(30));
        let elapsed = t0.elapsed();
        failed += !check("Instant advances across a sleep",
                         elapsed >= Duration::from_millis(20)) as i32;
        failed += !check("and not absurdly", elapsed < Duration::from_secs(5)) as i32;
    }

    // Detached threads: the kernel unmaps their stacks. Checks survival over
    // many
    {
        for _ in 0..32 {
            let _ = thread::spawn(|| thread::yield_now());
        }
        thread::sleep(Duration::from_millis(50));
        failed += !check("32 detached threads, nothing leaked into failure", true) as i32;
    }

    // Default stack is 2 MiB, as on Linux, with 1.5 MiB usable
    {
        let deep = thread::spawn(|| burn(1536)).join();
        failed += !check("a thread has 1.5 MiB of stack to use", deep.is_ok()) as i32;
    }

    // Overflow faults on the guard page
    {
        use std::os::unix::process::ExitStatusExt;
        let st = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--overflow")
            .status();
        println!("          child: {st:?}");
        failed += !check("a stack overflow dies on SIGSEGV",
                         matches!(st.map(|s| s.signal()), Ok(Some(11)))) as i32;
    }

    println!("stdthreads: {}", if failed == 0 { "all passed" } else { "FAILURES" });
    std::process::exit(if failed == 0 { 0 } else { 1 });
}

/// `--swarm`: times the child's exit from outside, from its last line to `wait`
/// returning
fn swarm(threads: usize, yields: usize) -> ! {
    use std::io::{BufRead, BufReader};
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--swarm-child", &threads.to_string(), &yields.to_string()])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("stdthreads: swarm child");
    let out = child.stdout.take().unwrap();
    let mut exiting = None;
    for line in BufReader::new(out).lines().map_while(Result::ok) {
        println!("{line}");
        if line.ends_with("exiting") {
            exiting = Some(Instant::now());
        }
    }
    let st = child.wait();
    if let Some(t) = exiting {
        println!("stdthreads: swarm exit took {} ms", t.elapsed().as_millis());
    }
    println!("stdthreads: swarm {}", if matches!(st, Ok(s) if s.success()) { "done" } else { "FAILED" });
    std::process::exit(0);
}

/// `threads` threads on small stacks, released together, each yielding `yields`
/// times, then parked on a `Condvar` until the exit ends them
fn swarm_child(threads: usize, yields: usize) -> ! {
    // (let go, how many are through)
    let state = Arc::new((Mutex::new((false, 0usize)), Condvar::new(), Condvar::new()));
    let t0 = Instant::now();
    let mut spawned = 0usize;
    for _ in 0..threads {
        let state = Arc::clone(&state);
        let h = thread::Builder::new().stack_size(64 * 1024).spawn(move || {
            let (m, go, through) = &*state;
            {
                let mut g = m.lock().unwrap();
                while !g.0 {
                    g = go.wait(g).unwrap();
                }
            }
            for _ in 0..yields {
                thread::yield_now();
            }
            m.lock().unwrap().1 += 1;
            through.notify_one();
            loop {
                thread::park();
            }
        });
        match h {
            Ok(_) => spawned += 1,
            Err(e) => {
                println!("stdthreads: swarm stopped spawning at {spawned}: {e}");
                break;
            }
        }
    }
    println!("stdthreads: swarm of {spawned} threads up in {} ms", t0.elapsed().as_millis());
    // Kernel stack usage and free memory
    if let Ok(info) = std::fs::read_to_string("/proc/meminfo") {
        for l in info.lines().filter(|l| l.starts_with("KernelStack:") || l.starts_with("MemFree:")) {
            println!("stdthreads: swarm up, {}", l.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }
    let (m, go, through) = &*state;
    let t1 = Instant::now();
    m.lock().unwrap().0 = true;
    go.notify_all();
    {
        let mut g = m.lock().unwrap();
        while g.1 < spawned {
            g = through.wait(g).unwrap();
        }
    }
    let ms = t1.elapsed().as_millis().max(1);
    let total = (spawned * yields) as u128;
    println!("stdthreads: swarm {spawned} woken, {yields} yields each, in {ms} ms ({} k yields/s)", total / ms);
    println!("stdthreads: swarm exiting");
    std::process::exit(0);
}
