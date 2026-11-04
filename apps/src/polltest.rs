#![no_std]
#![no_main]

//! Poll test from ring 3: after poll reports a descriptor ready, the following
//! read must not block. The connect/accept case needs two processes, so it
//! forks

pub use miku;
use miku::{MikuPollFd, POLLHUP, POLLIN, POLLNVAL, POLLOUT};

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
    miku::print(if ok { "  ok    " } else { "  FAIL  " });
    miku::println(name);
}

fn pollfd(fd: i64, events: i16) -> MikuPollFd {
    MikuPollFd {
        fd: fd as i32,
        events,
        revents: 0,
    }
}

/// The socket address these two halves meet at
const SOCK_PATH: &[u8] = b"/polltest.sock\0";

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    miku::println("polltest: poll and local sockets from ring 3");

    test_timeout();
    test_pipe();
    test_socketpair();
    test_invalid();
    test_named_socket();

    miku::print("polltest: ");
    unsafe {
        miku::print_int(PASSED as i64);
        miku::print(" passed, ");
        miku::print_int(FAILED as i64);
        miku::println(" failed");
        miku::exit(if FAILED == 0 { 0 } else { 1 });
    }
}

/// A timeout must not return early, and zero must not sleep
fn test_timeout() {
    let mut set = [pollfd(-1, POLLIN)];
    let none = unsafe { miku_poll_wrap(&mut set, 0) };
    check("poll with no live descriptors returns 0", none == 0);

    let t0 = miku::uptime_ms();
    let mut set = [pollfd(-1, POLLIN)];
    let r = unsafe { miku_poll_wrap(&mut set, 60) };
    let waited = miku::uptime_ms().wrapping_sub(t0);
    // Loose upper bound
    check(
        "poll honours a timeout",
        r == 0 && waited >= 40 && waited < 2000,
    );
}

fn test_pipe() {
    let mut fds = [0i64; 2];
    if unsafe { miku::miku_pipe(fds.as_mut_ptr()) } < 0 {
        check("pipe could not be created", false);
        return;
    }
    let (r, w) = (fds[0], fds[1]);

    let mut set = [pollfd(r, POLLIN)];
    let quiet = unsafe { miku_poll_wrap(&mut set, 0) };
    check(
        "empty pipe is not readable",
        quiet == 0 && set[0].revents == 0,
    );

    unsafe { miku::miku_write(w as u64, b"hello".as_ptr(), 5) };
    let mut set = [pollfd(r, POLLIN)];
    let ready = unsafe { miku_poll_wrap(&mut set, 1000) };
    check(
        "written pipe polls readable",
        ready == 1 && set[0].revents & POLLIN != 0,
    );

    // A read after a ready poll returns
    let mut buf = [0u8; 16];
    let n = unsafe { miku::miku_read(r as u64, buf.as_mut_ptr(), buf.len()) };
    check(
        "read after a ready poll returns at once",
        n == 5 && &buf[..5] == b"hello",
    );

    unsafe { miku::miku_close(w as i64) };
    let mut set = [pollfd(r, POLLIN)];
    let hup = unsafe { miku_poll_wrap(&mut set, 1000) };
    check(
        "closed writer shows as hangup",
        hup == 1 && set[0].revents & POLLHUP != 0,
    );

    unsafe { miku::miku_close(r as i64) };
}

fn test_socketpair() {
    let mut fds = [0u64; 2];
    if unsafe { miku::miku_socketpair(fds.as_mut_ptr()) } < 0 {
        check("socketpair could not be created", false);
        return;
    }
    let (a, b) = (fds[0] as i64, fds[1] as i64);

    let mut set = [pollfd(b, POLLIN | POLLOUT)];
    let idle = unsafe { miku_poll_wrap(&mut set, 0) };
    check(
        "idle pair is writable and not readable",
        idle == 1 && set[0].revents == POLLOUT,
    );

    unsafe { miku::miku_send(a, b"ping".as_ptr(), 4) };
    let mut set = [pollfd(b, POLLIN)];
    let ready = unsafe { miku_poll_wrap(&mut set, 1000) };
    let mut buf = [0u8; 16];
    let n = unsafe { miku::miku_recv(b, buf.as_mut_ptr(), buf.len()) };
    check(
        "pair delivers and polls readable",
        ready == 1 && set[0].revents & POLLIN != 0 && n == 4 && &buf[..4] == b"ping",
    );

    // Both directions
    unsafe { miku::miku_send(b, b"pong".as_ptr(), 4) };
    let n = unsafe { miku::miku_recv(a, buf.as_mut_ptr(), buf.len()) };
    check(
        "pair carries the other direction too",
        n == 4 && &buf[..4] == b"pong",
    );

    unsafe { miku::miku_close(a as i64) };
    let mut set = [pollfd(b, POLLIN)];
    let hup = unsafe { miku_poll_wrap(&mut set, 1000) };
    let eof = unsafe { miku::miku_recv(b, buf.as_mut_ptr(), buf.len()) };
    check(
        "closed peer is a hangup and an end of file",
        hup == 1 && set[0].revents & POLLHUP != 0 && eof == 0,
    );

    unsafe { miku::miku_close(b as i64) };
}

fn test_invalid() {
    // A closed descriptor reports invalid without failing the whole call
    let mut fds = [0u64; 2];
    if unsafe { miku::miku_socketpair(fds.as_mut_ptr()) } < 0 {
        return;
    }
    let a = fds[0] as i64;

    let mut set = [pollfd(120, POLLIN), pollfd(a, POLLOUT)];
    let r = unsafe { miku_poll_wrap(&mut set, 0) };
    check(
        "an unopened descriptor is invalid, not fatal",
        r == 2 && set[0].revents == POLLNVAL && set[1].revents == POLLOUT,
    );

    unsafe {
        miku::miku_close(fds[0] as i64);
        miku::miku_close(fds[1] as i64);
    }
}

/// bind / listen / accept against a connect from a forked child
fn test_named_socket() {
    let srv = unsafe { miku::miku_socket_unix() };
    if srv < 0 {
        check("local socket could not be created", false);
        return;
    }
    if unsafe { miku::miku_bind_unix(srv, SOCK_PATH.as_ptr()) } != 0 {
        check("bind to a name", false);
        return;
    }
    if unsafe { miku::miku_listen(srv, 4) } != 0 {
        check("listen on a named socket", false);
        return;
    }

    let pid = unsafe { miku::miku_fork() };
    if pid == 0 {
        // Child: client side
        let c = unsafe { miku::miku_socket_unix() };
        if c < 0 {
            miku::exit(2);
        }
        if unsafe { miku::miku_connect_unix(c, SOCK_PATH.as_ptr()) } != 0 {
            miku::exit(3);
        }
        unsafe { miku::miku_send(c, b"from-the-child".as_ptr(), 14) };
        // Left open until exit, so the parent can still read what was written
        miku::exit(0);
    }

    // Parent: a listener is readable when accept would not block
    let mut set = [pollfd(srv, POLLIN)];
    let ready = unsafe { miku_poll_wrap(&mut set, 5000) };
    check(
        "a pending connection polls readable on the listener",
        ready == 1 && set[0].revents & POLLIN != 0,
    );

    let conn = unsafe { miku::miku_accept(srv, core::ptr::null_mut(), core::ptr::null_mut()) };
    check("accept returns a connection", conn >= 0);
    if conn >= 0 {
        let mut set = [pollfd(conn, POLLIN)];
        let r = unsafe { miku_poll_wrap(&mut set, 5000) };
        let mut buf = [0u8; 32];
        let n = unsafe { miku::miku_recv(conn, buf.as_mut_ptr(), buf.len()) };
        check(
            "the child's bytes arrive over the name",
            r == 1 && n == 14 && &buf[..14] == b"from-the-child",
        );
        unsafe { miku::miku_close(conn as i64) };
    }

    let mut status: i64 = -1;
    unsafe { miku::miku_waitpid(pid as u64, &mut status) };
    check("the client exited cleanly", status == 0);

    unsafe { miku::miku_close(srv as i64) };
}

/// `miku_poll` over a slice
unsafe fn miku_poll_wrap(set: &mut [MikuPollFd], timeout_ms: i64) -> i64 {
    miku::miku_poll(set.as_mut_ptr(), set.len() as u64, timeout_ms)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::println("polltest: panic");
    miku::exit(1);
}
