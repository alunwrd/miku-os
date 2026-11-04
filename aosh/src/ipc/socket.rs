// Socket used by `aosh ipc`
//
// Request: 4-byte length, then NUL-joined fields (`call`, target, function,
// arguments; or `show`). Response: a status byte (`0` value, `1` error), then
// text until close.
//
// MikuOS local sockets have no `shutdown` or timeouts, so the length marks the
// end of a request and the server polls for at most 200 ms.
//
// Only peers with this user's uid (`SO_PEERCRED`) are answered. The socket
// directory is 0700.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// How long a connected client has to say what it wants
const WAIT_MS: u64 = 200;
const MAX_REQUEST: usize = 64 * 1024;

pub struct Server {
    listener: UnixListener,
    path: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// `$XDG_RUNTIME_DIR/aosh/ipc.sock`, or `~/.cache/aosh/ipc.sock` when unset.
/// `aosh -c NAME` uses `NAME.sock` in the same directory
pub fn path(config: Option<&str>) -> Option<PathBuf> {
    let file = match config {
        Some(name) => format!("aosh/{name}.sock"),
        None => "aosh/ipc.sock".to_string(),
    };
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d).join(file));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache").join(file))
}

impl Server {
    pub fn open(path: &Path) -> std::io::Result<Server> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        // One an earlier shell left behind
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        listener.set_nonblocking(true)?;
        Ok(Server { listener, path: path.to_path_buf() })
    }

    pub fn fd(&self) -> i32 {
        self.listener.as_raw_fd()
    }

    /// All waiting connections, each with its request or a refusal reason
    pub fn take(&self) -> Vec<(UnixStream, Result<Vec<String>, String>)> {
        let mut out = Vec::new();
        while let Ok((s, _)) = self.listener.accept() {
            let req = if same_user(&s) { read_request(&s) } else { Err("that shell is somebody else's".into()) };
            out.push((s, req));
        }
        out
    }
}

/// The answer, and the connection closed after it
pub fn answer(mut s: UnixStream, r: Result<String, String>) {
    let _ = s.set_nonblocking(false);
    let (status, text) = match r {
        Ok(t) => (b'0', t),
        Err(e) => (b'1', e),
    };
    let mut out = vec![status];
    out.extend_from_slice(text.as_bytes());
    let _ = s.write_all(&out);
}

fn same_user(s: &UnixStream) -> bool {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: owned descriptor and a correctly sized buffer
    let r = unsafe {
        libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED,
                         (&mut cred as *mut libc::ucred).cast(), &mut len)
    };
    // SAFETY: no arguments to validate
    r == 0 && cred.uid == unsafe { libc::geteuid() }
}

fn read_request(s: &UnixStream) -> Result<Vec<String>, String> {
    s.set_nonblocking(true).map_err(|e| e.to_string())?;
    let start = Instant::now();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if buf.len() >= 4 {
            let need = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
            if need > MAX_REQUEST {
                return Err("that request is too long".into());
            }
            if buf.len() >= 4 + need {
                return Ok(buf[4..4 + need].split(|&b| b == 0).map(|f| String::from_utf8_lossy(f).into_owned()).collect());
            }
        }
        let left = WAIT_MS.saturating_sub(start.elapsed().as_millis() as u64);
        if left == 0 {
            return Err("the request did not arrive".into());
        }
        let mut p = libc::pollfd { fd: s.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd
        unsafe { libc::poll(&mut p, 1, left as i32) };
        match (&*s).read(&mut chunk) {
            Ok(0) => return Err("the client went before it asked".into()),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// `aosh ipc ...`: sends the request and prints the response. The exit status
/// comes from the response
pub fn client(args: &[String]) -> i32 {
    let usage = || {
        eprintln!("usage: aosh ipc [-c NAME] call <target> <function> [arguments...]");
        eprintln!("       aosh ipc [-c NAME] show");
        2
    };
    // `-c NAME` selects the `aosh -c NAME` shell; otherwise the `default`
    // config
    let default = crate::shell::default_config();
    let (config, args) = match args.first().map(String::as_str) {
        Some("-c") => match args.get(1) {
            Some(name) if crate::shell::config_name(name) => (Some(name.as_str()), &args[2..]),
            _ => return usage(),
        },
        _ => (default.as_deref(), args),
    };
    let fields: Vec<&str> = match args.first().map(String::as_str) {
        Some("call") if args.len() >= 3 => args.iter().map(String::as_str).collect(),
        Some("show") => vec!["show"],
        _ => return usage(),
    };
    let Some(path) = path(config) else {
        eprintln!("aosh ipc: neither XDG_RUNTIME_DIR nor HOME is set, so there is no socket to find");
        return 1;
    };
    let mut s = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("aosh ipc: {}: {e} - is aosh running?", path.display());
            return 1;
        }
    };
    let body = fields.join("\0");
    let mut req = (body.len() as u32).to_le_bytes().to_vec();
    req.extend_from_slice(body.as_bytes());
    if let Err(e) = s.write_all(&req) {
        eprintln!("aosh ipc: {e}");
        return 1;
    }
    let mut back = Vec::new();
    let _ = s.read_to_end(&mut back);
    let Some((&status, text)) = back.split_first() else {
        eprintln!("aosh ipc: the shell closed without answering");
        return 1;
    };
    let text = String::from_utf8_lossy(text);
    if status == b'0' {
        if !text.is_empty() {
            println!("{}", text.trim_end_matches('\n'));
        }
        0
    } else {
        eprintln!("aosh ipc: {text}");
        1
    }
}
