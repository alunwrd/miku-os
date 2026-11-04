//! Zed's `remote_server`, end to end
//!
//! The client runs `remote_server proxy` and talks over stdin and stdout. The
//! proxy starts `remote_server run` with three Unix sockets and joins its stdio
//! to them.
//!
//! Request: `Envelope { id: 1, ping: {} }`, framed as a little-endian u32
//! length and protobuf (`crates/remote/src/protocol.rs`), answered by `Ack`,
//! `responding_to: 1`.
//!
//! `stdremote [path]`, `/bin/zedrs` by default. Skips if absent.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn check(name: &str, ok: bool, failed: &mut i32) {
    println!("  {}  {name}", if ok { "ok  " } else { "FAIL" });
    if !ok {
        *failed += 1;
    }
}

/// `Envelope { id: 1, ping: Ping {} }`: field 1 varint, field 7 empty message
const PING: [u8; 4] = [0x08, 0x01, 0x3A, 0x00];

/// Envelope fields used here: `responding_to` (2) and the `oneof` payload's
/// field number
fn decode(buf: &[u8]) -> Option<(Option<u64>, Option<u64>)> {
    let mut at = 0;
    let varint = |at: &mut usize| -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = *buf.get(*at)?;
            *at += 1;
            v |= ((b & 0x7F) as u64) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    };
    let (mut responding_to, mut payload) = (None, None);
    while at < buf.len() {
        let key = varint(&mut at)?;
        let (field, wire) = (key >> 3, key & 7);
        match wire {
            0 => {
                let v = varint(&mut at)?;
                if field == 2 {
                    responding_to = Some(v);
                }
            }
            2 => {
                let len = varint(&mut at)? as usize;
                at = at.checked_add(len)?;
                if field >= 4 && field != 266 {
                    payload = Some(field);
                }
            }
            5 => at += 4,
            1 => at += 8,
            _ => return None,
        }
    }
    Some((responding_to, payload))
}

/// `stdremote --run`: the server alone, with stderr on the console
fn run_server_alone(exe: &str) {
    println!("stdremote: `{exe} run` on its own");
    let at = |s: &str| format!("/tmp/zr.{s}");
    let child = Command::new(exe)
        .env("SHELL", "/bin/msh")
        .args(["run", "--log-file", &at("log"), "--pid-file", &at("pid")])
        .args(["--stdin-socket", &at("in"), "--stdout-socket", &at("out")])
        .args(["--stderr-socket", &at("err")])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();
    match child {
        Err(e) => println!("  spawn failed: {e}"),
        Ok(mut c) => {
            std::thread::sleep(Duration::from_secs(10));
            for s in ["in", "out", "err", "pid"] {
                println!("  {}: {}", at(s), Path::new(&at(s)).exists());
            }
            println!("  still running: {:?}", c.try_wait());
            let _ = c.kill();
        }
    }
    match std::fs::read_to_string(at("log")) {
        Ok(log) => println!("--- log ---\n{log}--- end ---"),
        Err(e) => println!("  no log: {e}"),
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--run") {
        run_server_alone("/bin/zedrs");
        return;
    }
    let exe = std::env::args().nth(1).unwrap_or_else(|| "/bin/zedrs".into());
    println!("stdremote: Zed's remote_server through its proxy");
    if !Path::new(&exe).exists() {
        println!("stdremote: skipped, there is no {exe}");
        return;
    }
    let mut failed = 0;

    // Sets the command interpreter, since otherwise Zed uses the /etc/passwd
    // shell, a session program on MikuOS
    let child = Command::new(&exe)
        .env("SHELL", "/bin/msh")
        .args(["proxy", "--identifier", "stdremote"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn();
    check("the proxy starts", child.is_ok(), &mut failed);
    let Ok(mut child) = child else {
        println!("stdremote: FAILURES");
        std::process::exit(1);
    };

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut frame = (PING.len() as u32).to_le_bytes().to_vec();
    frame.extend_from_slice(&PING);
    check("a Ping is written", stdin.write_all(&frame).is_ok(), &mut failed);

    // Reads frames on a separate thread, so a silent server fails with a
    // reason. The server sends `RemoteStarted` (381) first, then the frame
    // responding to 1
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || loop {
        let mut len = [0u8; 4];
        let r = stdout.read_exact(&mut len).and_then(|()| {
            let mut body = vec![0u8; u32::from_le_bytes(len) as usize];
            stdout.read_exact(&mut body).map(|()| body)
        });
        let end = r.is_err();
        if tx.send(r).is_err() || end {
            return;
        }
    });

    let (mut started, mut answered) = (false, None);
    while answered.is_none() {
        match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(Ok(body)) => {
                let decoded = decode(&body);
                println!("          {} bytes back: {:?}", body.len(), decoded);
                match decoded {
                    Some((None, Some(381))) => started = true,
                    Some((Some(1), payload)) => answered = Some(payload),
                    _ => {}
                }
            }
            Ok(Err(e)) => {
                println!("          the stream ended: {e}");
                break;
            }
            Err(_) => {
                println!("          nothing for 60 s");
                break;
            }
        }
    }
    check("the server says it started (RemoteStarted)", started, &mut failed);
    check("the server answers the Ping: Ack, responding to 1", answered == Some(Some(5)), &mut failed);

    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();

    println!("stdremote: {}", if failed == 0 { "all passed" } else { "FAILURES" });
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
