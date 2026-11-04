#![feature(miku_ext)]

//! `std::fs` on MikuOS: writing, reading, directory walks, metadata, following
//! and refusing symlinks, and tree deletion. `read_dir` past 64 entries needs
//! `getdents`(111), and `symlink_metadata` needs `lstat`(112)

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

fn check(name: &str, ok: bool, failed: &mut i32) {
    println!("  {}  {name}", if ok { "ok  " } else { "FAIL" });
    if !ok {
        *failed += 1;
    }
}

/// Nanoseconds on the monotonic clock, which two processes agree on
fn mono_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Process-shared mutex page: mutex at 0, parent's unlock time at
/// `UNLOCKED_AT`, child's latency at `LATENCY`
const UNLOCKED_AT: usize = 256;
const LATENCY: usize = 264;

/// Map the first page of `path` shared, or null
fn map_shared_page(path: &Path) -> *mut u8 {
    use std::os::fd::AsRawFd;
    let Ok(f) = fs::OpenOptions::new().read(true).write(true).open(path) else {
        return std::ptr::null_mut();
    };
    let p = unsafe {
        libc::mmap(std::ptr::null_mut(), 4096, libc::PROT_READ | libc::PROT_WRITE,
                   libc::MAP_SHARED, f.as_raw_fd(), 0)
    };
    if p == libc::MAP_FAILED { std::ptr::null_mut() } else { p as *mut u8 }
}

/// `stdfs --pshared-child PATH`: takes the parent's mutex and reports the
/// latency from its unlock
fn pshared_child(path: &Path) -> ! {
    let page = map_shared_page(path);
    if page.is_null() {
        std::process::exit(2);
    }
    unsafe {
        let m = page as *mut libc::pthread_mutex_t;
        libc::pthread_mutex_lock(m);
        let lat = mono_ns().saturating_sub((page.add(UNLOCKED_AT) as *const u64).read_volatile());
        (page.add(LATENCY) as *mut u64).write_volatile(lat);
        libc::pthread_mutex_unlock(m);
    }
    std::process::exit(0);
}

/// `stdfs --read <path>`: reads the file twice with 64 KiB `read(2)`s, timing
/// each pass with an FNV-1a sum. The first pass fills the page cache, the
/// second is served from it.
///
/// A second thread polls `/tmp` metadata during the first pass and reports its
/// slowest answer
fn read_twice(path: &str) -> ! {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    let stop = Arc::new(AtomicBool::new(false));
    let worst = Arc::new(AtomicU64::new(0));
    let asks = Arc::new(AtomicU64::new(0));
    let mut prober = Some({
        let (stop, worst, asks) = (stop.clone(), worst.clone(), asks.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let t0 = mono_ns();
                let _ = fs::metadata("/tmp");
                worst.fetch_max(mono_ns() - t0, Ordering::Relaxed);
                asks.fetch_add(1, Ordering::Relaxed);
            }
        })
    });
    let pass = || -> std::io::Result<(u64, u64, u64)> {
        let mut f = fs::File::open(path)?;
        let mut buf = vec![0u8; 64 << 10];
        let (mut total, mut sum, mut ns) = (0u64, 0xcbf2_9ce4_8422_2325u64, 0u64);
        loop {
            // Only reads are timed
            let t0 = mono_ns();
            let n = f.read(&mut buf)?;
            ns += mono_ns() - t0;
            if n == 0 {
                break;
            }
            for &b in &buf[..n] {
                sum = (sum ^ b as u64).wrapping_mul(0x100_0000_01b3);
            }
            total += n as u64;
        }
        Ok((total, ns, sum))
    };
    for name in ["cold", "warm"] {
        let result = pass();
        if name == "cold" {
            stop.store(true, Ordering::Relaxed);
            if let Some(p) = prober.take() {
                let _ = p.join();
            }
            println!(
                "stdfs: during the cold read, {} stats of /tmp, the slowest {} us",
                asks.load(Ordering::Relaxed),
                worst.load(Ordering::Relaxed) / 1000
            );
        }
        match result {
            Ok((bytes, ns, sum)) => println!(
                "stdfs: {name} read of {path}: {bytes} bytes in {} us ({} MB/s), fnv {sum:016x}",
                ns / 1000,
                bytes * 1000 / ns.max(1)
            ),
            Err(e) => println!("stdfs: {name} read of {path} failed: {e}"),
        }
    }
    std::process::exit(0)
}

/// `stdfs --walk <dir>`: walks the tree twice like `find` (`read_dir`,
/// `symlink_metadata` per name), timing each pass. The first fills the name and
/// inode caches, the second is served from them. Then each directory is asked
/// twice for four missing names. Each phase prints the change in `/proc/stat`'s
/// `dcache:` line
fn walk_twice(dir: &str) -> ! {
    fn dcache() -> [u64; 4] {
        let stat = fs::read_to_string("/proc/stat").unwrap_or_default();
        let mut out = [0; 4];
        if let Some(line) = stat.lines().find(|l| l.starts_with("dcache:")) {
            for (o, v) in out.iter_mut().zip(line[7..].split_whitespace()) {
                *o = v.parse().unwrap_or(0);
            }
        }
        out
    }
    fn walk(dir: &Path, dirs: &mut Vec<std::path::PathBuf>) -> (u64, u64) {
        dirs.push(dir.to_path_buf());
        let (mut names, mut bytes) = (0, 0);
        let Ok(rd) = fs::read_dir(dir) else { return (0, 0) };
        for e in rd.flatten() {
            let p = e.path();
            names += 1;
            match fs::symlink_metadata(&p) {
                Ok(m) if m.is_dir() => {
                    let (n, b) = walk(&p, dirs);
                    names += n;
                    bytes += b;
                }
                Ok(m) => bytes += m.len(),
                Err(_) => {}
            }
        }
        (names, bytes)
    }
    let phase = |name: &str, before: [u64; 4], ns: u64, what: String| {
        let after = dcache();
        println!(
            "stdfs: {name}: {what} in {} us; filesystem lookups {}, negative hits {}, complete misses {}, icache {}",
            ns / 1000,
            after[0] - before[0],
            after[1] - before[1],
            after[2] - before[2],
            after[3]
        );
    };
    let mut dirs = Vec::new();
    for name in ["cold walk", "warm walk"] {
        dirs.clear();
        let before = dcache();
        let t0 = mono_ns();
        let (names, bytes) = walk(Path::new(dir), &mut dirs);
        phase(name, before, mono_ns() - t0, format!("{names} names, {} dirs, {bytes} bytes", dirs.len()));
    }
    for name in ["missing names", "missing names again"] {
        let before = dcache();
        let t0 = mono_ns();
        let mut asked = 0;
        for d in &dirs {
            for i in 0..4 {
                let _ = fs::symlink_metadata(d.join(format!("no-such-name-{i}")));
                asked += 1;
            }
        }
        phase(name, before, mono_ns() - t0, format!("{asked} stats"));
    }
    std::process::exit(0)
}

/// `stdfs --get <host:port> <path>`: one HTTP/1.0 GET over `std::net`, twice,
/// with body size, time, and FNV-1a
fn get_twice(addr: &str, path: &str) -> ! {
    use std::net::TcpStream;
    let fetch = || -> std::io::Result<(u64, u64, u64)> {
        let t0 = mono_ns();
        let mut s = TcpStream::connect(addr)?;
        s.write_all(format!("GET {path} HTTP/1.0\r\nHost: {addr}\r\n\r\n").as_bytes())?;
        let mut buf = vec![0u8; 64 << 10];
        let (mut head, mut body, mut sum) = (Vec::new(), 0u64, 0xcbf2_9ce4_8422_2325u64);
        let mut in_body = false;
        loop {
            let n = s.read(&mut buf)?;
            if n == 0 {
                break;
            }
            let mut data = &buf[..n];
            if !in_body {
                head.extend_from_slice(data);
                let Some(end) = head.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                let rest = head.split_off(end + 4);
                in_body = true;
                for &b in &rest {
                    sum = (sum ^ b as u64).wrapping_mul(0x100_0000_01b3);
                }
                body += rest.len() as u64;
                data = &[];
            }
            for &b in data {
                sum = (sum ^ b as u64).wrapping_mul(0x100_0000_01b3);
            }
            body += data.len() as u64;
        }
        Ok((body, mono_ns() - t0, sum))
    };
    for round in 1..=2 {
        match fetch() {
            Ok((bytes, ns, sum)) => println!(
                "stdfs: get {round} of {addr}{path}: {bytes} bytes in {} ms ({} KB/s), fnv {sum:016x}",
                ns / 1_000_000,
                bytes * 1_000_000 / ns.max(1)
            ),
            Err(e) => println!("stdfs: get {round} of {addr}{path} failed: {e}"),
        }
    }
    std::process::exit(0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 4 && args[1] == "--get" {
        get_twice(&args[2], &args[3]);
    }
    if args.len() == 3 && args[1] == "--pshared-child" {
        pshared_child(Path::new(&args[2]));
    }
    if args.len() == 3 && args[1] == "--read" {
        read_twice(&args[2]);
    }
    if args.len() == 3 && args[1] == "--walk" {
        walk_twice(&args[2]);
    }
    println!("stdfs: files and directories through std");
    let mut failed = 0;
    let root = Path::new("/tmp/stdfs");

    // A clean slate, and the first use of `remove_dir_all`
    let _ = fs::remove_dir_all(root);
    check("create_dir_all", fs::create_dir_all(root.join("deep/er")).is_ok(), &mut failed);

    // Write, read back, and check the bytes are the bytes
    {
        let p = root.join("hello.txt");
        let wrote = fs::write(&p, b"miku").is_ok();
        let read = fs::read(&p).map(|v| v == b"miku").unwrap_or(false);
        check("write then read gives the same bytes", wrote && read, &mut failed);

        let meta = fs::metadata(&p);
        check(
            "metadata: a 4-byte regular file",
            meta.as_ref().map(|m| m.is_file() && m.len() == 4).unwrap_or(false),
            &mut failed,
        );
    }

    // `File` handle: append, seek, positional read
    {
        let p = root.join("seek.txt");
        {
            let mut f = fs::File::create(&p).unwrap();
            f.write_all(b"0123456789").unwrap();
        }
        let mut f = fs::File::open(&p).unwrap();
        let mut buf = [0u8; 4];
        let seeked = f.seek(SeekFrom::Start(6)).map(|n| n == 6).unwrap_or(false);
        let got = f.read_exact(&mut buf).is_ok() && &buf == b"6789";
        check("seek then read lands where it was told", seeked && got, &mut failed);

        let mut f = fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"ab").unwrap();
        let len = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        check("append writes at the end", len == 12, &mut failed);
    }

    // Directory with more entries than one `getdents` batch
    {
        let many = root.join("many");
        fs::create_dir_all(&many).unwrap();
        let mut made = 0;
        for i in 0..100 {
            if fs::write(many.join(format!("f{i:03}")), b"x").is_ok() {
                made += 1;
            }
        }
        let listed = fs::read_dir(&many).map(|d| d.count()).unwrap_or(0);
        check("100 files created", made == 100, &mut failed);
        check("read_dir returns all 100 - past the old 64 wall", listed == 100, &mut failed);

        // Names longer than 63 bytes come back whole
        let long = "n".repeat(200);
        fs::write(many.join(&long), b"y").unwrap();
        let found = fs::read_dir(&many)
            .map(|d| d.filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy() == long))
            .unwrap_or(false);
        check("a 200-character name survives the round trip", found, &mut failed);
    }

    // Symlinks, and the difference between the two ways of asking
    {
        let target = root.join("hello.txt");
        let link = root.join("link");
        let made = std::os::miku::fs::symlink(&target, &link).is_ok();
        let follow = fs::metadata(&link).map(|m| m.is_file()).unwrap_or(false);
        let no_follow = fs::symlink_metadata(&link).map(|m| m.is_symlink()).unwrap_or(false);
        check("symlink created", made, &mut failed);
        check("metadata follows it", follow, &mut failed);
        check("symlink_metadata does not - lstat(112)", no_follow, &mut failed);
        check(
            "read_link gives the target back",
            fs::read_link(&link).map(|p| p == target).unwrap_or(false),
            &mut failed,
        );
    }

    // `canonicalize` uses `realpath`(119), and `F_GETPATH` names a descriptor's
    // file the same way
    {
        let wanted = root.join("hello.txt");
        check(
            "canonicalize takes out .. and follows the link",
            fs::canonicalize(root.join("deep/er/../../link")).map(|p| p == wanted).unwrap_or(false),
            &mut failed,
        );
        check(
            "canonicalize of something missing is NotFound",
            fs::canonicalize(root.join("nope")).map_err(|e| e.kind())
                == Err(std::io::ErrorKind::NotFound),
            &mut failed,
        );
        let named = fs::File::open(&wanted).ok().and_then(|f| {
            use std::os::fd::AsRawFd;
            let mut buf = [0u8; 4096];
            let r = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
            let n = buf.iter().position(|&b| b == 0)?;
            (r == 0).then(|| std::path::PathBuf::from(String::from_utf8_lossy(&buf[..n]).into_owned()))
        });
        check("F_GETPATH names an open file", named.as_deref() == Some(wanted.as_path()), &mut failed);
        check(
            "env::consts says MikuOS, and unix",
            std::env::consts::OS == "miku" && std::env::consts::FAMILY == "unix",
            &mut failed,
        );
    }

    // Sets and reads back a file's times through `utimensat`(40)
    {
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let path = root.join("hello.txt");
        let by_file = fs::File::options().write(true).open(&path)
            .and_then(|f| f.set_modified(when)).is_ok();
        let read = fs::metadata(&path).and_then(|m| m.modified()).map(|t| t == when).unwrap_or(false);
        check("File::set_modified, and metadata says so", by_file && read, &mut failed);
        let later = when + std::time::Duration::from_secs(60);
        let by_path = fs::File::options().write(true).open(&path)
            .and_then(|f| f.set_times(fs::FileTimes::new().set_accessed(later))).is_ok();
        let kept = fs::metadata(&path).and_then(|m| m.modified()).map(|t| t == when).unwrap_or(false);
        check("setting atime alone leaves mtime", by_path && kept, &mut failed);
    }

    // Mutex shared by two processes in a mapped tmpfs file. The kernel keys the
    // futex on the file's frame, so the unlock wakes the child rather than the
    // 100 ms backstop. Checked five times, since one backstop wake could pass
    // by luck
    {
        let path = root.join("pshared");
        let made = fs::write(&path, vec![0u8; 4096]).is_ok();
        let page = if made { map_shared_page(&path) } else { std::ptr::null_mut() };
        let mut init = false;
        if !page.is_null() {
            unsafe {
                let mut attr: libc::pthread_mutexattr_t = std::mem::zeroed();
                libc::pthread_mutexattr_init(&mut attr);
                init = libc::pthread_mutexattr_setpshared(&mut attr, libc::PTHREAD_PROCESS_SHARED) == 0
                    && libc::pthread_mutex_init(page as *mut libc::pthread_mutex_t, &attr) == 0;
            }
        }
        check("pthread_mutexattr_setpshared(PTHREAD_PROCESS_SHARED)", init, &mut failed);
        let mut worst = u64::MAX;
        if init {
            let me = std::env::current_exe().unwrap_or_else(|_| "/bin/stdfs".into());
            worst = 0;
            for _ in 0..5 {
                let m = page as *mut libc::pthread_mutex_t;
                unsafe {
                    libc::pthread_mutex_lock(m);
                    (page.add(LATENCY) as *mut u64).write_volatile(u64::MAX);
                }
                let child = std::process::Command::new(&me).arg("--pshared-child").arg(&path).spawn();
                let Ok(mut child) = child else {
                    unsafe { libc::pthread_mutex_unlock(m) };
                    worst = u64::MAX;
                    break;
                };
                // Long enough for the child to be asleep in the lock
                std::thread::sleep(std::time::Duration::from_millis(150));
                unsafe {
                    (page.add(UNLOCKED_AT) as *mut u64).write_volatile(mono_ns());
                    libc::pthread_mutex_unlock(m);
                }
                let ok = child.wait().map(|s| s.success()).unwrap_or(false);
                let lat = unsafe { (page.add(LATENCY) as *const u64).read_volatile() };
                worst = worst.max(if ok { lat } else { u64::MAX });
            }
            unsafe { libc::munmap(page as *mut _, 4096) };
        }
        check(
            &format!("an unlock in one process wakes the waiter in another (worst {} ms)", worst / 1_000_000),
            worst < 40_000_000,
            &mut failed,
        );
        let _ = fs::remove_file(&path);
    }

    // Sets a mode through the descriptor: `chmod`(26) with an empty path and
    // the descriptor as base
    {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("hello.txt");
        let set = fs::File::open(&path)
            .and_then(|f| f.set_permissions(fs::Permissions::from_mode(0o755))).is_ok();
        let mode = fs::metadata(&path).map(|m| m.permissions().mode() & 0o777).unwrap_or(0);
        check(&format!("File::set_permissions through the descriptor ({set}, {mode:o})"), set && mode == 0o755, &mut failed);
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o644));
    }

    // A new file is dated now, in seconds comparable with `set_modified`
    {
        use std::time::{Duration, SystemTime};
        let path = root.join("dated.txt");
        let wrote = fs::write(&path, b"now").is_ok();
        let mtime = fs::metadata(&path).and_then(|m| m.modified()).ok();
        let now = SystemTime::now();
        let skew = mtime.map(|m| now.duration_since(m).unwrap_or_else(|e| e.duration()));
        let close = skew.is_some_and(|d| d <= Duration::from_secs(5));
        check(&format!("a new file's mtime is the wall clock ({skew:?})"), wrote && close, &mut failed);
        let _ = fs::remove_file(&path);
    }

    // A megabyte written and read back on tmpfs
    {
        let big = root.join("big.bin");
        let data: Vec<u8> = (0..1024 * 1024u32).map(|i| (i * 7 + i / 4096) as u8).collect();
        let wrote = fs::write(&big, &data).is_ok();
        let back = fs::read(&big).map(|b| b == data).unwrap_or(false);
        check("a 1 MiB file in tmpfs, written and read back whole", wrote && back, &mut failed);
        let _ = fs::remove_file(&big);
    }

    // Two `MAP_SHARED` mappings of a tmpfs file are one page, which `read` also
    // sees
    {
        use std::os::fd::AsRawFd;
        let path = root.join("shm");
        let f = fs::OpenOptions::new().read(true).write(true).create(true).open(&path);
        let ok = f.as_ref().map(|f| f.set_len(8192).is_ok()).unwrap_or(false);
        let (a, b) = match &f {
            Ok(f) => unsafe {
                let map = || libc::mmap(
                    std::ptr::null_mut(), 8192, libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED, f.as_raw_fd(), 0,
                );
                (map(), map())
            },
            Err(_) => (libc::MAP_FAILED, libc::MAP_FAILED),
        };
        let mapped = ok && a != libc::MAP_FAILED && b != libc::MAP_FAILED && a != b;
        let mut seen = false;
        let mut read_sees = false;
        let mut write_seen = false;
        if mapped {
            unsafe {
                *(a as *mut u8).add(4100) = 0x5a;
                seen = *(b as *const u8).add(4100) == 0x5a;
            }
            read_sees = fs::read(&path).map(|v| v.get(4100) == Some(&0x5a)).unwrap_or(false);
            let mut w = fs::OpenOptions::new().write(true).open(&path).unwrap();
            let _ = w.seek(SeekFrom::Start(10));
            let _ = w.write_all(&[0xa5]);
            write_seen = unsafe { *(a as *const u8).add(10) == 0xa5 };
            unsafe {
                libc::munmap(a, 8192);
                libc::munmap(b, 8192);
            }
        }
        check("two MAP_SHARED mappings of a tmpfs file", mapped, &mut failed);
        check("a store through one is seen through the other", seen, &mut failed);
        check("and by read", read_sees, &mut failed);
        check("and a write is seen through the mapping", write_seen, &mut failed);
        let _ = fs::remove_file(&path);
    }

    // *at calls: a path from a directory descriptor, carried in the high half
    // of the path length (docs/MikuOS_ABI.md, 3.2e)
    {
        use std::ffi::CString;
        let c = |s: &str| CString::new(s).unwrap();
        let dir = c(root.join("deep").to_str().unwrap());
        let dfd = unsafe { libc::open(dir.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) };
        check("open a directory for its descriptor", dfd >= 0, &mut failed);

        let made = unsafe { libc::mkdirat(dfd, c("at").as_ptr(), 0o755) } == 0;
        check("mkdirat lands under the descriptor", made && root.join("deep/at").is_dir(), &mut failed);

        let fd = unsafe {
            libc::openat(dfd, c("at/f.txt").as_ptr(), libc::O_WRONLY | libc::O_CREAT, 0o644)
        };
        let wrote = fd >= 0 && unsafe { libc::write(fd, b"at".as_ptr().cast(), 2) } == 2;
        if fd >= 0 { unsafe { libc::close(fd) }; }
        check(
            "openat creates relative to it",
            wrote && fs::read(root.join("deep/at/f.txt")).map(|v| v == b"at").unwrap_or(false),
            &mut failed,
        );

        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        let statted = unsafe { libc::fstatat(dfd, c("at/f.txt").as_ptr(), &mut st, 0) } == 0;
        check("fstatat sees it: 2 bytes", statted && st.st_size == 2, &mut failed);

        let moved = unsafe {
            libc::renameat(dfd, c("at/f.txt").as_ptr(), dfd, c("g.txt").as_ptr())
        } == 0;
        check("renameat between two relative paths", moved && root.join("deep/g.txt").is_file(), &mut failed);

        // An absolute path ignores the descriptor, even a closed one
        let abs = c(root.join("deep/g.txt").to_str().unwrap());
        check(
            "an absolute path ignores dirfd",
            unsafe { libc::fstatat(9999, abs.as_ptr(), &mut st, 0) } == 0,
            &mut failed,
        );

        // A file is not a valid base directory
        let ffd = unsafe { libc::open(abs.as_ptr(), libc::O_RDONLY) };
        let r = unsafe { libc::fstatat(ffd, c("x").as_ptr(), &mut st, 0) };
        check(
            "a file as the base is ENOTDIR",
            r < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOTDIR),
            &mut failed,
        );
        // ... but it is a descriptor `fstatvfs` can ask about
        let mut vfs: libc::statvfs = unsafe { std::mem::zeroed() };
        check(
            "fstatvfs on a file",
            unsafe { libc::fstatvfs(ffd, &mut vfs) } == 0 && vfs.f_bsize > 0,
            &mut failed,
        );
        unsafe { libc::close(ffd) };

        let gone = unsafe { libc::unlinkat(dfd, c("g.txt").as_ptr(), 0) } == 0
            && unsafe { libc::unlinkat(dfd, c("at").as_ptr(), libc::AT_REMOVEDIR) } == 0;
        check("unlinkat, and AT_REMOVEDIR", gone && !root.join("deep/at").exists(), &mut failed);

        // `fdopendir` over the same descriptor, which it then owns
        let dp = unsafe { libc::fdopendir(dfd) };
        let mut names = Vec::new();
        if !dp.is_null() {
            loop {
                let e = unsafe { libc::readdir(dp) };
                if e.is_null() { break; }
                names.push(unsafe { std::ffi::CStr::from_ptr((*e).d_name.as_ptr()) }.to_owned());
            }
            unsafe { libc::closedir(dp) };
        }
        check("fdopendir reads the directory", names.iter().any(|n| n.to_bytes() == b"er"), &mut failed);
    }

    // `fchdir`, then a single name relative to it
    {
        use std::ffi::CString;
        let here = std::env::current_dir().unwrap();
        let dir = CString::new(root.to_str().unwrap()).unwrap();
        let dfd = unsafe { libc::open(dir.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) };
        let changed = unsafe { libc::fchdir(dfd) } == 0;
        unsafe { libc::close(dfd) };
        check("fchdir", changed && std::env::current_dir().map(|d| d == root).unwrap_or(false), &mut failed);
        let made = fs::create_dir("relative").is_ok();
        check(
            "a bare name is made in the working directory",
            made && root.join("relative").is_dir() && !Path::new("/relative").exists(),
            &mut failed,
        );
        let _ = std::env::set_current_dir(here);
    }

    // Rename, then remove everything
    {
        let a = root.join("hello.txt");
        let b = root.join("renamed.txt");
        let ok = fs::rename(&a, &b).is_ok() && b.exists() && !a.exists();
        check("rename moves it", ok, &mut failed);
    }

    check("remove_dir_all takes the tree", fs::remove_dir_all(root).is_ok(), &mut failed);
    check("and it is gone", !root.exists(), &mut failed);

    println!("stdfs: {}", if failed == 0 { "all passed" } else { "FAILURES" });
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
