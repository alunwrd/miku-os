//! inotify through the `inotify` crate and mio, as `notify` uses them
//! (dev/fs-architecture.md, "Watching").
//!
//! Covers create, write, close, chmod, renames paired by cookie, directory
//! create and remove, deleting a watched file, and removing a watch. Edge
//! cases: a blocking read woken from another thread, a mio poll wake, a
//! thousand writes as one `IN_MODIFY`, a pipe leaving no trace in `/`, a
//! refused write to the descriptor, and 200 instances created and closed
//! against a limit of 128.

use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::time::Duration;

use inotify::{EventMask, Inotify, WatchMask};

type Ev = (i32, EventMask, u32, Option<String>);

/// All queued events. They are raised inside the causing syscall, so no wait is
/// needed
fn drain(ino: &mut Inotify) -> Vec<Ev> {
    let mut buf = [0u8; 4096];
    let mut out = Vec::new();
    loop {
        match ino.read_events(&mut buf) {
            Ok(evs) => {
                let before = out.len();
                for e in evs {
                    out.push((
                        e.wd.get_watch_descriptor_id(),
                        e.mask,
                        e.cookie,
                        e.name.map(|n: &OsStr| n.to_string_lossy().into_owned()),
                    ));
                }
                if out.len() == before {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    out
}

fn has(evs: &[Ev], mask: EventMask, name: Option<&str>) -> bool {
    evs.iter().any(|(_, m, _, n)| m.contains(mask) && n.as_deref() == name)
}

fn check(name: &str, ok: bool, evs: &[Ev], failed: &mut i32) {
    if ok {
        println!("  ok    {name}");
    } else {
        println!("  FAIL  {name}: {evs:?}");
        *failed += 1;
    }
}

fn main() {
    println!("stdwatch: inotify through the inotify crate and mio");
    let mut failed = 0;
    let root = std::path::Path::new("/tmp/stdwatch");
    let _ = fs::remove_dir_all(root);
    fs::create_dir_all(root).expect("mkdir /tmp/stdwatch");

    let mut ino = match Inotify::init() {
        Ok(i) => i,
        Err(e) => {
            println!("  FAIL  inotify_init1: {e}");
            println!("stdwatch: FAILURES");
            std::process::exit(1);
        }
    };
    let dir_mask = WatchMask::CREATE
        | WatchMask::DELETE
        | WatchMask::MODIFY
        | WatchMask::ATTRIB
        | WatchMask::MOVED_FROM
        | WatchMask::MOVED_TO
        | WatchMask::CLOSE_WRITE
        | WatchMask::DELETE_SELF
        | WatchMask::MOVE_SELF;
    let wd = ino.watches().add(root, dir_mask);
    check("inotify_init1 and a watch on a directory", wd.is_ok(), &[], &mut failed);
    let wd = wd.map(|w| w.get_watch_descriptor_id()).unwrap_or(-1);

    // A file is made, written, closed
    {
        let mut f = fs::File::create(root.join("a.txt")).unwrap();
        f.write_all(b"miku").unwrap();
        drop(f);
        let evs = drain(&mut ino);
        check("create, write and close: IN_CREATE, IN_MODIFY, IN_CLOSE_WRITE with the name",
            has(&evs, EventMask::CREATE, Some("a.txt"))
                && has(&evs, EventMask::MODIFY, Some("a.txt"))
                && has(&evs, EventMask::CLOSE_WRITE, Some("a.txt"))
                && evs.iter().all(|e| e.0 == wd),
            &evs, &mut failed);
    }

    // Repeated writes coalesce into one IN_MODIFY
    {
        let mut f = fs::OpenOptions::new().append(true).open(root.join("a.txt")).unwrap();
        for _ in 0..1000 {
            f.write_all(b"x").unwrap();
        }
        let evs = drain(&mut ino);
        let n = evs.iter().filter(|e| e.1.contains(EventMask::MODIFY)).count();
        check("1000 writes in a row coalesce into one IN_MODIFY", n == 1, &evs, &mut failed);
        drop(f);
        drain(&mut ino);
    }

    // Metadata
    {
        use std::os::unix::fs::PermissionsExt;
        let r = fs::set_permissions(root.join("a.txt"), fs::Permissions::from_mode(0o600));
        let evs = drain(&mut ino);
        check("chmod: IN_ATTRIB", r.is_ok() && has(&evs, EventMask::ATTRIB, Some("a.txt")), &evs, &mut failed);
    }

    // A rename is two halves with one cookie
    {
        fs::rename(root.join("a.txt"), root.join("b.txt")).unwrap();
        let evs = drain(&mut ino);
        let from = evs.iter().find(|e| e.1.contains(EventMask::MOVED_FROM) && e.3.as_deref() == Some("a.txt"));
        let to = evs.iter().find(|e| e.1.contains(EventMask::MOVED_TO) && e.3.as_deref() == Some("b.txt"));
        check("rename: IN_MOVED_FROM and IN_MOVED_TO paired by a nonzero cookie",
            matches!((from, to), (Some(f), Some(t)) if f.2 == t.2 && f.2 != 0),
            &evs, &mut failed);
    }

    // A directory, and a move between directories
    {
        fs::create_dir(root.join("sub")).unwrap();
        let evs = drain(&mut ino);
        check("mkdir: IN_CREATE | IN_ISDIR", has(&evs, EventMask::CREATE | EventMask::ISDIR, Some("sub")),
            &evs, &mut failed);
        let sub_wd = ino.watches().add(root.join("sub"), dir_mask).map(|w| w.get_watch_descriptor_id()).unwrap_or(-2);
        fs::rename(root.join("b.txt"), root.join("sub/c.txt")).unwrap();
        let evs = drain(&mut ino);
        let from = evs.iter().find(|e| e.0 == wd && e.1.contains(EventMask::MOVED_FROM));
        let to = evs.iter().find(|e| e.0 == sub_wd && e.1.contains(EventMask::MOVED_TO) && e.3.as_deref() == Some("c.txt"));
        check("a move between two watched directories arrives at each, one cookie",
            matches!((from, to), (Some(f), Some(t)) if f.2 == t.2), &evs, &mut failed);
    }

    // A watch on a file, and the file deleted
    {
        let fwd = ino.watches().add(root.join("sub/c.txt"), WatchMask::DELETE_SELF | WatchMask::MODIFY)
            .map(|w| w.get_watch_descriptor_id()).unwrap_or(-3);
        fs::write(root.join("sub/c.txt"), b"y").unwrap();
        let evs = drain(&mut ino);
        check("a watch on the file hears IN_MODIFY with no name",
            evs.iter().any(|e| e.0 == fwd && e.1.contains(EventMask::MODIFY) && e.3.is_none()), &evs, &mut failed);
        fs::remove_file(root.join("sub/c.txt")).unwrap();
        let evs = drain(&mut ino);
        let del = evs.iter().position(|e| e.1.contains(EventMask::DELETE) && e.3.as_deref() == Some("c.txt"));
        let selfdel = evs.iter().position(|e| e.0 == fwd && e.1.contains(EventMask::DELETE_SELF));
        let ign = evs.iter().position(|e| e.0 == fwd && e.1.contains(EventMask::IGNORED));
        check("unlink: IN_DELETE in the directory, then IN_DELETE_SELF and IN_IGNORED on the file",
            del.is_some() && matches!((selfdel, ign), (Some(a), Some(b)) if a < b), &evs, &mut failed);
    }

    // rmdir, with the directory watched too
    {
        fs::remove_dir(root.join("sub")).unwrap();
        let evs = drain(&mut ino);
        check("rmdir: IN_DELETE | IN_ISDIR in the parent, the directory's own watch ends",
            has(&evs, EventMask::DELETE | EventMask::ISDIR, Some("sub"))
                && evs.iter().any(|e| e.1.contains(EventMask::DELETE_SELF))
                && evs.iter().any(|e| e.1.contains(EventMask::IGNORED)),
            &evs, &mut failed);
    }

    // rm_watch
    {
        let w = ino.watches().add(root, dir_mask).unwrap();
        check("adding a watch on the same directory hands back the same wd",
            w.get_watch_descriptor_id() == wd, &[], &mut failed);
        let r = ino.watches().remove(w);
        fs::write(root.join("after"), b"").unwrap();
        let evs = drain(&mut ino);
        check("rm_watch: IN_IGNORED, and nothing after it",
            r.is_ok() && evs.iter().any(|e| e.1.contains(EventMask::IGNORED))
                && !has(&evs, EventMask::CREATE, Some("after")),
            &evs, &mut failed);
    }

    // A blocking read that another thread's create wakes
    {
        let wd2 = ino.watches().add(root, WatchMask::CREATE).is_ok();
        let t = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            fs::write("/tmp/stdwatch/woke", b"").unwrap();
        });
        let mut buf = [0u8; 1024];
        let names: Vec<String> = ino.read_events_blocking(&mut buf)
            .map(|evs| evs.filter_map(|e| e.name.map(|n| n.to_string_lossy().into_owned())).collect())
            .unwrap_or_default();
        let _ = t.join();
        check("a blocking read wakes on another thread's create",
            wd2 && names.iter().any(|n| n == "woke"), &[], &mut failed);
        drain(&mut ino);
    }

    // mio, the way notify's inotify backend waits
    {
        let mut poll = mio::Poll::new().unwrap();
        let fd = ino.as_raw_fd();
        let reg = poll.registry().register(&mut mio::unix::SourceFd(&fd), mio::Token(7), mio::Interest::READABLE);
        let t = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            fs::write("/tmp/stdwatch/polled", b"").unwrap();
        });
        let mut events = mio::Events::with_capacity(4);
        let got = poll.poll(&mut events, Some(Duration::from_secs(5)));
        let ready = events.iter().any(|e| e.token() == mio::Token(7) && e.is_readable());
        let _ = t.join();
        let evs = drain(&mut ino);
        check("mio: the inotify descriptor becomes readable on an event",
            reg.is_ok() && got.is_ok() && ready && has(&evs, EventMask::CREATE, Some("polled")),
            &evs, &mut failed);
    }

    // Read-only descriptor; a buffer too small for one event is EINVAL
    {
        let fd = ino.as_raw_fd();
        let w = unsafe { libc::write(fd, b"x".as_ptr().cast(), 1) };
        let werr = std::io::Error::last_os_error().raw_os_error();
        fs::write(root.join("a-rather-long-file-name"), b"").unwrap();
        let mut small = [0u8; 8];
        let r = unsafe { libc::read(fd, small.as_mut_ptr().cast(), small.len()) };
        let rerr = std::io::Error::last_os_error().raw_os_error();
        drain(&mut ino);
        check("write is EINVAL, and so is a read too small for the next event",
            w == -1 && werr == Some(libc::EINVAL) && r == -1 && rerr == Some(libc::EINVAL),
            &[], &mut failed);
    }

    // A pipe is nowhere in the namespace any more
    {
        let mut top = Inotify::init().unwrap();
        let ok = top.watches().add("/", WatchMask::CREATE | WatchMask::DELETE).is_ok();
        let mut fds = [0; 2];
        let p = unsafe { libc::pipe(fds.as_mut_ptr()) };
        let evs = drain(&mut top);
        let listed = fs::read_dir("/").map(|d| d.flatten().any(|e| e.file_name().to_string_lossy().starts_with("_pipe_")))
            .unwrap_or(true);
        unsafe { libc::close(fds[0]); libc::close(fds[1]); }
        check("pipe(): no event on /, and no name in it",
            ok && p == 0 && evs.is_empty() && !listed, &evs, &mut failed);
    }

    // Closing ends an instance: 200 created and closed with a limit of 128
    {
        let mut made = 0;
        for _ in 0..200 {
            match Inotify::init() {
                Ok(i) => {
                    made += 1;
                    let _ = i.close();
                }
                Err(_) => break,
            }
        }
        check("200 instances made and closed, with 128 allowed at once", made == 200, &[], &mut failed);
    }

    let _ = fs::remove_dir_all(root);
    println!("stdwatch: {}", if failed == 0 { "all passed" } else { "FAILURES" });
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
