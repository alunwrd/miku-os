// Downloading, resuming, and refusing, with no network. The logic sits behind a
// trait

use mikupkg::apply::mem::MemRoot;
use mikupkg::apply::root::Root;
use mikupkg::fetch::{object, Answer, FetchError, Fetched, Transfer, Wanted};
use mikupkg::format::sha256;
use mikunet::http::wire::BodySink;

/// A written-out server. `answers` is what it does next
struct Server {
    body: Vec<u8>,
    /// Ignores `Range` and sends everything, as a server may
    ignores_range: bool,
    /// Stop after this many bytes, as a connection that drops does
    cuts_off_at: Option<usize>,
    status: u16,
}

impl Server {
    fn new(body: &[u8]) -> Server {
        Server {
            body: body.to_vec(),
            ignores_range: false,
            cuts_off_at: None,
            status: 200,
        }
    }
}

impl Transfer for Server {
    fn get(
        &self,
        _url: &str,
        from: Option<u64>,
        max_bytes: u64,
        sink: &mut dyn BodySink,
    ) -> Option<Answer> {
        if self.status != 200 {
            return Some(Answer { status: self.status, partial: false, truncated: false });
        }
        let honours = from.is_some() && !self.ignores_range;
        let start = if honours { from.unwrap_or(0) as usize } else { 0 };
        let mut body = &self.body[start.min(self.body.len())..];
        if let Some(cut) = self.cuts_off_at {
            body = &body[..cut.min(body.len())];
        }
        let mut truncated = false;
        let take = (body.len() as u64).min(max_bytes) as usize;
        if take < body.len() {
            truncated = true;
        }
        if take > 0 {
            sink.chunk(&body[..take]);
        }
        Some(Answer {
            status: if honours { 206 } else { 200 },
            partial: honours,
            truncated,
        })
    }
}

fn root() -> MemRoot {
    let mut r = MemRoot::new();
    r.preset_dir("pkg", 0o755);
    r.preset_dir("pkg/cache", 0o755);
    r
}

const BODY: &[u8] = b"a package, or near enough for a test to tell whether it arrived";

fn wanted<'a>(url: &'a str, dest: &'a str) -> Wanted<'a> {
    Wanted { url, sha256: Some(sha256(BODY)), size: Some(BODY.len() as u64), dest }
}

#[test]
fn a_file_that_is_not_here_is_downloaded() {
    let mut r = root();
    let got = object(
        &wanted("https://repo/x.mpk", "pkg/cache/x.mpk"),
        &Server::new(BODY),
        &mut r,
        1 << 20,
    )
    .expect("it should download");

    assert_eq!(got, Fetched::Downloaded { bytes: BODY.len() as u64 });
    assert_eq!(r.read("pkg/cache/x.mpk").unwrap(), BODY);
    assert!(r.stat("pkg/cache/x.mpk.part").is_none(), "the part file goes");
}

/// Right length is not the right file; only the hash says "already here"
#[test]
fn something_already_here_is_not_downloaded_again() {
    let mut r = root();
    r.preset_file("pkg/cache/x.mpk", BODY, 0o644, 0, 0);

    let got = object(
        &wanted("https://repo/x.mpk", "pkg/cache/x.mpk"),
        &Server::new(BODY),
        &mut r,
        1 << 20,
    )
    .unwrap();
    assert_eq!(got, Fetched::AlreadyHere);

    // ...and something else of the same length is downloaded again
    let wrong = vec![b'z'; BODY.len()];
    r.preset_file("pkg/cache/x.mpk", &wrong, 0o644, 0, 0);
    let got = object(
        &wanted("https://repo/x.mpk", "pkg/cache/x.mpk"),
        &Server::new(BODY),
        &mut r,
        1 << 20,
    )
    .unwrap();
    assert_eq!(got, Fetched::Downloaded { bytes: BODY.len() as u64 });
    assert_eq!(r.read("pkg/cache/x.mpk").unwrap(), BODY);
}

#[test]
fn an_interrupted_download_is_continued_rather_than_repeated() {
    let mut r = root();
    // What a connection that dropped left behind
    r.preset_file("pkg/cache/x.mpk.part", &BODY[..20], 0o644, 0, 0);

    let got = object(
        &wanted("https://repo/x.mpk", "pkg/cache/x.mpk"),
        &Server::new(BODY),
        &mut r,
        1 << 20,
    )
    .expect("it should resume");

    assert_eq!(got, Fetched::Resumed { had: 20, bytes: (BODY.len() - 20) as u64 });
    assert_eq!(r.read("pkg/cache/x.mpk").unwrap(), BODY);
}

/// A server may ignore a `Range` and answer 200 with the whole body, which must
/// not be appended
#[test]
fn a_server_that_ignores_the_range_makes_it_start_over() {
    let mut r = root();
    r.preset_file("pkg/cache/x.mpk.part", &BODY[..20], 0o644, 0, 0);

    let mut server = Server::new(BODY);
    server.ignores_range = true;

    let got = object(
        &wanted("https://repo/x.mpk", "pkg/cache/x.mpk"),
        &server,
        &mut r,
        1 << 20,
    )
    .expect("it should start again");

    assert_eq!(got, Fetched::Downloaded { bytes: BODY.len() as u64 });
    assert_eq!(
        r.read("pkg/cache/x.mpk").unwrap(),
        BODY,
        "not twenty bytes followed by the whole file"
    );
}

/// No retry: resuming onto these bytes could never hash right
#[test]
fn a_body_that_does_not_hash_right_is_refused_and_not_kept() {
    let mut r = root();
    // Same length as asked for, so the length check cannot catch it. A merely
    // short body is a different case, kept for the next attempt
    let impostor: Vec<u8> = BODY.iter().map(|b| b ^ 0x20).collect();
    assert_eq!(impostor.len(), BODY.len());

    let err = object(
        &wanted("https://repo/x.mpk", "pkg/cache/x.mpk"),
        &Server::new(&impostor),
        &mut r,
        1 << 20,
    )
    .unwrap_err();

    assert!(matches!(err, FetchError::WrongHash { .. }), "{err}");
    assert!(r.stat("pkg/cache/x.mpk").is_none());
    assert!(
        r.stat("pkg/cache/x.mpk.part").is_none(),
        "it must not be left for a resume that would never converge"
    );
    let said = std::format!("{err}");
    assert!(said.contains("not what the index says"), "{said}");
}

/// A transfer that stopped early is kept for the next attempt
#[test]
fn a_transfer_that_stopped_early_is_kept_for_next_time() {
    let mut r = root();
    let mut server = Server::new(BODY);
    server.cuts_off_at = Some(20);

    let err = object(
        &wanted("https://repo/x.mpk", "pkg/cache/x.mpk"),
        &server,
        &mut r,
        1 << 20,
    )
    .unwrap_err();
    assert!(matches!(err, FetchError::Short { had: 20, .. }), "{err}");
    assert_eq!(r.read("pkg/cache/x.mpk.part").unwrap(), &BODY[..20]);

    // ...and the next attempt, against a server that is behaving, finishes it
    let got = object(
        &wanted("https://repo/x.mpk", "pkg/cache/x.mpk"),
        &Server::new(BODY),
        &mut r,
        1 << 20,
    )
    .unwrap();
    assert_eq!(got, Fetched::Resumed { had: 20, bytes: (BODY.len() - 20) as u64 });
    assert_eq!(r.read("pkg/cache/x.mpk").unwrap(), BODY);
}

#[test]
fn a_body_past_the_cap_is_refused() {
    let mut r = root();
    let err = object(
        &wanted("https://repo/x.mpk", "pkg/cache/x.mpk"),
        &Server::new(BODY),
        &mut r,
        8,
    )
    .unwrap_err();
    assert!(matches!(err, FetchError::TooBig { .. }), "{err}");
    let said = std::format!("{err}");
    assert!(said.contains("endless body"), "{said}");
}

#[test]
fn what_the_server_says_when_it_says_no() {
    let mut r = root();
    let mut server = Server::new(BODY);
    server.status = 404;

    let err = object(
        &wanted("https://repo/x.mpk", "pkg/cache/x.mpk"),
        &server,
        &mut r,
        1 << 20,
    )
    .unwrap_err();
    assert_eq!(
        err,
        FetchError::Status { url: "https://repo/x.mpk".into(), status: 404 }
    );
}

/// Nothing known in advance, as for a local file by name. It is downloaded
/// unchecked; the signature covers it
#[test]
fn an_object_with_no_hash_is_fetched_and_not_judged() {
    let mut r = root();
    let got = object(
        &Wanted { url: "https://repo/x", sha256: None, size: None, dest: "pkg/cache/x" },
        &Server::new(BODY),
        &mut r,
        1 << 20,
    )
    .unwrap();
    assert_eq!(got, Fetched::Downloaded { bytes: BODY.len() as u64 });
    assert_eq!(r.read("pkg/cache/x").unwrap(), BODY);
}

/// A download bigger than the sink's buffer has to come out in one piece
#[test]
fn a_big_body_survives_being_flushed_in_batches() {
    use mikupkg::fetch::sink::FLUSH_AT;

    let big: Vec<u8> = (0..(FLUSH_AT * 2 + 1234)).map(|i| (i % 251) as u8).collect();
    let mut r = root();
    let want = Wanted {
        url: "https://repo/big",
        sha256: Some(sha256(&big)),
        size: Some(big.len() as u64),
        dest: "pkg/cache/big",
    };
    let got = object(&want, &Server::new(&big), &mut r, 1 << 24).unwrap();
    assert_eq!(got, Fetched::Downloaded { bytes: big.len() as u64 });
    assert_eq!(r.read("pkg/cache/big").unwrap(), big);
}
