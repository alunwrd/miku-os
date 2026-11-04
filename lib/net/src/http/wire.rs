//! The bytes of HTTP: URLs, requests, responses, chunked framing, and the
//! streaming decoder. Not `userspace`-gated, so `cargo test` covers it without
//! a socket. The transport is `super`

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

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
        if crate::interrupted() { break; }
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

/// Where a body goes as it arrives. Functions take `&mut S` with `S: ?Sized`,
/// so a `&mut dyn BodySink` can be passed. Returning `false` stops the
/// transfer, which is also how a size cap works
pub trait BodySink {
    fn chunk(&mut self, bytes: &[u8]) -> bool;
}

/// The sink `get` uses
impl BodySink for Vec<u8> {
    fn chunk(&mut self, bytes: &[u8]) -> bool {
        self.extend_from_slice(bytes);
        true
    }
}

/// Longest stall before a transfer counts as finished. Restarts on every byte,
/// so it bounds the gap, not the transfer
pub const RECV_IDLE_MS: u64 = 4_000;

pub struct GetOpts {
    /// Requests the body from this byte onward, for resuming a download
    pub range_from: Option<u64>,
    /// Stops after this many body bytes, against an endless response
    pub max_bytes: u64,
    /// Longest stall. Restarts on every byte
    pub idle_ms: u64,
}

impl Default for GetOpts {
    fn default() -> GetOpts {
        GetOpts { range_from: None, max_bytes: 64 * 1024 * 1024, idle_ms: RECV_IDLE_MS }
    }
}

/// What came back, without the body, which went to the sink
pub struct GetInfo {
    pub status: u16,
    pub reason: String,
    pub location: Option<String>,
    /// What the server said it would send. For a partial answer, the length of
    /// the part
    pub content_length: Option<u64>,
    pub chunked: bool,
    /// How many bytes actually reached the sink
    pub received: u64,
    /// The server honored a `Range` and answered 206. One ignoring it answers
    /// 200 with the whole body, which must not be appended
    pub partial: bool,
    /// The sink asked to stop, or `max_bytes` was reached
    pub truncated: bool,
}

/// Largest response head
const MAX_HEAD: usize = 64 * 1024;

pub struct Decoder {
    head: Vec<u8>,
    info: Option<GetInfo>,
    state: State,
    max: u64,
    received: u64,
    truncated: bool,
    /// What is left of a chunk header or trailer that a segment cut in half
    pending: Vec<u8>,
    /// A redirect's body is consumed and discarded; the status and location are
    /// still reported
    skip: bool,
}

pub enum State {
    Head,
    /// `None` when the server gave no length and the body ends with the
    /// connection
    Plain { remaining: Option<u64> },
    ChunkHeader,
    ChunkData { remaining: u64 },
    /// The CRLF after a chunk's data
    ChunkEnd,
    Done,
}

impl Decoder {
    pub fn new(max: u64) -> Decoder {
        Decoder {
            head: Vec::new(),
            info: None,
            state: State::Head,
            max,
            received: 0,
            truncated: false,
            pending: Vec::new(),
            skip: false,
        }
    }

    /// Hands over what arrived. `false` when there is no reason to read more:
    /// the body is complete, the cap was reached, or the sink refused
    pub fn feed<S: BodySink + ?Sized>(&mut self, bytes: &[u8], sink: &mut S) -> bool {
        match self.state {
            State::Done => return false,
            State::Head => {}
            _ => return self.body(bytes, sink),
        }

        // Still in the head. It may arrive in pieces, and the first body bytes
        // may share its last read
        self.head.extend_from_slice(bytes);
        let at = match find_headers_end(&self.head) {
            None => {
                // A head that never ends
                if self.head.len() > MAX_HEAD {
                    self.state = State::Done;
                    return false;
                }
                return true;
            }
            Some(at) => at,
        };

        let parsed = match parse_response(&self.head[..at + 4]) {
            Some(p) => p,
            None => {
                self.state = State::Done;
                return false;
            }
        };
        self.state = if parsed.chunked {
            State::ChunkHeader
        } else {
            State::Plain { remaining: parsed.content_length.map(|n| n as u64) }
        };
        self.info = Some(GetInfo {
            status: parsed.status,
            reason: parsed.reason.clone(),
            location: parsed.location.clone(),
            content_length: parsed.content_length.map(|n| n as u64),
            chunked: parsed.chunked,
            received: 0,
            // 200 in answer to a `Range` means the whole body; reported so the
            // caller does not append it
            partial: parsed.status == 206,
            truncated: false,
        });

        self.skip = matches!(parsed.status, 301 | 302 | 303 | 307 | 308)
            && parsed.location.is_some();

        let rest: Vec<u8> = self.head[at + 4..].to_vec();
        self.head = Vec::new();
        self.body(&rest, sink)
    }

    fn body<S: BodySink + ?Sized>(&mut self, mut bytes: &[u8], sink: &mut S) -> bool {
        while !bytes.is_empty() {
            match self.state {
                State::Done => return false,
                State::Head => return true,

                State::Plain { remaining } => {
                    let want = match remaining {
                        Some(r) => (r as usize).min(bytes.len()),
                        None => bytes.len(),
                    };
                    if !self.give(&bytes[..want], sink) {
                        return false;
                    }
                    bytes = &bytes[want..];
                    self.state = match remaining {
                        Some(r) if r as usize <= want => State::Done,
                        Some(r) => State::Plain { remaining: Some(r - want as u64) },
                        None => State::Plain { remaining: None },
                    };
                    if matches!(self.state, State::Done) {
                        return false;
                    }
                }

                State::ChunkHeader => {
                    // The size line may be split across two reads
                    let (line, rest) = match take_line(&mut self.pending, bytes) {
                        None => return true,
                        Some(v) => v,
                    };
                    bytes = rest;
                    // `1a;ext=1` is a legal chunk size with an extension
                    let digits: Vec<u8> = line
                        .iter()
                        .copied()
                        .take_while(|c| c.is_ascii_hexdigit())
                        .collect();
                    let size = parse_hex(&digits) as u64;
                    self.state = if size == 0 {
                        // The trailer is ignored
                        State::Done
                    } else {
                        State::ChunkData { remaining: size }
                    };
                    if matches!(self.state, State::Done) {
                        return false;
                    }
                }

                State::ChunkData { remaining } => {
                    let want = (remaining as usize).min(bytes.len());
                    if !self.give(&bytes[..want], sink) {
                        return false;
                    }
                    bytes = &bytes[want..];
                    self.state = if remaining as usize <= want {
                        State::ChunkEnd
                    } else {
                        State::ChunkData { remaining: remaining - want as u64 }
                    };
                }

                State::ChunkEnd => {
                    match take_line(&mut self.pending, bytes) {
                        None => return true,
                        Some((_, rest)) => {
                            bytes = rest;
                            self.state = State::ChunkHeader;
                        }
                    }
                }
            }
        }
        true
    }

    /// Hands bytes over, honoring the cap; the cap and the sink's refusal both
    /// stop the transfer
    fn give<S: BodySink + ?Sized>(&mut self, bytes: &[u8], sink: &mut S) -> bool {
        if self.skip {
            // Consumed to keep the framing in step, counted as nothing since
            // nothing was delivered
            return true;
        }
        let room = self.max.saturating_sub(self.received);
        let take = (bytes.len() as u64).min(room) as usize;
        if take < bytes.len() {
            self.truncated = true;
        }
        if take > 0 && !sink.chunk(&bytes[..take]) {
            self.truncated = true;
            self.received += take as u64;
            self.state = State::Done;
            return false;
        }
        self.received += take as u64;
        if self.truncated {
            self.state = State::Done;
            return false;
        }
        true
    }

    pub fn finish(mut self) -> Option<GetInfo> {
        let mut info = self.info.take()?;
        info.received = self.received;
        info.truncated = self.truncated;
        Some(info)
    }
}

fn find_headers_end(data: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i + 3 < data.len() {
        if &data[i..i + 4] == b"\r\n\r\n" {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// A CRLF-terminated line that may have arrived in pieces. Returns the line
/// without its terminator and the rest, or `None` while incomplete, keeping
/// what was seen in `pending`
fn take_line<'a>(pending: &mut Vec<u8>, bytes: &'a [u8]) -> Option<(Vec<u8>, &'a [u8])> {
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            let mut line = core::mem::take(pending);
            line.extend_from_slice(&bytes[..i]);
            while line.last() == Some(&b'\r') {
                line.pop();
            }
            return Some((line, &bytes[i + 1..]));
        }
    }
    pending.extend_from_slice(bytes);
    // Longest chunk header
    if pending.len() > 1024 {
        pending.clear();
    }
    None
}
/// A number as decimal bytes, for a header, without a formatter
pub fn fmt_u64(mut n: u64, buf: &mut [u8; 20]) -> usize {
    if n == 0 { buf[0] = b'0'; return 1; }
    let mut tmp = [0u8; 20]; let mut len = 0;
    while n > 0 { tmp[len] = b'0' + (n % 10) as u8; n /= 10; len += 1; }
    for i in 0..len { buf[i] = tmp[len-1-i]; }
    len
}
