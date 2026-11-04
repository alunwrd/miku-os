//! `smol` on MikuOS: `async-io` timers and `Async<T>`, `async-executor`,
//! `async-channel`, `async-process`
//!
//! 1. a timer fires, not before it is due
//! 2. a pipe read waits for a writer on another thread without blocking the
//!    executor
//!   3. a local socket pair carries bytes both ways
//! 4. a listener accepts a connection from `Async::connect`
//! 5. tasks on one executor pass values over a channel
//! 6. a child process's output comes back through the reactor
//! 7. `Async::<TcpStream>::connect` (`EINPROGRESS`, writable when established)
//!    and an HTTP response
//! 8. a refused connect returns `ConnectionRefused` through `SO_ERROR`
//! 9. `TcpStream::connect_timeout`
//!
//! 7-9 need `python3 -m http.server 8000` on the host (`10.0.2.2` in QEMU).
//! Port 1 is assumed closed.

use std::io::Write;
use std::time::{Duration, Instant};

use smol::io::{AsyncReadExt, AsyncWriteExt};
use smol::{Async, Timer};

fn check(name: &str, ok: bool, failed: &mut i32) {
    println!("  {}  {name}", if ok { "ok  " } else { "FAIL" });
    if !ok {
        *failed += 1;
    }
}

fn check_eq(name: &str, got: &str, want: &str, failed: &mut i32) {
    if got == want {
        println!("  ok    {name}");
    } else {
        println!("  FAIL  {name}: got {got:?}, wanted {want:?}");
        *failed += 1;
    }
}

/// Timeout for a case that would otherwise hang
async fn within<T>(secs: u64, f: impl std::future::Future<Output = T>) -> Option<T> {
    smol::future::or(async { Some(f.await) }, async {
        Timer::after(Duration::from_secs(secs)).await;
        None
    })
    .await
}

fn main() {
    println!("stdasync: smol - executor, timers, Async<T>, channels, processes");
    let mut failed = 0;

    smol::block_on(async {
        // 1. A timer
        {
            let t0 = Instant::now();
            Timer::after(Duration::from_millis(60)).await;
            let waited = t0.elapsed();
            check(
                "a 60 ms timer fires, and not early",
                waited >= Duration::from_millis(50) && waited < Duration::from_secs(2),
                &mut failed,
            );
        }

        // 2. A pipe, written from another thread after a pause
        {
            let r = async {
                let (rx, mut tx) = std::io::pipe()?;
                let mut rx = Async::new(rx)?;
                let writer = std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(50));
                    tx.write_all(b"miku").unwrap();
                });
                let mut buf = [0u8; 4];
                rx.read_exact(&mut buf).await?;
                let _ = writer.join();
                std::io::Result::Ok(buf)
            };
            let got = match within(5, r).await {
                Some(Ok(b)) => String::from_utf8_lossy(&b).to_string(),
                Some(Err(e)) => format!("error: {e}"),
                None => "timed out".into(),
            };
            check_eq("an Async pipe waits for a writer on another thread", &got, "miku", &mut failed);
        }

        // 3. A local socket pair
        {
            let r = async {
                let (a, b) = Async::<std::os::unix::net::UnixStream>::pair()?;
                let (mut a, mut b) = (a, b);
                a.write_all(b"ping").await?;
                let mut buf = [0u8; 4];
                b.read_exact(&mut buf).await?;
                b.write_all(b"pong").await?;
                let mut back = [0u8; 4];
                a.read_exact(&mut back).await?;
                std::io::Result::Ok(format!(
                    "{}/{}",
                    String::from_utf8_lossy(&buf),
                    String::from_utf8_lossy(&back)
                ))
            };
            let got = match within(5, r).await {
                Some(Ok(s)) => s,
                Some(Err(e)) => format!("error: {e}"),
                None => "timed out".into(),
            };
            check_eq("a UnixStream pair carries bytes both ways", &got, "ping/pong", &mut failed);
        }

        // 4. Accepts on its own thread, so a blocking connect cannot deadlock
        //    the executor
        {
            let path = "/tmp/stdasync.sock";
            let _ = std::fs::remove_file(path);
            let r = async {
                let listener = std::os::unix::net::UnixListener::bind(path)?;
                let server = std::thread::spawn(move || -> std::io::Result<()> {
                    let (mut s, _) = listener.accept()?;
                    s.write_all(b"hello")?;
                    Ok(())
                });
                let mut c = Async::<std::os::unix::net::UnixStream>::connect(path).await?;
                let mut buf = [0u8; 5];
                c.read_exact(&mut buf).await?;
                let _ = server.join();
                std::io::Result::Ok(String::from_utf8_lossy(&buf).to_string())
            };
            let got = match within(10, r).await {
                Some(Ok(s)) => s,
                Some(Err(e)) => format!("error: {e}"),
                None => "timed out".into(),
            };
            let _ = std::fs::remove_file(path);
            check_eq("Async::<UnixStream>::connect reaches a listener", &got, "hello", &mut failed);
        }

        // 5. Tasks and a channel
        {
            let ex = smol::LocalExecutor::new();
            let (tx, rx) = smol::channel::bounded::<u32>(1);
            let producer = ex.spawn(async move {
                for i in 1..=10 {
                    tx.send(i).await.unwrap();
                }
            });
            let consumer = ex.spawn(async move {
                let mut sum = 0;
                while let Ok(v) = rx.recv().await {
                    sum += v;
                }
                sum
            });
            let sum = within(5, ex.run(async {
                producer.await;
                consumer.await
            }))
            .await;
            check_eq(
                "two tasks hand 1..=10 across a channel of one",
                &format!("{sum:?}"),
                "Some(55)",
                &mut failed,
            );
        }

        // 6. A child process
        {
            let r = smol::process::Command::new("/bin/echo").arg("smol").output();
            let got = match within(10, r).await {
                Some(Ok(o)) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
                Some(Err(e)) => format!("error: {e}"),
                None => "timed out".into(),
            };
            check_eq("async-process runs a child and reads its output", &got, "smol", &mut failed);
        }

        // 7. A non-blocking TCP connect, through the reactor
        {
            let r = async {
                let mut c = Async::<std::net::TcpStream>::connect(([10, 0, 2, 2], 8000)).await?;
                c.write_all(b"HEAD / HTTP/1.0\r\n\r\n").await?;
                let mut buf = [0u8; 8];
                c.read_exact(&mut buf).await?;
                std::io::Result::Ok(String::from_utf8_lossy(&buf[..7]).to_string())
            };
            let got = match within(15, r).await {
                Some(Ok(s)) => s,
                Some(Err(e)) => format!("error: {e}"),
                None => "timed out".into(),
            };
            check_eq("Async::<TcpStream>::connect, and an HTTP answer", &got, "HTTP/1.", &mut failed);
        }

        // 8. Refused: writable with an error, reported by SO_ERROR
        {
            let r = Async::<std::net::TcpStream>::connect(([10, 0, 2, 2], 1));
            let got = match within(15, r).await {
                Some(Ok(_)) => "connected".to_string(),
                Some(Err(e)) => format!("{:?}", e.kind()),
                None => "timed out".into(),
            };
            check_eq("a refused connect is ConnectionRefused", &got, "ConnectionRefused", &mut failed);
        }
    });

    // 9. The same machinery under std's blocking API
    {
        let addr = std::net::SocketAddr::from(([10, 0, 2, 2], 8000));
        let r = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(5));
        check(
            "TcpStream::connect_timeout reaches the host",
            r.as_ref().map(|s| s.take_error().ok().flatten().is_none()).unwrap_or(false),
            &mut failed,
        );
        if let Err(e) = r {
            println!("        ({e})");
        }
    }

    println!("stdasync: {}", if failed == 0 { "all passed" } else { "FAILURES" });
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
