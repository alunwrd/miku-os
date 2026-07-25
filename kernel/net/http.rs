extern crate alloc;
use alloc::vec::Vec;
use alloc::string::String;
use core::sync::atomic::Ordering;
use super::CTRL_C;

pub struct ParsedUrl<'a> {
    pub scheme:   &'a str,
    pub host:     &'a str,
    pub hostname: &'a str,
    pub port:     u16,
    pub path:     &'a str,
    pub use_tls:  bool,
}

impl<'a> ParsedUrl<'a> {
    pub fn parse(url: &'a str) -> Option<Self> {
        let (scheme, rest) = if url.starts_with("https://") {
            ("https", &url[8..])
        } else if url.starts_with("http://") {
            ("http", &url[7..])
        } else {
            ("http", url)
        };
        let use_tls = scheme == "https";

        let (host, path) = if let Some(slash) = rest.find('/') {
            (&rest[..slash], &rest[slash..])
        } else {
            (rest, "/")
        };

        let (hostname, port) = if let Some(colon) = host.rfind(':') {
            let p: u16 = host[colon + 1..].parse().ok()?;
            (&host[..colon], p)
        } else {
            (host, if use_tls { 443 } else { 80 })
        };

        Some(ParsedUrl { scheme, host, hostname, port, path, use_tls })
    }
}

pub struct HttpRequest<'a> {
    pub method:  &'a str,
    pub path:    &'a str,
    pub host:    &'a str,
    pub body:    Option<&'a [u8]>,
    pub headers: &'a [(&'a str, &'a str)],
}

impl<'a> HttpRequest<'a> {
    pub fn write_to(&self, buf: &mut Vec<u8>) {
        let push = |buf: &mut Vec<u8>, s: &str| buf.extend_from_slice(s.as_bytes());

        push(buf, self.method);
        push(buf, " ");
        push(buf, self.path);
        push(buf, " HTTP/1.1\r\nHost: ");
        push(buf, self.host);
        push(buf, "\r\nUser-Agent: MikuOS/0.1\r\nConnection: close\r\nAccept: */*\r\n");

        for (k, v) in self.headers {
            push(buf, k); push(buf, ": "); push(buf, v); push(buf, "\r\n");
        }

        if let Some(body) = self.body {
            let mut tmp = [0u8; 20];
            let n = fmt_u64(body.len() as u64, &mut tmp);
            push(buf, "Content-Length: ");
            buf.extend_from_slice(&tmp[..n]);
            push(buf, "\r\n\r\n");
            buf.extend_from_slice(body);
        } else {
            push(buf, "\r\n");
        }
    }
}

pub struct HttpResponse {
    pub status:         u16,
    pub reason:         String,
    pub location:       Option<String>,
    pub body:           Vec<u8>,
    pub content_length: Option<usize>,
    pub chunked:        bool,
}

impl HttpResponse {
    fn new() -> Self {
        Self { status: 0, reason: String::new(), location: None,
               body: Vec::new(), content_length: None, chunked: false }
    }
}

pub fn parse_response(data: &[u8]) -> Option<HttpResponse> {
    let hdr_end = {
        let mut pos = None;
        let mut i   = 0usize;
        while i + 3 < data.len() {
            if &data[i..i+4] == b"\r\n\r\n" { pos = Some(i); break; }
            i += 1;
        }
        pos?
    };

    let mut resp  = HttpResponse::new();
    let mut lines = data[..hdr_end].split(|&b| b == b'\n');

    {
        let sl = trim_crlf(lines.next()?);
        let mut p = sl.splitn(3, |&b| b == b' ');
        let _ver  = p.next()?;
        let code  = p.next()?;
        let rsn   = p.next().unwrap_or(b"");
        resp.status = core::str::from_utf8(code).ok()?.trim().parse().ok()?;
        resp.reason = String::from(core::str::from_utf8(rsn).unwrap_or("").trim());
    }

    for line in lines {
        let line = trim_crlf(line);
        if line.is_empty() { continue; }
        if let Some(colon) = line.iter().position(|&b| b == b':') {
            let key   = ascii_lower(trim_ws(&line[..colon]));
            let value = trim_ws(&line[colon + 1..]);
            match key.as_str() {
                "content-length" => {
                    if let Ok(s) = core::str::from_utf8(value) {
                        resp.content_length = s.trim().parse().ok();
                    }
                }
                "transfer-encoding" => {
                    if let Ok(s) = core::str::from_utf8(value) {
                        if s.trim().eq_ignore_ascii_case("chunked") { resp.chunked = true; }
                    }
                }
                "location" => {
                    if let Ok(s) = core::str::from_utf8(value) {
                        resp.location = Some(String::from(s.trim()));
                    }
                }
                _ => {}
            }
        }
    }

    let body_start = hdr_end + 4;
    if body_start <= data.len() {
        let raw = &data[body_start..];
        resp.body = if resp.chunked { decode_chunked(raw) } else { Vec::from(raw) };
    }

    Some(resp)
}

fn decode_chunked(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    loop {
        if CTRL_C.load(Ordering::SeqCst) { break; }
        let crlf = match find_crlf(data, pos) { Some(e) => e, None => break };
        let size  = parse_hex(trim_crlf(&data[pos..crlf]));
        pos = crlf + 2;
        if size == 0 { break; }
        let end = (pos + size).min(data.len());
        out.extend_from_slice(&data[pos..end]);
        if pos + size >= data.len() { break; }
        pos = pos + size + 2;
    }
    out
}

fn find_crlf(data: &[u8], from: usize) -> Option<usize> {
    for i in from..data.len().saturating_sub(1) {
        if data[i] == b'\r' && data[i+1] == b'\n' { return Some(i); }
    }
    None
}

fn parse_hex(s: &[u8]) -> usize {
    let mut v = 0usize;
    for &b in s {
        let d = match b {
            b'0'..=b'9' => (b - b'0') as usize,
            b'a'..=b'f' => (b - b'a' + 10) as usize,
            b'A'..=b'F' => (b - b'A' + 10) as usize,
            _           => break,
        };
        v = v.wrapping_mul(16).wrapping_add(d);
    }
    v
}

fn trim_crlf(s: &[u8]) -> &[u8] {
    let mut e = s.len();
    while e > 0 && (s[e-1] == b'\r' || s[e-1] == b'\n') { e -= 1; }
    &s[..e]
}

fn trim_ws(s: &[u8]) -> &[u8] {
    let st = s.iter().position(|&b| b != b' ' && b != b'\t').unwrap_or(0);
    let en = s.iter().rposition(|&b| b != b' ' && b != b'\t').map(|i| i+1).unwrap_or(0);
    if st < en { &s[st..en] } else { b"" }
}

fn ascii_lower(s: &[u8]) -> String {
    let mut o = String::with_capacity(s.len());
    for &b in s { o.push(if b.is_ascii_uppercase() { (b+32) as char } else { b as char }); }
    o
}

fn do_request(url: &ParsedUrl, req_bytes: &[u8]) -> Option<Vec<u8>> {
    if !crate::net::is_ready() {
        crate::print_error!("http: net not ready (run dhcp first)");
        return None;
    }
    for _ in 0..3 { crate::net::poll(); }

    let dns = crate::net::get_dns();
    let ip  = if let Some(ip) = parse_ip(url.hostname) {
        ip
    } else {
        crate::cprintln!(57, 197, 187, "http: resolving {}...", url.hostname);
        match crate::net::dns::resolve(url.hostname, &dns) {
            Some(ip) => ip,
            None => { crate::print_error!("http: cannot resolve '{}'", url.hostname); return None; }
        }
    };

    crate::cprintln!(57, 197, 187,
        "http: {}:{} ({})", url.hostname, url.port,
        if url.use_tls { "TLS" } else { "plain" });

    x86_64::instructions::interrupts::enable();

    if url.use_tls {
        crate::cprintln!(120, 200, 200, "http: TLS handshake...");
        let mut s = match crate::net::tls::TlsStream::connect(url.hostname, ip, url.port) {
            Some(s) => s,
            None => { crate::print_error!("http: TLS failed"); return None; }
        };
        crate::print_success!("http: TLS ok ({})", s.cipher_name());
        
        if s.is_h2() {
            crate::cprintln!(57, 197, 187, "http: using HTTP/2");
            let resp = crate::net::http2::h2_request(
                &mut s, "GET", "https", url.hostname, url.path, None,
            )?;
            s.close();
            let status_line = alloc::format!("HTTP/2 {} \r\n\r\n", resp.status);
            let mut out = Vec::from(status_line.as_bytes());
            out.extend_from_slice(&resp.body);
            return Some(out);
        }
        
        if !s.send(req_bytes) { crate::print_error!("http: send failed"); s.close(); return None; }
        let out = Vec::from(s.recv_all(10_000_000));
        crate::log!("http: {} bytes: 0x{:02x} 0x{:02x} 0x{:02x} 0x{:02x} 0x{:02x} 0x{:02x} 0x{:02x} 0x{:02x}", out.len(), out.get(0).copied().unwrap_or(0), out.get(1).copied().unwrap_or(0), out.get(2).copied().unwrap_or(0), out.get(3).copied().unwrap_or(0), out.get(4).copied().unwrap_or(0), out.get(5).copied().unwrap_or(0), out.get(6).copied().unwrap_or(0), out.get(7).copied().unwrap_or(0));
        s.close();
        Some(out)
    } else {
        let mut s = match crate::net::tcp::TcpSocket::connect(ip, url.port) {
            Some(s) => s,
            None => { crate::print_error!("http: connect failed"); return None; }
        };
        crate::print_success!("http: connected");
        if !s.send(req_bytes) { crate::print_error!("http: send failed"); s.close(); return None; }
        let out = Vec::from(s.recv_all(10_000_000));
        crate::log!("http: {} bytes: 0x{:02x} 0x{:02x} 0x{:02x} 0x{:02x} 0x{:02x} 0x{:02x} 0x{:02x} 0x{:02x}", out.len(), out.get(0).copied().unwrap_or(0), out.get(1).copied().unwrap_or(0), out.get(2).copied().unwrap_or(0), out.get(3).copied().unwrap_or(0), out.get(4).copied().unwrap_or(0), out.get(5).copied().unwrap_or(0), out.get(6).copied().unwrap_or(0), out.get(7).copied().unwrap_or(0));
        s.close();
        Some(out)
    }
}

pub fn get(url_str: &str) -> Option<HttpResponse> {
    let mut current = String::from(url_str);
    for _ in 0..5 {
        if CTRL_C.load(Ordering::SeqCst) { return None; }
        let url = ParsedUrl::parse(&current)?;
        let mut req = Vec::new();
        HttpRequest { method: "GET", path: url.path, host: url.host,
                      body: None, headers: &[] }.write_to(&mut req);
        let raw  = do_request(&url, &req)?;
        let resp = parse_response(&raw)?;

        match resp.status {
            301 | 302 | 303 | 307 | 308 => {
                if let Some(ref loc) = resp.location.clone() {
                    crate::cprintln!(120, 200, 200, "http: redirect {} -> {}", resp.status, loc);
                    current = if loc.starts_with("http") {
                        loc.clone()
                    } else {
                        alloc::format!("{}://{}{}", url.scheme, url.host, loc)
                    };
                    continue;
                }
                return Some(resp);
            }
            _ => return Some(resp),
        }
    }
    crate::print_error!("http: too many redirects");
    None
}

pub fn post(url_str: &str, body: &[u8], content_type: &str) -> Option<HttpResponse> {
    if CTRL_C.load(Ordering::SeqCst) { return None; }
    let url = ParsedUrl::parse(url_str)?;
    let mut req = Vec::new();
    HttpRequest { method: "POST", path: url.path, host: url.host,
                  body: Some(body), headers: &[("Content-Type", content_type)] }
        .write_to(&mut req);
    let raw = do_request(&url, &req)?;
    parse_response(&raw)
}

fn vfs_write_bytes(path: &str, data: &[u8]) -> Result<usize, &'static str> {
    use crate::fs::vfs::{with_vfs, OpenFlags, FileMode};

    // Resolve relative paths against the calling process's cwd, not the
    // shell session: the network stack has no business reaching into a
    // shell, and this also does the right thing for non-shell callers
    let cwd = crate::sched::current_cwd() as usize;
    let fl  = OpenFlags(OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE);

    with_vfs(|v| {
        let fd = match v.open(cwd, path, fl, FileMode::default_file()) {
            Ok(fd) => fd,
            Err(_) => return Err("open failed"),
        };
        let n = match v.write(fd, data) {
            Ok(n) => n,
            Err(_) => { let _ = v.close(fd); return Err("write failed"); }
        };
        let _ = v.close(fd);
        Ok(n)
    })
}

pub fn cmd_wget(args: &str) {
    super::CTRL_C.store(false, Ordering::SeqCst);
    let mut parts   = args.split_whitespace();
    let url_str     = match parts.next() {
        Some(u) => u,
        None => { crate::println!("Usage: wget <url> [-O <file>]"); return; }
    };

    let mut out_path: Option<&str> = None;
    while let Some(flag) = parts.next() {
        if flag == "-O" || flag == "-o" { out_path = parts.next(); }
    }

    let inferred;
    let save_as = match out_path {
        Some(p) => p,
        None => { inferred = infer_filename(url_str); inferred.as_str() }
    };

    crate::cprintln!(57, 197, 187, "wget: {}", url_str);

    let resp = match get(url_str) { Some(r) => r, None => return };

    let sc = status_color(resp.status);
    crate::cprintln!(sc.0, sc.1, sc.2, "HTTP {} {}", resp.status, resp.reason);

    if resp.body.is_empty() { crate::print_warn!("wget: empty response"); return; }
    crate::cprintln!(120, 200, 200, "wget: {} bytes received", resp.body.len());

    match vfs_write_bytes(save_as, &resp.body) {
        Ok(n)  => crate::print_success!("wget: '{}' saved ({} bytes)", save_as, n),
        Err(e) => {
            crate::print_error!("wget: save failed ({}), printing to screen:", e);
            print_body(&resp.body, 4096);
        }
    }
}

pub fn cmd_curl(args: &str) {
    super::CTRL_C.store(false, Ordering::SeqCst);
    let mut parts   = args.split_whitespace();
    let url_str     = match parts.next() {
        Some(u) => u,
        None => {
            crate::println!("Usage: curl <url> [-X GET|POST] [-d <data>] [-o <file>] [-I]");
            return;
        }
    };

    let mut method    = "GET";
    let mut post_data: Option<&str> = None;
    let mut out_path:  Option<&str> = None;
    let mut show_hdrs = false;

    while let Some(flag) = parts.next() {
        match flag {
            "-X"        => { method = parts.next().unwrap_or("GET"); }
            "-d"        => { post_data = parts.next(); method = "POST"; }
            "-o" | "-O" => { out_path  = parts.next(); }
            "-I" | "-i" => { show_hdrs = true; }
            _ => {}
        }
    }

    crate::cprintln!(57, 197, 187, "curl: {} {}", method, url_str);

    let resp = if method == "POST" {
        post(url_str, post_data.unwrap_or("").as_bytes(),
             "application/x-www-form-urlencoded")
    } else {
        get(url_str)
    };

    let resp = match resp { Some(r) => r, None => return };

    let sc = status_color(resp.status);
    crate::cprintln!(sc.0, sc.1, sc.2, "< HTTP/1.1 {} {}", resp.status, resp.reason);

    if show_hdrs {
        if let Some(cl) = resp.content_length {
            crate::cprintln!(120, 200, 200, "< Content-Length: {}", cl);
        }
        if resp.chunked {
            crate::cprintln!(120, 200, 200, "< Transfer-Encoding: chunked");
        }
        crate::println!("");
    }

    if resp.body.is_empty() { crate::print_warn!("curl: empty body"); return; }
    crate::cprintln!(120, 200, 200, "curl: {} bytes", resp.body.len());

    match out_path {
        Some(path) => match vfs_write_bytes(path, &resp.body) {
            Ok(n)  => crate::print_success!("curl: saved '{}' ({} bytes)", path, n),
            Err(e) => crate::print_error!("curl: save failed: {}", e),
        },
        None => print_body(&resp.body, 8192),
    }
}

fn print_body(body: &[u8], limit: usize) {
    let show = body.len().min(limit);
    let mut s = String::with_capacity(show);
    for &b in &body[..show] {
        match b {
            b'\n' | b'\t' => s.push(b as char),
            b'\r'         => {}
            32..=126      => s.push(b as char),
            _             => s.push('.'),
        }
    }
    crate::println!("{}", s);
    if body.len() > show {
        crate::cprintln!(120, 140, 140,
            "... ({} bytes total, showing {})", body.len(), limit);
    }
}

fn infer_filename(url: &str) -> String {
    let after = if let Some(i) = url.find("://") { &url[i+3..] } else { url };
    let path  = if let Some(i) = after.find('/') { &after[i+1..] } else { "" };
    let name  = path.rsplit('/').next().unwrap_or("");
    let name  = name.split('?').next().unwrap_or("");
    let name  = name.split('#').next().unwrap_or("");
    if name.is_empty() { String::from("index.html") } else { String::from(name) }
}

fn status_color(s: u16) -> (u8, u8, u8) {
    if s < 300 { (100, 220, 150) } else if s < 400 { (57, 197, 187) } else { (255, 80, 80) }
}

fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let mut p = s.split('.');
    Some([p.next()?.parse().ok()?, p.next()?.parse().ok()?,
          p.next()?.parse().ok()?, p.next()?.parse().ok()?])
}

fn fmt_u64(mut n: u64, buf: &mut [u8; 20]) -> usize {
    if n == 0 { buf[0] = b'0'; return 1; }
    let mut tmp = [0u8; 20]; let mut len = 0;
    while n > 0 { tmp[len] = b'0' + (n % 10) as u8; n /= 10; len += 1; }
    for i in 0..len { buf[i] = tmp[len-1-i]; }
    len
}
