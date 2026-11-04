//! tokio on MikuOS. `mio`, `socket2`, and tokio come from crates.io with MikuOS
//! added to their target lists (`builder/src/stdport/arms.rs`).
//!
//! 1. multi-threaded runtime with a hundred tasks
//! 2. a timer fires, not before it is due
//! 3. a channel between workers
//! 4. `spawn_blocking`
//! 5. `tokio::fs` write and read
//! 6. a Unix socket pair, both ways
//! 7. peer credentials (`SO_PEERCRED`, `peercred`(83))
//!    8. a Unix listener accepts a `UnixStream::connect`
//! 9. a child's output
//! 10. a TCP connect and HTTP response
//! 11. a refused TCP connect
//!
//! 10 and 11 need `python3 -m http.server 8000` on the host (`10.0.2.2` in
//! QEMU). Port 1 is assumed closed.

use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
async fn within<T>(secs: u64, f: impl std::future::Future<Output = std::io::Result<T>>) -> Result<T, String> {
    match tokio::time::timeout(Duration::from_secs(secs), f).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(format!("error: {e}")),
        Err(_) => Err("timed out".into()),
    }
}

fn show<T: ToString>(r: Result<T, String>) -> String {
    match r {
        Ok(v) => v.to_string(),
        Err(e) => e,
    }
}

fn main() {
    println!("stdtokio: tokio - runtime, time, sync, fs, net, process");
    let mut failed = 0;

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            check(&format!("the runtime builds: {e}"), false, &mut failed);
            std::process::exit(1);
        }
    };

    rt.block_on(async {
        // 1. Many tasks, many workers
        {
            let handles: Vec<_> = (1..=100u64).map(|i| tokio::spawn(async move { i * 2 })).collect();
            let mut sum = 0;
            for h in handles {
                sum += h.await.unwrap_or(0);
            }
            check_eq("a hundred tasks across four workers", &sum.to_string(), "10100", &mut failed);
        }

        // 2. A timer
        {
            let t0 = Instant::now();
            tokio::time::sleep(Duration::from_millis(60)).await;
            let waited = t0.elapsed();
            check(
                "a 60 ms sleep fires, and not early",
                waited >= Duration::from_millis(50) && waited < Duration::from_secs(2),
                &mut failed,
            );
        }

        // 3. A channel between spawned tasks
        {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<u32>(1);
            let producer = tokio::spawn(async move {
                for i in 1..=10 {
                    if tx.send(i).await.is_err() {
                        break;
                    }
                }
            });
            let consumer = tokio::spawn(async move {
                let mut sum = 0;
                while let Some(v) = rx.recv().await {
                    sum += v;
                }
                sum
            });
            let _ = producer.await;
            let sum = tokio::time::timeout(Duration::from_secs(5), consumer).await;
            check_eq(
                "a channel of one carries 1..=10 between tasks",
                &format!("{:?}", sum.ok().and_then(|r| r.ok())),
                "Some(55)",
                &mut failed,
            );
        }

        // 4. spawn_blocking
        {
            let r = tokio::task::spawn_blocking(|| {
                std::thread::sleep(Duration::from_millis(20));
                std::thread::current().id()
            })
            .await;
            let here = std::thread::current().id();
            check(
                "spawn_blocking runs on another thread and comes back",
                r.map(|id| id != here).unwrap_or(false),
                &mut failed,
            );
        }

        // 5. tokio::fs
        {
            let path = "/tmp/stdtokio.txt";
            let r = async {
                tokio::fs::write(path, b"miku").await?;
                let s = tokio::fs::read_to_string(path).await?;
                tokio::fs::remove_file(path).await?;
                Ok(s)
            };
            check_eq("tokio::fs writes, reads and removes", &show(within(5, r).await), "miku", &mut failed);
        }

        // 6 and 7. A Unix-socket pair, and who is on the other end
        {
            let r = async {
                let (mut a, mut b) = tokio::net::UnixStream::pair()?;
                a.write_all(b"ping").await?;
                let mut buf = [0u8; 4];
                b.read_exact(&mut buf).await?;
                b.write_all(b"pong").await?;
                let mut back = [0u8; 4];
                a.read_exact(&mut back).await?;
                let cred = b.peer_cred()?;
                Ok((
                    format!("{}/{}", String::from_utf8_lossy(&buf), String::from_utf8_lossy(&back)),
                    cred,
                ))
            };
            match within(5, r).await {
                Ok((s, cred)) => {
                    check_eq("a UnixStream pair carries bytes both ways", &s, "ping/pong", &mut failed);
                    let pid = std::process::id() as i32;
                    check(
                        "peer_cred names this process",
                        cred.pid() == Some(pid),
                        &mut failed,
                    );
                    println!("        (uid {} gid {} pid {:?})", cred.uid(), cred.gid(), cred.pid());
                }
                Err(e) => {
                    check_eq("a UnixStream pair carries bytes both ways", &e, "ping/pong", &mut failed);
                    check("peer_cred names this process", false, &mut failed);
                }
            }
        }

        // 8. A Unix listener
        {
            let path = "/tmp/stdtokio.sock";
            let _ = std::fs::remove_file(path);
            let r = async {
                let listener = tokio::net::UnixListener::bind(path)?;
                let server = tokio::spawn(async move {
                    let (mut s, _) = listener.accept().await?;
                    s.write_all(b"hello").await?;
                    std::io::Result::Ok(())
                });
                let mut c = tokio::net::UnixStream::connect(path).await?;
                let mut buf = [0u8; 5];
                let mut got = 0;
                while got < buf.len() {
                    match c.read(&mut buf[got..]).await {
                        Ok(0) => break,
                        Ok(n) => got += n,
                        Err(e) => return Err(e),
                    }
                }
                let srv = server.await;
                if got < buf.len() {
                    return Err(std::io::Error::other(format!("eof after {got} bytes; server said {srv:?}")));
                }
                Ok(String::from_utf8_lossy(&buf).to_string())
            };
            let got = show(within(10, r).await);
            let _ = std::fs::remove_file(path);
            check_eq("a UnixListener accepts a UnixStream::connect", &got, "hello", &mut failed);
        }

        // 9. A child process
        {
            let r = async {
                let o = tokio::process::Command::new("/bin/echo").arg("tokio").output().await?;
                Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
            };
            check_eq("tokio::process runs a child and reads its output", &show(within(10, r).await), "tokio", &mut failed);
        }

        // 10. TCP to the host
        {
            let r = async {
                let mut c = tokio::net::TcpStream::connect("10.0.2.2:8000").await?;
                c.write_all(b"HEAD / HTTP/1.0\r\n\r\n").await?;
                let mut buf = [0u8; 8];
                c.read_exact(&mut buf).await?;
                Ok(String::from_utf8_lossy(&buf[..7]).to_string())
            };
            check_eq("TcpStream::connect, and an HTTP answer", &show(within(15, r).await), "HTTP/1.", &mut failed);
        }

        // 11. And a refusal
        {
            let r = tokio::time::timeout(
                Duration::from_secs(15),
                tokio::net::TcpStream::connect("10.0.2.2:1"),
            )
            .await;
            let got = match r {
                Ok(Ok(_)) => "connected".to_string(),
                Ok(Err(e)) => format!("{:?}", e.kind()),
                Err(_) => "timed out".into(),
            };
            check_eq("a refused connect is ConnectionRefused", &got, "ConnectionRefused", &mut failed);
        }
    });

    println!("stdtokio: {}", if failed == 0 { "all passed" } else { "FAILURES" });
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
