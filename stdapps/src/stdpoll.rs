//! Reactor test: `polling` over `poll`(76). `polling` needs no `epoll`: its
//! `poll(2)` backend's `cfg` is `unix`, so no patch is required.
//!
//! 1. a `Poller` exists, with its notification pipe non-blocking
//! 2. an idle descriptor is not readable, and the wait honors its timeout
//! 3. a readable descriptor is reported under its key
//! 4. `notify` wakes a wait
//! 5. `notify` from another thread during `poll`, draining the notifier until
//!    EAGAIN

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use polling::{Event, Events, Poller};

fn check(name: &str, ok: bool, failed: &mut i32) {
    println!("  {}  {name}", if ok { "ok  " } else { "FAIL" });
    if !ok {
        *failed += 1;
    }
}

fn main() {
    println!("stdpoll: a reactor through polling + poll(76)");
    let mut failed = 0;

    let poller = match Poller::new() {
        Ok(p) => {
            check("Poller::new - its notifier pipe was made", true, &mut failed);
            p
        }
        Err(e) => {
            check(&format!("Poller::new: {e}"), false, &mut failed);
            std::process::exit(1);
        }
    };

    let (mut rx, mut tx) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => {
            check(&format!("pipe: {e}"), false, &mut failed);
            std::process::exit(1);
        }
    };

    const KEY: usize = 7;
    let added = unsafe { poller.add(&rx, Event::readable(KEY)) }.is_ok();
    check("a descriptor can be registered", added, &mut failed);

    // Nothing written, so nothing ready; the wait must time out
    {
        let mut events = Events::new();
        let t0 = Instant::now();
        let r = poller.wait(&mut events, Some(Duration::from_millis(50)));
        let waited = t0.elapsed();
        check(
            "an idle descriptor reports nothing",
            r.is_ok() && events.iter().count() == 0,
            &mut failed,
        );
        check(
            "and the timeout is what ended the wait",
            waited >= Duration::from_millis(20) && waited < Duration::from_secs(5),
            &mut failed,
        );
    }

    // Readable now
    {
        tx.write_all(b"miku").unwrap();
        let _ = poller.modify(&rx, Event::readable(KEY));

        let mut events = Events::new();
        let r = poller.wait(&mut events, Some(Duration::from_secs(2)));
        let got: Vec<Event> = events.iter().collect();
        check(
            "a readable descriptor is reported, under its own key",
            r.is_ok() && got.len() == 1 && got[0].key == KEY && got[0].readable,
            &mut failed,
        );

        let mut buf = [0u8; 8];
        let n = rx.read(&mut buf).unwrap_or(0);
        check(
            "and the bytes it promised are there",
            n == 4 && &buf[..4] == b"miku",
            &mut failed,
        );
    }

    // `notify` is how one thread ends another's wait
    {
        let mut events = Events::new();
        poller.notify().unwrap();
        let t0 = Instant::now();
        let r = poller.wait(&mut events, Some(Duration::from_secs(2)));
        check(
            "notify ends a wait nothing else would have",
            r.is_ok() && t0.elapsed() < Duration::from_secs(1),
            &mut failed,
        );
    }

    // `notify` from another thread during `poll`: the notification drains with
    // `while read(..).is_ok() {}`, which ends only on EAGAIN from an empty
    // non-blocking pipe
    {
        let poller = std::sync::Arc::new(poller);
        let waker = {
            let p = poller.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                p.notify().unwrap();
            })
        };
        let mut events = Events::new();
        let t0 = Instant::now();
        let r = poller.wait(&mut events, Some(Duration::from_secs(3)));
        let waited = t0.elapsed();
        let _ = waker.join();
        check(
            "a notify from another thread ends a wait already in poll",
            r.is_ok() && waited >= Duration::from_millis(50) && waited < Duration::from_secs(2),
            &mut failed,
        );
        let _ = poller.delete(&rx);
    }

    println!("stdpoll: {}", if failed == 0 { "all passed" } else { "FAILURES" });
    std::process::exit(if failed == 0 { 0 } else { 1 });
}

