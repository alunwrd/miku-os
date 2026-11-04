// Selftest for the descriptor contract
//
// `read`, `write` and `poll` all route a descriptor through `classify`, and
// every readiness predicate has a blocking operation it has to agree with: if
// `poll` says a pipe is readable and the `read` that follows blocks, the
// program hangs.
//
// All offline: a pipe needs no hardware, and the cases (empty, written to,
// writer gone, never opened) are reachable in a few lines.

extern crate alloc;
use alloc::format;
use alloc::string::String;

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::kcore::selftest::{Report, Runner};
use crate::syscall::adapters::file::make_pipe;
use crate::syscall::adapters::io::{
    classify, revents_for, FdKind, POLLHUP, POLLIN, POLLNVAL, POLLOUT,
};

pub fn run() -> Report {
    let mut t = Runner::new();
    check_classify(&mut t);
    check_pipe_readiness(&mut t);
    check_pipe_revents(&mut t);
    check_pipe_ring(&mut t);
    check_pipe_ends(&mut t);
    check_invalid_and_ignored(&mut t);
    check_socket_range(&mut t);
    check_socketpair(&mut t);
    check_unix_pair_readiness(&mut t);
    check_unix_unconnected(&mut t);
    check_unix_names(&mut t);
    check_icmp_socket(&mut t);
    check_raw_net_device(&mut t);
    check_sleep_precision(&mut t);
    check_wait_deadline(&mut t);
    check_keyed_wake(&mut t);
    crate::sched::selftest::checks(&mut t);
    t.report()
}

// time and waiting (`dev/kernel-core.md`)

/// A millisecond's sleep is a millisecond, not the next 4 ms tick: what every
/// `nanosleep`, `std::thread::sleep` and futex timeout is made of
fn check_sleep_precision(t: &mut Runner) {
    if crate::arch::x86_64::apic::timer_mode() == crate::arch::x86_64::apic::TimerMode::Periodic {
        t.skip("time/sleep-1ms", String::from("periodic timer (no TSC, or `tickonly`): sleeps end on ticks"));
        return;
    }
    let (mut least, mut most, mut sum) = (u64::MAX, 0u64, 0u64);
    let (ticks0, deadlines0, arms0) = crate::arch::x86_64::apic::timer_counts();
    const N: u64 = 20;
    for _ in 0..N {
        let t0 = crate::kcore::time::now_ns();
        crate::sched::sleep_ns(1_000_000);
        let took = crate::kcore::time::now_ns() - t0;
        least = least.min(took);
        most = most.max(took);
        sum += took;
    }
    let mean = sum / N;
    let (ticks1, deadlines1, arms1) = crate::arch::x86_64::apic::timer_counts();
    t.check("time/sleep-1ms", least >= 1_000_000 && mean < 2_000_000 && most < 6_000_000,
            format!("{N} sleeps of 1 ms: {} us at least, {} us on average, {} us at most; \
                     meanwhile {} ticks, {} deadlines and {} early arms on all CPUs",
                    least / 1000, mean / 1000, most / 1000, ticks1 - ticks0, deadlines1 - deadlines0,
                    arms1 - arms0));
}

/// A wait with nothing to wake it ends at its deadline (`poll`'s timeout), not
/// at the next tick or backstop
fn check_wait_deadline(t: &mut Runner) {
    if crate::arch::x86_64::apic::timer_mode() == crate::arch::x86_64::apic::TimerMode::Periodic {
        t.skip("wait/deadline", String::from("periodic timer: deadlines end on ticks"));
        return;
    }
    use crate::sched::wait::{key, keyed, wait_any};
    let quiet = keyed(key::socket(4000));
    let t0 = crate::kcore::time::now_ns();
    wait_any(&[quiet], t0 + 3_000_000);
    let took = crate::kcore::time::now_ns() - t0;
    t.check("wait/deadline", took >= 3_000_000 && took < 6_000_000,
            format!("a 3 ms wait on a quiet queue took {} us", took / 1000));
}

static WAIT_KEY: AtomicU64 = AtomicU64::new(0);
static WAIT_TOOK: AtomicU64 = AtomicU64::new(0);
/// Set just before the wake the test means; a return before it is counted
static WAIT_FIRED: AtomicBool = AtomicBool::new(false);
static WAIT_EARLY: AtomicU64 = AtomicU64::new(0);

/// Waits on whatever `WAIT_KEY` says until the wake that was meant, counting
/// the ones that were not, and writes down how long the last wait was
fn keyed_waiter() -> ! {
    loop {
        let k = WAIT_KEY.load(Ordering::Acquire);
        if k != 0 {
            let t0 = crate::kcore::time::now_ns();
            crate::sched::wait::wait_any(&[crate::sched::wait::keyed(k)], t0 + 1_000_000_000);
            if WAIT_FIRED.load(Ordering::Acquire) {
                WAIT_TOOK.store((crate::kcore::time::now_ns() - t0).max(1), Ordering::Release);
                WAIT_KEY.store(0, Ordering::Release);
            } else {
                WAIT_EARLY.fetch_add(1, Ordering::AcqRel);
            }
            continue;
        }
        crate::sched::sleep_ns(500_000);
    }
}

/// A waiter on one key is woken by that key and not by another, so a frame of
/// compositor events does not wake every program that polls
fn check_keyed_wake(t: &mut Runner) {
    use crate::sched::wait::{key, keyed, wake_key};
    // Two keys known to be on different queues, named after no socket there is,
    // so nothing real wakes either
    let mine = key::socket(4001);
    let other = (4100..4200).map(key::socket).find(|&k| !core::ptr::eq(keyed(k), keyed(mine)))
        .unwrap_or(key::socket(4002));
    WAIT_TOOK.store(0, Ordering::Release);
    WAIT_FIRED.store(false, Ordering::Release);
    WAIT_EARLY.store(0, Ordering::Release);
    WAIT_KEY.store(mine, Ordering::Release);
    let waiter = crate::sched::spawn_named(keyed_waiter, "ipc-test-wait", 10);

    // Until it is on the queue: after that a wake cannot miss it
    let mut queued = false;
    for _ in 0..200 {
        if !keyed(mine).is_empty() { queued = true; break; }
        crate::sched::sleep_ns(1_000_000);
    }
    // The other key, for long enough that a herd would have been woken
    for _ in 0..10 {
        wake_key(other);
        crate::sched::sleep_ns(1_000_000);
    }
    // Ten wakes of another queue. One or two may still come from something real
    // sharing its queue, which a wake is allowed to be
    let early = WAIT_EARLY.load(Ordering::Acquire);
    let untouched = early < 3;
    let t0 = crate::kcore::time::now_ns();
    WAIT_FIRED.store(true, Ordering::Release);
    wake_key(mine);
    let mut woke_us = None;
    for _ in 0..200 {
        if WAIT_TOOK.load(Ordering::Acquire) != 0 {
            woke_us = Some((crate::kcore::time::now_ns() - t0) / 1000);
            break;
        }
        crate::sched::sleep_ns(200_000);
    }
    crate::sched::kill(waiter);
    t.check("wait/keyed", queued && untouched && woke_us.is_some_and(|us| us < 20_000),
            format!("on its queue {queued}; woken {early} times by ten wakes of another key; \
                     woken {woke_us:?} us after its own"));
}

/// A raw ICMP socket, without needing a network
///
/// The bookkeeping: there are eight ICMP waiters on the machine, so a slot
/// leaking on close costs every later ping, and a program then cannot open a
/// socket. Opening and closing more than the registry holds shows it
fn check_icmp_socket(t: &mut Runner) {
    use crate::net::socket;

    let pid = crate::sched::current_pid();
    let fd = match socket::create_icmp(pid) {
        Some(fd) => fd,
        None => { t.skip("icmp/socket", String::from("no ICMP waiter free")); return; }
    };

    // Nothing has arrived, and a socket with no queue is not readable. It is
    // writable: there is no connection to be backed up
    let r = socket::poll_fd(fd);
    let idle = !r.readable && r.writable && !r.hup && !r.invalid;

    // The option, and its one refusal. A TTL of zero is discarded by the first
    // router, so accepting it would turn a caller's off-by-one into a
    // traceroute that never leaves the machine
    let set_ok = socket::set_ttl(fd, 5).is_ok() && socket::ttl_of(fd) == 5;
    let zero_refused = socket::set_ttl(fd, 0).is_err();

    socket::close_fd(fd);

    // The registry is whole again. Twelve is more than the eight slots there
    // are, so a leak cannot hide behind free ones
    let mut cycled = true;
    for _ in 0..12 {
        match socket::create_icmp(pid) {
            Some(f) => { socket::close_fd(f); }
            None => { cycled = false; break; }
        }
    }

    t.check("icmp/socket", idle && set_ok && zero_refused && cycled,
        format!("idle={} ttl-set={} zero-refused={} twelve-open-close={}",
                idle, set_ok, zero_refused, cycled));
}

/// /dev/net0: whole frames in, whole frames out, and a `poll` that agrees
///
/// Offline, with no card and no cable. Readiness and the read that follows tell
/// the same story: a `poll` reporting readable on an empty queue turns the
/// reader's wait into a spin, and one reporting nothing on a full queue
/// delivers frames only when something else wakes it.
///
/// The frames here are synthetic and drained again, so the queue is left as
/// found even on a machine booted with `net=raw`
fn check_raw_net_device(t: &mut Runner) {
    use crate::fs::vfs::types::{FileMode, OpenFlags};
    use crate::net::raw;

    let fd = match crate::fs::vfs::core::with_vfs(|vfs| {
        vfs.open(0, "/dev/net0", OpenFlags(0x0003), FileMode(0o666))
    }) {
        Ok(fd) => fd,
        Err(e) => { t.skip("netraw/device", format!("cannot open /dev/net0: {:?}", e)); return; }
    };

    let kind_ok = classify(fd as u64) == FdKind::NetRaw;

    // Nothing has arrived. Not readable, and writable: a full transmit ring is
    // a failed write, not a reason to make the caller sleep
    let idle = revents_for(fd as i32, POLLIN | POLLOUT) == POLLOUT;

    // A frame, made up but the right shape: broadcast destination, our
    // ethertype field, and a body long enough that a truncating read shows up
    // as a length mismatch
    let mut frame = [0u8; 74];
    frame[..6].copy_from_slice(&[0xff; 6]);
    frame[6..12].copy_from_slice(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    frame[12] = 0x08;
    frame[13] = 0x06;
    for (i, b) in frame[14..].iter_mut().enumerate() {
        *b = i as u8;
    }
    raw::push(&frame);

    let ready = revents_for(fd as i32, POLLIN | POLLOUT) == (POLLIN | POLLOUT);

    let mut got = [0u8; 2048];
    let n = crate::fs::vfs::core::with_vfs(|vfs| vfs.read(fd, &mut got)).unwrap_or(0);
    let same = n == frame.len() && got[..n] == frame[..];

    // One frame per read, so the second one has nothing to say
    let drained = crate::fs::vfs::core::with_vfs(|vfs| vfs.read(fd, &mut got)).unwrap_or(0) == 0;
    let quiet = revents_for(fd as i32, POLLIN) == 0;

    // A buffer too small to hold the frame drops it rather than handing back
    // half of one, with a plausible header and a severed body
    let (_, _, _, under_before) = raw::stats();
    raw::push(&frame);
    let mut tiny = [0u8; 16];
    let short = crate::fs::vfs::core::with_vfs(|vfs| vfs.read(fd, &mut tiny)).unwrap_or(0);
    let (_, _, _, under_after) = raw::stats();
    let dropped_not_truncated = short == 0 && under_after == under_before + 1;

    crate::fs::vfs::core::with_vfs(|vfs| { let _ = vfs.close(fd); });

    let ok = kind_ok && idle && ready && same && drained && quiet && dropped_not_truncated;
    t.check("netraw/device", ok,
        format!("classify={} idle={} ready-after-push={} round-trip={} ({} of {} bytes) \
drained={} quiet={} undersized-dropped={}",
                kind_ok, idle, ready, same, n, frame.len(),
                drained, quiet, dropped_not_truncated));
}

/// Open a pipe, hand both ends to `f`, and close whatever is left
fn with_pipe<F: FnOnce(usize, usize) -> (bool, String)>(t: &mut Runner, name: &'static str, f: F) {
    let (r, w) = match make_pipe(0) {
        Ok(pair) => pair,
        Err(_) => { t.skip(name, String::from("could not open a pipe")); return; }
    };
    let (ok, detail) = f(r, w);
    crate::fs::vfs::core::with_vfs(|vfs| {
        let _ = vfs.close(r);
        let _ = vfs.close(w);
    });
    t.check(name, ok, detail);
}

fn check_classify(t: &mut Runner) {
    with_pipe(t, "poll/classify", |r, w| {
        let rk = matches!(classify(r as u64), FdKind::Pipe(_));
        let wk = matches!(classify(w as u64), FdKind::Pipe(_));
        // A descriptor nothing was ever opened on. Well above anything the
        // VFS hands out and well below the socket range
        let closed = classify(120) == FdKind::Closed;
        (rk && wk && closed,
         format!("read end pipe={}, write end pipe={}, unopened fd closed={}", rk, wk, closed))
    });
}

fn check_pipe_readiness(t: &mut Runner) {
    with_pipe(t, "poll/pipe-readiness", |r, w| {
        let vid = match crate::fs::vfs::core::pipe_vnode_of_fd(r) {
            Some(v) => v,
            None => return (false, String::from("read end is not a pipe")),
        };

        let empty = crate::fs::vfs::core::pipe_poll(vid);
        let wrote = crate::fs::vfs::core::with_vfs(|vfs| vfs.write(w, b"hello").is_ok());
        let full  = crate::fs::vfs::core::pipe_poll(vid);
        // Closing the only writer is what turns a quiet pipe into end of file
        crate::fs::vfs::core::with_vfs(|vfs| { let _ = vfs.close(w); });
        let gone  = crate::fs::vfs::core::pipe_poll(vid);

        let ok = empty == (false, false)
            && wrote
            && full  == (true, false)
            && gone  == (true, true);
        (ok, format!("empty={:?} after-write={:?} writer-gone={:?}", empty, full, gone))
    });
}

/// The ring (`fs::vfs::pipe`): it holds exactly its capacity and every byte
/// comes back in order, and a small write goes in whole or not at all
fn check_pipe_ring(t: &mut Runner) {
    use crate::fs::vfs::types::VfsError;
    with_pipe(t, "pipe/ring", |r, w| {
        let cap = crate::fs::vfs::pipe::CAPACITY;
        // A length that does not divide the ring, so the wrap is exercised
        let chunk: alloc::vec::Vec<u8> = (0..3000u32).map(|i| (i * 31 + 7) as u8).collect();
        let mut put = 0usize;
        let full = loop {
            match crate::fs::vfs::core::with_vfs(|vfs| vfs.write(w, &chunk)) {
                Ok(n) => put += n,
                Err(VfsError::WouldBlock) => break true,
                Err(_) => break false,
            }
            if put > 2 * cap {
                break false;
            }
        };
        // Whole 3000-byte writes only: the ring stops short of full by less
        // than one of them, and refuses the one that would not fit
        let whole = full && put % chunk.len() == 0 && cap - put < chunk.len();

        // Half out, more in across the wrap, then all of it back in order
        let mut got = alloc::vec::Vec::new();
        let mut buf = alloc::vec![0u8; 5000];
        while got.len() < put / 2 {
            match crate::fs::vfs::core::with_vfs(|vfs| vfs.read(r, &mut buf)) {
                Ok(n) if n > 0 => got.extend_from_slice(&buf[..n]),
                _ => break,
            }
        }
        let more = crate::fs::vfs::core::with_vfs(|vfs| vfs.write(w, &chunk)).unwrap_or(0);
        loop {
            match crate::fs::vfs::core::with_vfs(|vfs| vfs.read(r, &mut buf)) {
                Ok(n) if n > 0 => got.extend_from_slice(&buf[..n]),
                _ => break,
            }
        }
        let expect: alloc::vec::Vec<u8> = chunk.iter().cycle().take(put + more).copied().collect();
        let intact = got == expect;
        (whole && more == chunk.len() && intact,
         format!("{} bytes in before full (cap {}), {} more after half was read, {} back in order: {}",
                 put, cap, more, got.len(), intact))
    });
}

/// The ends are counted by their descriptors: a duplicate of the write end
/// keeps the pipe open after the original closes, its close is the end of
/// file, and a write with no reader left is refused
fn check_pipe_ends(t: &mut Runner) {
    use crate::fs::vfs::core::with_vfs;
    use crate::fs::vfs::types::VfsError;
    with_pipe(t, "pipe/ends-counted", |r, w| {
        let vid = crate::fs::vfs::core::pipe_vnode_of_fd(r).unwrap_or(0);
        let w2 = match with_vfs(|vfs| vfs.dup(w)) {
            Ok(fd) => fd,
            Err(e) => return (false, format!("dup: {:?}", e)),
        };
        with_vfs(|vfs| { let _ = vfs.close(w); });
        let open_after_one = !crate::fs::vfs::core::pipe_poll(vid).1;
        with_vfs(|vfs| { let _ = vfs.close(w2); });
        let mut b = [0u8; 8];
        let eof = with_vfs(|vfs| vfs.read(r, &mut b)) == Ok(0) && crate::fs::vfs::core::pipe_poll(vid).1;
        (open_after_one && eof,
         format!("open with a duplicate writer left: {}, end of file after its close: {}", open_after_one, eof))
    });
    with_pipe(t, "pipe/no-reader", |r, w| {
        with_vfs(|vfs| { let _ = vfs.close(r); });
        let refused = with_vfs(|vfs| vfs.write(w, b"anyone?"));
        (refused == Err(VfsError::BrokenPipe), format!("a write with the reader gone: {:?}", refused))
    });
}

fn check_pipe_revents(t: &mut Runner) {
    with_pipe(t, "poll/pipe-revents", |r, w| {
        let quiet = revents_for(r as i32, POLLIN);
        crate::fs::vfs::core::with_vfs(|vfs| { let _ = vfs.write(w, b"x"); });
        let data = revents_for(r as i32, POLLIN);
        crate::fs::vfs::core::with_vfs(|vfs| { let _ = vfs.close(w); });
        let hup = revents_for(r as i32, POLLIN);

        // A poller that asked only for POLLIN is still told about the hangup:
        // otherwise it waits for data from a writer that has gone
        let ok = quiet == 0 && data == POLLIN && hup == (POLLIN | POLLHUP);
        (ok, format!("quiet={:#05x} with-data={:#05x} writer-gone={:#05x}", quiet, data, hup))
    });
}

fn check_invalid_and_ignored(t: &mut Runner) {
    let nval = revents_for(120, POLLIN | POLLOUT);
    // A negative entry is how a caller parks a slot in a fixed-size set
    let ignored = revents_for(-1, POLLIN);
    t.check("poll/invalid-and-ignored", nval == POLLNVAL && ignored == 0,
        format!("unopened fd={:#05x} (want {:#05x}), negative fd={:#05x}",
            nval, POLLNVAL, ignored));
}

fn check_socket_range(t: &mut Runner) {
    // A socket descriptor must classify by range alone, without the VFS being
    // consulted, which keeps the two namespaces apart
    let base = crate::net::socket::SOCK_FD_BASE as u64;
    let in_range  = classify(base) == FdKind::Socket;
    let below     = classify(base - 1) != FdKind::Socket;
    // Nothing is open there, so readiness must say so rather than guess
    let nval = revents_for(base as i32, POLLIN) == POLLNVAL;
    t.check("poll/socket-range", in_range && below && nval,
        format!("fd {} is a socket={}, fd {} is not={}, unopened socket fd is NVAL={}",
            base, in_range, base - 1, below, nval));
}

// Local sockets
//
// The pair cases run here because they need only this thread. The
// connect/accept handshake needs two processes, since one side blocks until the
// other arrives, and is tested from ring 3

use crate::net::socket;

/// Somewhere in the root that no real service would claim
const TEST_PATH: &str = "/ipctest.sock";

fn check_socketpair(t: &mut Runner) {
    let pid = crate::sched::current_pid();
    let (a, b) = match socket::socketpair(pid) {
        Some(p) => p,
        None => { t.skip("ipc/socketpair", String::from("socket table full")); return; }
    };

    let mut buf = [0u8; 32];
    let sent    = socket::send(a, b"ping").unwrap_or(0);
    let got     = socket::recv(b, &mut buf).unwrap_or(0);
    let forward = sent == 4 && got == 4 && &buf[..4] == b"ping";

    // Both directions: a bug writing into the sender's own queue would pass a
    // one-way test
    let sent_b  = socket::send(b, b"pong!").unwrap_or(0);
    let got_b   = socket::recv(a, &mut buf).unwrap_or(0);
    let back    = sent_b == 5 && got_b == 5 && &buf[..5] == b"pong!";

    // Closing one end must read as end of file at the other, not as a wait
    socket::close_fd(a);
    let eof = socket::recv(b, &mut buf) == Ok(0);
    // And writing into a connection whose far end has gone is a broken pipe
    let epipe = socket::send(b, b"x").is_err();
    socket::close_fd(b);

    t.check("ipc/socketpair", forward && back && eof && epipe,
        format!("a->b={} b->a={} eof-after-close={} write-after-close-fails={}",
            forward, back, eof, epipe));
}

fn check_unix_pair_readiness(t: &mut Runner) {
    let pid = crate::sched::current_pid();
    let (a, b) = match socket::socketpair(pid) {
        Some(p) => p,
        None => { t.skip("ipc/unix-readiness", String::from("socket table full")); return; }
    };

    let quiet = socket::poll_fd(b);
    socket::send(a, b"hi").ok();
    let data = socket::poll_fd(b);
    socket::close_fd(a);
    let hup = socket::poll_fd(b);
    socket::close_fd(b);

    let ok = !quiet.readable && quiet.writable && !quiet.hup
        && data.readable
        // Bytes already delivered stay readable after the peer goes: the reader
        // drains them before it believes the end
        && hup.readable && hup.hup && !hup.writable;
    t.check("ipc/unix-readiness", ok,
        format!("quiet r={} w={}, with-data r={}, peer-gone r={} w={} hup={}",
            quiet.readable, quiet.writable, data.readable,
            hup.readable, hup.writable, hup.hup));
}

fn check_unix_unconnected(t: &mut Runner) {
    let pid = crate::sched::current_pid();
    let fd = match socket::create_unix(pid) {
        Some(f) => f,
        None => { t.skip("ipc/unix-unconnected", String::from("socket table full")); return; }
    };
    // A socket never connected has no peer, which differs from a peer that has
    // gone. A hangup here would tell a poller to give up on a connect not yet
    // attempted
    let r = socket::poll_fd(fd);
    socket::close_fd(fd);
    t.check("ipc/unix-unconnected", !r.readable && !r.writable && !r.hup && !r.invalid,
        format!("readable={} writable={} hup={} invalid={}",
            r.readable, r.writable, r.hup, r.invalid));
}

fn check_unix_names(t: &mut Runner) {
    let pid = crate::sched::current_pid();
    // Leave nothing behind from a previous run, or the bind below fails for
    // the wrong reason
    crate::fs::vfs::core::unlink_path("/", &TEST_PATH[1..]);

    let fd = match socket::create_unix(pid) {
        Some(f) => f,
        None => { t.skip("ipc/unix-names", String::from("socket table full")); return; }
    };
    let bound   = socket::unix_bind(fd, TEST_PATH).is_ok();
    let visible = crate::fs::vfs::core::socket_node_at(TEST_PATH).is_some();
    let listens = socket::unix_listen(fd).is_ok();

    // A second socket must not take an address in use, or a program could
    // displace a running service
    let taken = match socket::create_unix(pid) {
        Some(f2) => {
            let refused = socket::unix_bind(f2, TEST_PATH).is_err();
            socket::close_fd(f2);
            refused
        }
        None => true,
    };

    // Connecting to a name nothing is listening on is a refusal, not a wait
    let refused = match socket::create_unix(pid) {
        Some(f3) => {
            let r = socket::unix_connect(f3, "/no-such-socket") == Err(socket::SockError::ConnRefused);
            socket::close_fd(f3);
            r
        }
        None => true,
    };

    // Closing the listener takes the name with it: a name outliving its socket
    // makes the address unusable
    socket::close_fd(fd);
    let cleaned = crate::fs::vfs::core::socket_node_at(TEST_PATH).is_none();

    t.check("ipc/unix-names", bound && visible && listens && taken && refused && cleaned,
        format!("bound={} visible={} listening={} address-in-use-refused={} \
                 connect-to-nothing-refused={} name-removed-on-close={}",
            bound, visible, listens, taken, refused, cleaned));
}
