//! `stdbench [dir]`: cost of common kernel paths in ns per operation, through
//! `std` only, so it also builds for Linux
//! (`rustc -O --edition 2021 stdbench.rs`). `dir` holds the file cases: `/tmp`
//! by default, or a mounted disk.
//!
//! Each case prints `name: N ns/op`. Scaling cases run on 1, 2, and 4 threads
//! and print the total rate.
//!
//! `stdbench [dir] --net HOST:PORT` also measures a connection, a small
//! request, and 64 MiB from an HTTP server with `small.txt` and `big64.bin`.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Condvar, Mutex};
use std::time::Instant;

fn report(name: &str, n: u64, ns: u128) {
    println!("stdbench: {name}: {} ns/op ({n} ops)", ns / n as u128);
}

fn time(name: &str, n: u64, mut f: impl FnMut(u64)) {
    f(0);
    let t = Instant::now();
    for i in 0..n {
        f(i);
    }
    report(name, n, t.elapsed().as_nanos());
}

/// Runs `f` on `threads` threads, `n` times each; prints the total rate
fn scale(name: &str, n: u64, f: impl Fn(usize, u64) + Send + Sync + 'static) {
    let f = Arc::new(f);
    let mut line = format!("stdbench: {name}:");
    for threads in [1usize, 2, 4] {
        let start = Arc::new(Barrier::new(threads + 1));
        let hs: Vec<_> = (0..threads)
            .map(|t| {
                let (f, start) = (f.clone(), start.clone());
                std::thread::spawn(move || {
                    start.wait();
                    for i in 0..n {
                        f(t, i);
                    }
                })
            })
            .collect();
        start.wait();
        let t0 = Instant::now();
        for h in hs {
            let _ = h.join();
        }
        let ns = t0.elapsed().as_nanos().max(1);
        let rate = (threads as u128 * n as u128) * 1_000_000_000 / ns;
        line += &format!(" {threads}t {rate}/s");
    }
    println!("{line}");
}

/// Connect, a small request, and 64 MiB, from an HTTP/1.0 server at `addr`
fn bench_net(addr: &str) {
    use std::net::TcpStream;
    let get = |path: &str| -> Option<usize> {
        let mut s = TcpStream::connect(addr).ok()?;
        s.write_all(format!("GET /{path} HTTP/1.0\r\nHost: x\r\n\r\n").as_bytes()).ok()?;
        let mut buf = vec![0u8; 1 << 16];
        let mut total = 0;
        loop {
            match s.read(&mut buf) {
                Ok(0) => return Some(total),
                Ok(n) => total += n,
                Err(_) => return None,
            }
        }
    };
    const N: u64 = 50;
    let (mut connect_ns, mut close_ns) = (0u128, 0u128);
    let mut ok = 0;
    for _ in 0..N {
        let t = Instant::now();
        let s = TcpStream::connect(addr);
        connect_ns += t.elapsed().as_nanos();
        if s.is_ok() {
            ok += 1;
        }
        let t = Instant::now();
        drop(s);
        close_ns += t.elapsed().as_nanos();
    }
    report(&format!("tcp connect to {addr} ({ok} of {N})"), N, connect_ns);
    report("tcp close", N, close_ns);
    let t = Instant::now();
    let mut ok = 0;
    for _ in 0..N {
        if get("small.txt").is_some_and(|n| n > 0) {
            ok += 1;
        }
    }
    report(&format!("http GET small.txt, connection each ({ok} of {N})"), N, t.elapsed().as_nanos());
    let t = Instant::now();
    match get("big64.bin") {
        Some(n) => {
            let ns = t.elapsed().as_nanos().max(1);
            println!("stdbench: http GET big64.bin: {} bytes, {} MB/s", n, n as u128 * 1000 / ns);
        }
        None => println!("stdbench: http GET big64.bin: failed"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("--exit") => std::process::exit(0),
        Some("--echo") => {
            // One byte in, one byte out, until the pipe closes
            let (mut i, mut o) = (std::io::stdin(), std::io::stdout());
            let mut b = [0u8; 1];
            while i.read(&mut b).map(|n| n == 1).unwrap_or(false) {
                let _ = o.write_all(&b);
                let _ = o.flush();
            }
            std::process::exit(0)
        }
        _ => {}
    }
    let net = args.iter().position(|a| a == "--net").and_then(|i| args.get(i + 1)).cloned();
    let dir = PathBuf::from(args.get(1).filter(|a| !a.starts_with("--")).map(|s| s.as_str()).unwrap_or("/tmp")).join("stdbench");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("a/b/c/d")).expect("stdbench: make the directory");
    println!("stdbench: in {}", dir.display());

    // the system call itself, and the clock
    time("getpid", 200_000, |_| {
        std::hint::black_box(std::process::id());
    });
    time("clock (vDSO)", 1_000_000, |_| {
        std::hint::black_box(Instant::now());
    });

    // names
    let deep = dir.join("a/b/c/d/file");
    File::create(&deep).unwrap().write_all(&[7u8; 4096]).unwrap();
    time("stat, short path", 100_000, |_| {
        let _ = std::hint::black_box(fs::metadata(&dir));
    });
    time("stat, 6 components", 100_000, |_| {
        let _ = std::hint::black_box(fs::metadata(&deep));
    });
    let missing = dir.join("a/b/c/d/no-such");
    time("stat, missing name", 100_000, |_| {
        let _ = std::hint::black_box(fs::metadata(&missing));
    });
    time("open+close", 50_000, |_| {
        drop(std::hint::black_box(File::open(&deep)));
    });

    // data
    let mut f = File::options().read(true).write(true).open(&deep).unwrap();
    let mut buf = [0u8; 4096];
    time("read 4 KiB, cached", 100_000, |_| {
        f.seek(SeekFrom::Start(0)).unwrap();
        let _ = f.read(&mut buf);
    });
    time("write 4 KiB, same place", 50_000, |_| {
        f.seek(SeekFrom::Start(0)).unwrap();
        let _ = f.write(&buf);
    });
    drop(f);
    {
        let big = dir.join("big");
        let chunk = vec![1u8; 1 << 20];
        let t = Instant::now();
        let mut f = File::create(&big).unwrap();
        for _ in 0..64 {
            f.write_all(&chunk).unwrap();
        }
        let written = t.elapsed().as_nanos().max(1);
        f.sync_all().unwrap();
        let ns = t.elapsed().as_nanos().max(1);
        println!("stdbench: write 64 MiB + fsync: {} MB/s", 64u128 * 1_048_576 * 1000 / ns);
        println!("stdbench:   of it the writes {} ms, the fsync {} ms", written / 1_000_000, (ns - written) / 1_000_000);
        let t = Instant::now();
        let mut f = File::open(&big).unwrap();
        let mut b = vec![0u8; 1 << 20];
        while f.read(&mut b).map(|n| n > 0).unwrap_or(false) {}
        let ns = t.elapsed().as_nanos().max(1);
        println!("stdbench: read it back: {} MB/s", 64u128 * 1_048_576 * 1000 / ns);
        let _ = fs::remove_file(&big);
    }
    {
        // Many small files created, then removed, like an untar or a build
        let small = dir.join("small");
        fs::create_dir(&small).unwrap();
        let t = Instant::now();
        for i in 0..2000 {
            File::create(small.join(format!("f{i:05}"))).unwrap().write_all(&[1u8; 1000]).unwrap();
        }
        report("create+write 1 KB file", 2000, t.elapsed().as_nanos());
        // Twenty listings, since a single one is noisy
        let t = Instant::now();
        let mut n = 0;
        for _ in 0..20 {
            n += fs::read_dir(&small).unwrap().count();
        }
        report("read_dir entry", n.max(1) as u64, t.elapsed().as_nanos());
        let t = Instant::now();
        for i in 0..2000 {
            let _ = fs::remove_file(small.join(format!("f{i:05}")));
        }
        report("unlink", 2000, t.elapsed().as_nanos());
    }
    {
        // `stat` of a cached path while another thread walks an uncached tree
        let root = dir.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("/"));
        let stop = Arc::new(AtomicBool::new(false));
        let walked = Arc::new(AtomicU64::new(0));
        let (s2, w2) = (stop.clone(), walked.clone());
        let h = std::thread::spawn(move || {
            let mut stack = vec![root];
            while let Some(d) = stack.pop() {
                let Ok(rd) = fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    if s2.load(Ordering::Relaxed) || w2.load(Ordering::Relaxed) >= 50_000 {
                        return;
                    }
                    let name = e.file_name();
                    if ["stdbench", "proc", "dev", "sys", "run"].iter().any(|n| name == *n) {
                        continue;
                    }
                    let p = e.path();
                    if fs::symlink_metadata(&p).map(|m| m.is_dir()).unwrap_or(false) {
                        stack.push(p);
                    }
                    w2.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
        let (mut n, mut total, mut worst) = (0u64, 0u128, 0u128);
        let t0 = Instant::now();
        while !h.is_finished() && t0.elapsed().as_secs() < 3 {
            let t = Instant::now();
            let _ = std::hint::black_box(fs::metadata(&deep));
            let ns = t.elapsed().as_nanos();
            total += ns;
            worst = worst.max(ns);
            n += 1;
        }
        stop.store(true, Ordering::Relaxed);
        let _ = h.join();
        println!(
            "stdbench: stat during a cold walk: {} ns/op, worst {} ns ({} ops, {} names walked)",
            total / n.max(1) as u128,
            worst,
            n,
            walked.load(Ordering::Relaxed)
        );
    }

    // waking another thread, and another process
    {
        let pair = Arc::new((Mutex::new(0u64), Condvar::new()));
        let p2 = pair.clone();
        const N: u64 = 20_000;
        let h = std::thread::spawn(move || {
            let (m, c) = &*p2;
            let mut g = m.lock().unwrap();
            for i in 0..N {
                while *g != 2 * i + 1 {
                    g = c.wait(g).unwrap();
                }
                *g += 1;
                c.notify_one();
            }
        });
        let (m, c) = &*pair;
        let t = Instant::now();
        let mut g = m.lock().unwrap();
        for i in 0..N {
            *g += 1;
            c.notify_one();
            while *g != 2 * i + 2 {
                g = c.wait(g).unwrap();
            }
        }
        drop(g);
        report("thread ping-pong (futex), round trip", N, t.elapsed().as_nanos());
        let _ = h.join();
    }
    {
        // One byte through a pipe and back on one thread, with no wakeups
        let (mut r, mut w) = std::io::pipe().unwrap();
        let mut b = [0u8; 1];
        time("pipe write+read, one thread, no waiting", 100_000, |_| {
            w.write_all(&[1]).unwrap();
            r.read_exact(&mut b).unwrap();
        });
    }
    {
        // Pipe ping-pong between two threads of this process
        let (mut r1, mut w1) = std::io::pipe().unwrap();
        let (mut r2, mut w2) = std::io::pipe().unwrap();
        let h = std::thread::spawn(move || {
            let mut b = [0u8; 1];
            while r1.read(&mut b).map(|n| n == 1).unwrap_or(false) {
                if w2.write_all(&b).is_err() {
                    break;
                }
            }
        });
        let mut b = [0u8; 1];
        time("pipe ping-pong between threads, round trip", 20_000, |_| {
            w1.write_all(&[1]).unwrap();
            r2.read_exact(&mut b).unwrap();
        });
        drop(w1);
        let _ = h.join();
    }
    {
        let me = std::env::current_exe().unwrap();
        let mut child = Command::new(&me)
            .arg("--echo")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let (mut i, mut o) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
        let mut b = [0u8; 1];
        time("pipe ping-pong between processes, round trip", 20_000, |_| {
            i.write_all(&[1]).unwrap();
            o.read_exact(&mut b).unwrap();
        });
        drop(i);
        let _ = child.wait();
    }

    // making things
    time("thread spawn+join", 2_000, |_| {
        std::thread::spawn(|| {}).join().unwrap();
    });
    let me = std::env::current_exe().unwrap();
    time("process spawn+wait", 300, |_| {
        Command::new(&me).arg("--exit").status().unwrap();
    });
    time("process spawn+wait, /bin/echo", 300, |_| {
        Command::new("/bin/echo").stdout(Stdio::null()).status().unwrap();
    });
    {
        const PAGES: usize = 16384;
        let t = Instant::now();
        let mut v: Vec<u8> = Vec::with_capacity(PAGES * 4096);
        unsafe { v.set_len(PAGES * 4096) };
        for p in 0..PAGES {
            v[p * 4096] = 1;
        }
        std::hint::black_box(&v);
        let touched = t.elapsed().as_nanos();
        let f = Instant::now();
        drop(v);
        let freed = f.elapsed().as_nanos();
        report("page fault (64 MiB, touched once, freed)", PAGES as u64, t.elapsed().as_nanos());
        report("  of which the faults", PAGES as u64, touched);
        report("  of which the free", PAGES as u64, freed);
    }

    // TLB: one cache line read from each page at random. Lines fit in the
    // caches at every size, so growth shows TLB misses
    for mib in [1usize, 16, 256] {
        let pages = mib * 256;
        let mut v: Vec<u8> = vec![0u8; pages * 4096];
        for p in 0..pages {
            v[p * 4096 + (p % 64) * 64] = 1;
        }
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut sum = 0u64;
        const N: u64 = 4_000_000;
        let t = Instant::now();
        for _ in 0..N {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let p = (x as usize) % pages;
            sum = sum.wrapping_add(v[p * 4096 + (p % 64) * 64] as u64);
        }
        std::hint::black_box(sum);
        report(&format!("random line per page over {mib} MiB"), N, t.elapsed().as_nanos());
    }

    // the network, when asked
    if let Some(addr) = net {
        bench_net(&addr);
    }

    // the same work on more threads at once
    scale("getpid scaling", 100_000, |_, _| {
        std::hint::black_box(std::process::id());
    });
    for t in 0..4 {
        File::create(dir.join(format!("a/b/c/d/t{t}"))).unwrap().write_all(&[3u8; 4096]).unwrap();
    }
    let d = dir.clone();
    scale("stat scaling", 50_000, move |t, _| {
        let _ = std::hint::black_box(fs::metadata(d.join(format!("a/b/c/d/t{t}"))));
    });
    let files: Vec<Mutex<File>> = (0..4).map(|t| Mutex::new(File::open(dir.join(format!("a/b/c/d/t{t}"))).unwrap())).collect();
    scale("read 4 KiB scaling", 50_000, move |t, _| {
        let mut f = files[t].lock().unwrap();
        let mut b = [0u8; 4096];
        f.seek(SeekFrom::Start(0)).unwrap();
        let _ = f.read(&mut b);
    });
    let d = dir.clone();
    scale("open+close scaling", 20_000, move |t, _| {
        drop(std::hint::black_box(File::open(d.join(format!("a/b/c/d/t{t}")))));
    });
    scale("4 KiB alloc+free scaling", 200_000, |_, _| {
        drop(std::hint::black_box(vec![0u8; 4096]));
    });

    let _ = fs::remove_dir_all(&dir);
    println!("stdbench: done");
}
