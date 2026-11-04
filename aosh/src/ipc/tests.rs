// IpcHandler calls, a signal word, and the socket, on the host

use std::path::Path;

use crate::object::engine::Engine;

fn load(src: &str) -> (Engine, u32) {
    let mut e = Engine::new().unwrap();
    let root = e.load_source("test.qml", src, Path::new(".")).unwrap();
    (e, root)
}

const HANDLER: &str = r#"
    Item {
        id: root
        property var got: []
        property int toggles: 0
        IpcHandler {
            id: h
            target: "rect"
            function set(n: int, on: bool, k: real, s: string, any): string {
                root.got = [typeof n, n + 1, on, k * 2, s, any];
                return "done";
            }
            function count(): int { return 3 }
            function nothing(): void { }
        }
        IpcHandler {
            id: launcher
            target: "launcher"
            function toggle() { root.toggles++ }
        }
    }
"#;

fn call(e: &Engine, target: &str, f: &str, args: &[&str]) -> Result<String, String> {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    e.with_ctx(|ctx, core| crate::ipc::call(ctx, core, target, f, &args))
}

#[test]
fn a_call_converts_its_arguments_by_their_declared_types() {
    let (e, r) = load(HANDLER);
    assert_eq!(call(&e, "rect", "set", &["41", "false", "1.5", "x", "7"]).unwrap(), "done");
    assert_eq!(e.eval(r, "JSON.stringify(got)").unwrap(), r#"["number",42,false,3,"x","7"]"#);
    assert_eq!(call(&e, "rect", "count", &[]).unwrap(), "3");
    assert_eq!(call(&e, "rect", "nothing", &[]).unwrap(), "", "void says nothing");
    // Missing arguments are undefined, as in JS
    assert_eq!(call(&e, "rect", "set", &["1"]).unwrap(), "done");
    assert!(call(&e, "nobody", "set", &[]).unwrap_err().contains("no target"));
    assert!(call(&e, "rect", "fly", &[]).unwrap_err().contains("no function"));
    assert!(call(&e, "rect", "set", &["x"]).unwrap_err().contains("not an int"));
    assert!(call(&e, "rect", "count", &["1"]).unwrap_err().contains("argument"));
    let shown = e.with_ctx(|_, core| crate::ipc::show(core));
    assert!(shown.contains("target rect\n  function set(n: int, on: bool, k: real, s: string, any): string"), "{shown}");
    assert!(shown.contains("  function toggle(): void"), "{shown}");
}

#[test]
fn a_signal_word_is_a_call_and_a_disabled_handler_takes_none() {
    let (e, r) = load(HANDLER);
    assert!(e.with_ctx(|ctx, core| crate::ipc::word(ctx, core, "launcher toggle")));
    assert_eq!(e.eval(r, "toggles").unwrap(), "1");
    // A bare word is not a call and stays with `Desktop.signalled`
    assert!(!e.with_ctx(|ctx, core| crate::ipc::word(ctx, core, "launcher")));
    e.eval(r, "launcher.enabled = false").unwrap();
    assert!(!e.with_ctx(|ctx, core| crate::ipc::word(ctx, core, "launcher toggle")));
    assert_eq!(e.eval(r, "toggles").unwrap(), "1");
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

#[test]
fn the_socket_answers_a_call_and_show_and_only_one_request_each() {
    let (e, r) = load(HANDLER);
    let dir = std::env::temp_dir().join(format!("aosh-ipc-{}", std::process::id()));
    let path = dir.join("ipc.sock");
    let server = crate::ipc::socket::Server::open(&path).unwrap();
    e.core.0.borrow_mut().ipc.server = Some(server);

    let ask = |fields: &[&str]| {
        let path = path.clone();
        let fields: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut s = std::os::unix::net::UnixStream::connect(&path).unwrap();
            let body = fields.join("\0");
            let mut req = (body.len() as u32).to_le_bytes().to_vec();
            req.extend_from_slice(body.as_bytes());
            s.write_all(&req).unwrap();
            let mut back = Vec::new();
            s.read_to_end(&mut back).unwrap();
            back
        })
    };
    let serve_until = |t: &std::thread::JoinHandle<Vec<u8>>| {
        let start = std::time::Instant::now();
        while !t.is_finished() && start.elapsed().as_secs() < 3 {
            for (fd, f) in e.io_fds() {
                let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
                unsafe { libc::poll(&mut p, 1, 10) };
                if p.revents != 0 {
                    e.io_ready(f);
                }
            }
        }
    };
    let t = ask(&["call", "launcher", "toggle"]);
    serve_until(&t);
    assert_eq!(t.join().unwrap(), b"0");
    assert_eq!(e.eval(r, "toggles").unwrap(), "1");

    let t = ask(&["call", "rect", "count"]);
    serve_until(&t);
    assert_eq!(t.join().unwrap(), b"03");

    let t = ask(&["call", "rect", "fly"]);
    serve_until(&t);
    let back = t.join().unwrap();
    assert_eq!(back[0], b'1');

    let t = ask(&["show"]);
    serve_until(&t);
    assert!(String::from_utf8_lossy(&t.join().unwrap()).contains("target launcher"));

    // The shell going takes its socket with it
    e.core.0.borrow_mut().ipc.server = None;
    assert!(!path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
