//! HTTP transport: the resolver, the connection, and two ways to request a
//! body. Gated behind `userspace`; the bytes are `super::wire`

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

use super::wire::*;


static TIMING: spin::Mutex<[u64; 4]> = spin::Mutex::new([0; 4]);

pub fn last_timing() -> String {
    let t = *TIMING.lock();
    alloc::format!("resolve={}ms connect={}ms transfer={}ms close={}ms",
        t[0], t[1], t[2], t[3])
}

/// What a request is made against: resolver and trust store, passed by the
/// caller
pub struct Client<'a> {
    /// What certificate chains are checked against
    pub trust: &'a crate::verify::Trust,
    /// The resolver to ask
    pub dns: [u8; 4],
}

impl<'a> Client<'a> {
    pub fn new(trust: &'a crate::verify::Trust, dns: [u8; 4]) -> Self {
        Self { trust, dns }
    }

    /// This machine's resolver, or why it has none
    pub fn from_system(
        trust: &'a crate::verify::Trust,
    ) -> Result<Self, crate::dns::NoResolver> {
        crate::dns::system_resolver().map(|dns| Self { trust, dns })
    }
}

fn do_request(url: &ParsedUrl, req_bytes: &[u8], client: &Client<'_>) -> Option<Vec<u8>> {
    let mut mark = crate::plat::uptime_ms();
    let mut lap = || {
        let now = crate::plat::uptime_ms();
        let d = now.wrapping_sub(mark);
        mark = now;
        d
    };

    let ip = if let Some(ip) = crate::parse_ipv4(url.hostname) {
        ip
    } else {
        netlog!("http: resolving {}...", url.hostname);
        match crate::dns::resolve(url.hostname, &client.dns) {
            Some(ip) => ip,
            None => { netlog_err!("http: cannot resolve '{}'", url.hostname); return None; }
        }
    };

    netlog!("http: {}:{} ({})", url.hostname, url.port,
        if url.use_tls { "TLS" } else { "plain" });

    TIMING.lock()[0] = lap();

    if url.use_tls {
        netlog!("http: TLS handshake...");
        let mut s = match crate::tls::TlsStream::connect(url.hostname, ip, url.port, client.trust) {
            Some(s) => s,
            None => { netlog_err!("http: TLS failed"); return None; }
        };
        netlog!("http: TLS ok ({})", s.cipher_name());
        TIMING.lock()[1] = lap();

        if s.is_h2() {
            netlog!("http: using HTTP/2");
            let resp = crate::http2::h2_request(
                &mut s, "GET", "https", url.hostname, url.path, None,
            )?;
            s.close();
            let status_line = alloc::format!("HTTP/2 {} \r\n\r\n", resp.status);
            let mut out = Vec::from(status_line.as_bytes());
            out.extend_from_slice(&resp.body);
            return Some(out);
        }
        
        if !s.send(req_bytes) { netlog_err!("http: send failed"); s.close(); return None; }
        let mut out = Vec::new();
        s.recv_to_vec(&mut out, RECV_IDLE_MS);
        TIMING.lock()[2] = lap();
        log_head("http", &out);
        s.close();
        TIMING.lock()[3] = lap();
        Some(out)
    } else {
        let mut s = match crate::sock::Stream::connect(ip, url.port) {
            Some(s) => s,
            None => { netlog_err!("http: connect failed"); return None; }
        };
        netlog!("http: connected");
        TIMING.lock()[1] = lap();
        if !s.send(req_bytes) { netlog_err!("http: send failed"); s.close(); return None; }
        let mut out = Vec::new();
        s.recv_to_vec(&mut out, RECV_IDLE_MS);
        TIMING.lock()[2] = lap();
        log_head("http", &out);
        s.close();
        TIMING.lock()[3] = lap();
        Some(out)
    }
}

fn log_head(tag: &str, out: &[u8]) {
    let b = |i: usize| out.get(i).copied().unwrap_or(0);
    netlog!(
        "{}: {} bytes: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
        tag, out.len(), b(0), b(1), b(2), b(3), b(4), b(5), b(6), b(7)
    );
}

/// GET with the body in a `Vec`: `get_to` with a `Vec` as sink. Five redirects,
/// chunked framing, an idle timeout, and a size cap
pub fn get(url_str: &str, client: &Client<'_>) -> Option<HttpResponse> {
    let mut body: Vec<u8> = Vec::new();
    let info = get_to(url_str, client, &GetOpts::default(), &mut body)?;
    Some(HttpResponse {
        status: info.status,
        reason: info.reason,
        location: info.location,
        body,
        content_length: info.content_length.map(|n| n as usize),
        chunked: info.chunked,
    })
}

pub fn post(url_str: &str, body: &[u8], content_type: &str, client: &Client<'_>) -> Option<HttpResponse> {
    if crate::interrupted() { return None; }
    let url = ParsedUrl::parse(url_str)?;
    let mut req = Vec::new();
    HttpRequest { method: "POST", path: url.path, host: url.host,
                  body: Some(body), headers: &[("Content-Type", content_type)] }
        .write_to(&mut req);
    let raw = do_request(&url, &req, client)?;
    parse_response(&raw)
}


// Streaming
//
// The transfer is a state machine fed a run of bytes at a time, so a download
// can be hashed and written as it arrives and resumed. `get` wraps it with a
// `Vec` as sink.

/// GET, with the body handed to `sink` as it arrives.
pub fn get_to<S: BodySink + ?Sized>(
    url_str: &str,
    client: &Client<'_>,
    opts: &GetOpts,
    sink: &mut S,
) -> Option<GetInfo> {
    let mut current = String::from(url_str);
    for _ in 0..5 {
        if crate::interrupted() {
            return None;
        }
        let url = ParsedUrl::parse(&current)?;
        let info = fetch_once(&url, client, opts, sink)?;

        // A redirect's body must not reach the sink. The decoder holds the head
        // back until parsed, so there is nothing to undo
        let is_redirect = matches!(info.status, 301 | 302 | 303 | 307 | 308);
        match (is_redirect, info.location.clone()) {
            (true, Some(next)) => {
                current = if next.starts_with("http://") || next.starts_with("https://") {
                    next
                } else if let Some(rest) = next.strip_prefix('/') {
                    alloc::format!("{}://{}/{}", url.scheme, url.host, rest)
                } else {
                    alloc::format!("{}://{}{}", url.scheme, url.host, next)
                };
            }
            _ => return Some(info),
        }
    }
    None
}

fn fetch_once<S: BodySink + ?Sized>(
    url: &ParsedUrl,
    client: &Client<'_>,
    opts: &GetOpts,
    sink: &mut S,
) -> Option<GetInfo> {
    let mut range_buf = [0u8; 40];
    let headers: [(&str, &str); 1];
    let header_slice: &[(&str, &str)] = match opts.range_from {
        None => &[],
        Some(from) => {
            let mut n = 0usize;
            for b in b"bytes=" {
                range_buf[n] = *b;
                n += 1;
            }
            let mut tmp = [0u8; 20];
            let d = fmt_u64(from, &mut tmp);
            range_buf[n..n + d].copy_from_slice(&tmp[..d]);
            n += d;
            range_buf[n] = b'-';
            n += 1;
            let text = core::str::from_utf8(&range_buf[..n]).ok()?;
            headers = [("Range", text)];
            &headers
        }
    };

    let mut req = Vec::new();
    HttpRequest {
        method: "GET",
        path: url.path,
        host: url.host,
        body: None,
        headers: header_slice,
    }
    .write_to(&mut req);

    let mut mark = crate::plat::uptime_ms();
    let mut lap = || {
        let now = crate::plat::uptime_ms();
        let d = now.wrapping_sub(mark);
        mark = now;
        d
    };

    let ip = match crate::parse_ipv4(url.hostname) {
        Some(ip) => ip,
        None => match crate::dns::resolve(url.hostname, &client.dns) {
            Some(ip) => ip,
            None => {
                netlog_err!("http: cannot resolve '{}'", url.hostname);
                return None;
            }
        },
    };
    TIMING.lock()[0] = lap();

    let mut transport = Transport::open(url, ip, client)?;
    TIMING.lock()[1] = lap();

    // No HTTP/2 streaming path. The h2 case falls back to the buffered request
    // and gives the sink the result in one piece; the size cap still applies
    if transport.is_h2() {
        let resp = transport.h2(url)?;
        let mut received = 0u64;
        let mut truncated = false;
        if resp.body.len() as u64 > opts.max_bytes {
            truncated = true;
        }
        let take = (resp.body.len() as u64).min(opts.max_bytes) as usize;
        if take > 0 && !sink.chunk(&resp.body[..take]) {
            truncated = true;
        }
        received += take as u64;
        transport.close();
        return Some(GetInfo {
            status: resp.status,
            reason: String::new(),
            location: None,
            content_length: Some(resp.body.len() as u64),
            chunked: false,
            received,
            partial: false,
            truncated,
        });
    }

    if !transport.send(&req) {
        netlog_err!("http: send failed");
        transport.close();
        return None;
    }

    let mut decoder = Decoder::new(opts.max_bytes);
    let mut scratch: Vec<u8> = Vec::new();
    loop {
        scratch.clear();
        if !transport.recv_into(&mut scratch, opts.idle_ms) {
            break;
        }
        if !decoder.feed(&scratch, sink) {
            break;
        }
    }
    TIMING.lock()[2] = lap();
    transport.close();
    TIMING.lock()[3] = lap();

    decoder.finish()
}

/// One connection of either kind. The enum keeps "the transfer is over" the
/// same for TLS and plain
enum Transport {
    Tls(crate::tls::TlsStream),
    Plain(crate::sock::Stream),
}

impl Transport {
    fn open(url: &ParsedUrl, ip: [u8; 4], client: &Client<'_>) -> Option<Transport> {
        if url.use_tls {
            let s = crate::tls::TlsStream::connect(url.hostname, ip, url.port, client.trust)?;
            netlog!("http: TLS ok ({})", s.cipher_name());
            Some(Transport::Tls(s))
        } else {
            Some(Transport::Plain(crate::sock::Stream::connect(ip, url.port)?))
        }
    }

    fn is_h2(&self) -> bool {
        match self {
            Transport::Tls(s) => s.is_h2(),
            Transport::Plain(_) => false,
        }
    }

    fn h2(&mut self, url: &ParsedUrl) -> Option<crate::http2::H2Response> {
        match self {
            Transport::Tls(s) => {
                crate::http2::h2_request(s, "GET", "https", url.hostname, url.path, None)
            }
            Transport::Plain(_) => None,
        }
    }

    fn send(&mut self, data: &[u8]) -> bool {
        match self {
            Transport::Tls(s) => s.send(data),
            Transport::Plain(s) => s.send(data),
        }
    }

    /// What has arrived. `false` when there will be no more: the peer closed,
    /// or nothing came for `idle_ms`
    fn recv_into(&mut self, out: &mut Vec<u8>, idle_ms: u64) -> bool {
        match self {
            Transport::Tls(s) => {
                let chunk = s.recv_chunk(idle_ms);
                if chunk.is_empty() {
                    return false;
                }
                out.extend_from_slice(chunk);
                true
            }
            Transport::Plain(s) => {
                const PROBE_MS: u64 = 250;
                let mut idle = 0u64;
                let mut buf = [0u8; 4096];
                loop {
                    if crate::interrupted() {
                        return false;
                    }
                    let mut n = 0usize;
                    s.recv_one_into(&mut buf, &mut n, PROBE_MS);
                    if n > 0 {
                        out.extend_from_slice(&buf[..n]);
                        return true;
                    }
                    // Closed check after the read: a FIN on the same segment as
                    // the last bytes must not lose the tail
                    if s.peer_closed {
                        return false;
                    }
                    idle += PROBE_MS;
                    if idle > idle_ms {
                        return false;
                    }
                }
            }
        }
    }

    fn close(&mut self) {
        match self {
            Transport::Tls(s) => s.close(),
            Transport::Plain(s) => s.close(),
        }
    }
}

