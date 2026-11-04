// The bytes of HTTP against realistic segments. The key case splits a response
// at every byte, as a TCP segment boundary can

use mikunet::http::wire::{BodySink, Decoder, GetInfo, HttpRequest, ParsedUrl};

/// Somewhere for a body to go, and a record of how it got there
#[derive(Default)]
struct Sink {
    bytes: Vec<u8>,
    calls: usize,
    /// Stop after this many bytes, to test a sink that refuses
    stop_after: Option<usize>,
}

impl BodySink for Sink {
    fn chunk(&mut self, bytes: &[u8]) -> bool {
        self.calls += 1;
        self.bytes.extend_from_slice(bytes);
        match self.stop_after {
            Some(limit) => self.bytes.len() < limit,
            None => true,
        }
    }
}

/// Feed a response in pieces of `step` bytes
fn decode(response: &[u8], step: usize, max: u64) -> (Sink, Option<GetInfo>) {
    let mut d = Decoder::new(max);
    let mut sink = Sink::default();
    let mut at = 0usize;
    while at < response.len() {
        let end = (at + step).min(response.len());
        if !d.feed(&response[at..end], &mut sink) {
            break;
        }
        at = end;
    }
    (sink, d.finish())
}

// URLs

#[test]
fn a_url_comes_apart_the_way_it_should() {
    let u = ParsedUrl::parse("https://repo.miku/core/miku-base-0.2.10.mpk").unwrap();
    assert_eq!(u.scheme, "https");
    assert_eq!(u.hostname, "repo.miku");
    assert_eq!(u.port, 443);
    assert_eq!(u.path, "/core/miku-base-0.2.10.mpk");
    assert!(u.use_tls);

    let u = ParsedUrl::parse("http://10.0.2.2:8080/index.mpki").unwrap();
    assert_eq!(u.port, 8080);
    assert_eq!(u.hostname, "10.0.2.2");
    assert_eq!(u.host, "10.0.2.2:8080", "Host: carries the port");
    assert!(!u.use_tls);

    // No path at all is the root
    assert_eq!(ParsedUrl::parse("https://repo.miku").unwrap().path, "/");
}

#[test]
fn a_request_carries_the_headers_it_is_given() {
    let mut out = Vec::new();
    HttpRequest {
        method: "GET",
        path: "/core/index.mpki",
        host: "repo.miku",
        body: None,
        headers: &[("Range", "bytes=4096-")],
    }
    .write_to(&mut out);

    let text = String::from_utf8(out).unwrap();
    assert!(text.starts_with("GET /core/index.mpki HTTP/1.1\r\n"), "{text}");
    assert!(text.contains("Host: repo.miku\r\n"), "{text}");
    assert!(text.contains("Range: bytes=4096-\r\n"), "{text}");
    assert!(text.ends_with("\r\n\r\n"), "{text}");
}

// A body with a length

const WITH_LENGTH: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nContent-Type: application/octet-stream\r\n\r\nhello world";

#[test]
fn a_body_with_a_length_arrives_whole_however_it_is_cut() {
    for step in 1..=WITH_LENGTH.len() {
        let (sink, info) = decode(WITH_LENGTH, step, 1 << 20);
        let info = info.unwrap_or_else(|| panic!("step {step}: no response"));
        assert_eq!(info.status, 200, "step {step}");
        assert_eq!(info.reason, "OK", "step {step}");
        assert_eq!(info.content_length, Some(11), "step {step}");
        assert!(!info.chunked, "step {step}");
        assert!(!info.truncated, "step {step}");
        assert_eq!(info.received, 11, "step {step}");
        assert_eq!(sink.bytes, b"hello world", "step {step}");
    }
}

/// A body ending with the connection: the decoder cannot claim to be finished
#[test]
fn a_body_with_no_length_runs_to_the_close() {
    let response = b"HTTP/1.1 200 OK\r\n\r\nsome bytes with no length at all";
    for step in 1..=response.len() {
        let (sink, info) = decode(response, step, 1 << 20);
        let info = info.unwrap();
        assert_eq!(info.content_length, None, "step {step}");
        assert_eq!(sink.bytes, b"some bytes with no length at all", "step {step}");
    }
}

// Chunked

const CHUNKED: &[u8] = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
5\r\nhello\r\n1\r\n \r\n6\r\nworld!\r\n0\r\n\r\n";

/// A chunk boundary can fall anywhere in a segment
#[test]
fn a_chunked_body_arrives_whole_however_it_is_cut() {
    for step in 1..=CHUNKED.len() {
        let (sink, info) = decode(CHUNKED, step, 1 << 20);
        let info = info.unwrap_or_else(|| panic!("step {step}: no response"));
        assert!(info.chunked, "step {step}");
        assert_eq!(
            sink.bytes, b"hello world!",
            "step {step}: the pieces did not come back together"
        );
        assert_eq!(info.received, 12, "step {step}");
    }
}

/// A chunk size may have an extension and upper-case hex
#[test]
fn a_chunk_size_may_be_dressed_up() {
    let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
A;name=value\r\n0123456789\r\n0\r\n\r\n";
    for step in 1..=response.len() {
        let (sink, _) = decode(response, step, 1 << 20);
        assert_eq!(sink.bytes, b"0123456789", "step {step}");
    }
}

#[test]
fn a_trailer_after_the_last_chunk_is_not_body() {
    let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
4\r\nbody\r\n0\r\nX-Checksum: 1234\r\n\r\n";
    for step in 1..=response.len() {
        let (sink, _) = decode(response, step, 1 << 20);
        assert_eq!(sink.bytes, b"body", "step {step}");
    }
}

// Limits, and a refusing sink

/// An endless body in answer to a small request
#[test]
fn a_body_past_the_cap_is_cut_off_and_says_so() {
    let (sink, info) = decode(WITH_LENGTH, 64, 5);
    let info = info.unwrap();
    assert!(info.truncated, "it must say it stopped early");
    assert_eq!(sink.bytes, b"hello");
    assert_eq!(info.received, 5);

    // ...and the same for a chunked one, where the cap falls inside a chunk
    let (sink, info) = decode(CHUNKED, 3, 7);
    assert!(info.unwrap().truncated);
    assert_eq!(sink.bytes, b"hello w");
}

#[test]
fn a_sink_that_refuses_stops_the_transfer() {
    let mut d = Decoder::new(1 << 20);
    let mut sink = Sink { stop_after: Some(5), ..Sink::default() };
    let mut at = 0usize;
    let mut stopped = false;
    while at < WITH_LENGTH.len() {
        let end = (at + 4).min(WITH_LENGTH.len());
        if !d.feed(&WITH_LENGTH[at..end], &mut sink) {
            stopped = true;
            break;
        }
        at = end;
    }
    assert!(stopped, "the decoder should have given up");
    assert!(d.finish().unwrap().truncated);
}

// What it refuses

#[test]
fn a_redirect_is_reported_with_where_it_points() {
    let response = b"HTTP/1.1 302 Found\r\nLocation: https://elsewhere/core\r\nContent-Length: 0\r\n\r\n";
    for step in 1..=response.len() {
        let (_, info) = decode(response, step, 1 << 20);
        let info = info.unwrap();
        assert_eq!(info.status, 302, "step {step}");
        assert_eq!(info.location.as_deref(), Some("https://elsewhere/core"), "step {step}");
    }
}

/// A `Range` honored or ignored; a caller must tell them apart to avoid
/// appending the file twice
#[test]
fn a_partial_answer_is_told_apart_from_a_whole_one() {
    let partial = b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 5-10/11\r\nContent-Length: 6\r\n\r\n world";
    let (_, info) = decode(partial, 7, 1 << 20);
    let info = info.unwrap();
    assert!(info.partial, "206 means the range was honoured");
    assert_eq!(info.content_length, Some(6), "and the length is of the part");

    let (_, info) = decode(WITH_LENGTH, 7, 1 << 20);
    assert!(!info.unwrap().partial, "200 means it was not");
}

#[test]
fn something_that_is_not_a_response_is_refused() {
    let mut d = Decoder::new(1 << 20);
    let mut sink = Sink::default();
    assert!(!d.feed(b"this is not HTTP at all\r\n\r\n", &mut sink));
    assert!(d.finish().is_none());
}

/// A head that never ends
#[test]
fn an_endless_head_is_given_up_on() {
    let mut d = Decoder::new(1 << 20);
    let mut sink = Sink::default();
    let filler = vec![b'x'; 8192];
    let mut gave_up = false;
    for _ in 0..16 {
        if !d.feed(&filler, &mut sink) {
            gave_up = true;
            break;
        }
    }
    assert!(gave_up, "it must stop rather than grow");
    assert!(sink.bytes.is_empty(), "and nothing of it is a body");
}

/// A redirect's body must not reach the sink
#[test]
fn a_redirects_own_body_does_not_reach_the_sink() {
    let response = b"HTTP/1.1 302 Found\r\nLocation: https://elsewhere/core\r\nContent-Length: 44\r\n\r\n\
<html><body>Moved. Go away.</body></html>abc";
    for step in [1usize, 7, 64, response.len()] {
        let (sink, info) = decode(response, step, 1 << 20);
        let info = info.unwrap();
        assert_eq!(info.status, 302, "step {step}");
        assert_eq!(info.location.as_deref(), Some("https://elsewhere/core"));
        assert!(sink.bytes.is_empty(), "step {step}: {:?}", sink.bytes);
        assert_eq!(info.received, 0, "step {step}");
    }

    // A redirect with no location is an ordinary response, and its body is
    // delivered
    let dead_end = b"HTTP/1.1 302 Found\r\nContent-Length: 5\r\n\r\nhello";
    let (sink, _) = decode(dead_end, 3, 1 << 20);
    assert_eq!(sink.bytes, b"hello");
}
