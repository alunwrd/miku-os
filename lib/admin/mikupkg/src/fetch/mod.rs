//! Fetches a named object, checks it, and lands it in the cache
//!
//! The transfer is behind a trait, so `object` can be tested with no network.
//! `net::Http` is the real implementation, `userspace`-gated.
//!
//! - A server may ignore a `Range` and answer 200 with the whole body. The
//!   answer says which, and this restarts when needed.
//! - A hash mismatch is not retried: it means a corrupt mirror or someone in
//!   the middle, and the file is deleted.

pub mod sink;

#[cfg(feature = "userspace")]
pub mod net;

use alloc::string::{String, ToString};
use core::fmt;

use mikucrypto::primitives::Sha256State;
use mikunet::http::wire::BodySink;

use crate::apply::root::{Root, RootError};
use crate::format::sha256;

/// What a caller wants, and what it knows about it in advance.
pub struct Wanted<'a> {
    pub url: &'a str,
    /// From the index. Without it, nothing can say whether the right file
    /// arrived
    pub sha256: Option<[u8; 32]>,
    pub size: Option<u64>,
    /// Where it lands, relative to the root
    pub dest: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fetched {
    /// Already present, and hashes to what the index says
    AlreadyHere,
    Downloaded { bytes: u64 },
    /// An interrupted download was continued
    Resumed { had: u64, bytes: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// Nothing came back at all
    Unreachable { url: String },
    Status { url: String, status: u16 },
    TooBig { url: String, limit: u64 },
    /// What arrived is not what the index says
    WrongHash { url: String },
    /// The server sent fewer bytes than it promised
    Short { url: String, had: u64, wanted: u64 },
    Root(RootError),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::Unreachable { url } => write!(f, "{url}: nothing came back"),
            FetchError::Status { url, status } => write!(f, "{url}: the server said {status}"),
            FetchError::TooBig { url, limit } => write!(
                f,
                "{url}: more than {limit} bytes. A server that answers a small request \
                 with an endless body is not one to keep reading"
            ),
            FetchError::WrongHash { url } => write!(
                f,
                "{url}: what arrived is not what the index says it is. Either a mirror \
                 is serving something else or somebody is in the way - it has not been \
                 kept"
            ),
            FetchError::Short { url, had, wanted } => {
                write!(f, "{url}: {had} bytes of a promised {wanted}")
            }
            FetchError::Root(e) => write!(f, "{e}"),
        }
    }
}

impl From<RootError> for FetchError {
    fn from(e: RootError) -> FetchError {
        FetchError::Root(e)
    }
}

/// What a transfer reported.
pub struct Answer {
    pub status: u16,
    /// The server honored a `Range`. A 200 to a ranged request means it did
    /// not, and earlier bytes are discarded
    pub partial: bool,
    pub truncated: bool,
}

/// How bytes are actually got. One real implementation, one in every test.
pub trait Transfer {
    fn get(
        &self,
        url: &str,
        from: Option<u64>,
        max_bytes: u64,
        sink: &mut dyn BodySink,
    ) -> Option<Answer>;
}

/// Fetches unless already here, resuming if possible
pub fn object<T: Transfer + ?Sized, R: Root + ?Sized>(
    want: &Wanted<'_>,
    transfer: &T,
    root: &mut R,
    max_bytes: u64,
) -> Result<Fetched, FetchError> {
    // Only the hash says whether it is already here
    if let (Some(expect), Some(_)) = (want.sha256, root.stat(want.dest)) {
        if let Ok(bytes) = root.read(want.dest) {
            if sha256(&bytes) == expect {
                return Ok(Fetched::AlreadyHere);
            }
        }
    }

    let part = alloc::format!("{}.part", want.dest);

    // Half-downloaded. Resuming needs the hash of what is there, so the file is
    // re-read
    let (already, state) = match root.stat(&part) {
        Some(_) if want.sha256.is_some() => match root.read(&part) {
            Ok(bytes) => {
                let mut h = Sha256State::new();
                h.update(&bytes);
                (bytes.len() as u64, h)
            }
            Err(_) => (0, Sha256State::new()),
        },
        _ => (0, Sha256State::new()),
    };

    let mut start_over = false;
    let (digest, total) = {
        let mut sink = sink::FileSink::new(root, &part, already, state);
        let answer = transfer
            .get(
                want.url,
                if already > 0 { Some(already) } else { None },
                max_bytes.saturating_sub(already),
                &mut sink,
            )
            .ok_or_else(|| FetchError::Unreachable { url: want.url.to_string() })?;

        if answer.status != 200 && answer.status != 206 {
            return Err(FetchError::Status {
                url: want.url.to_string(),
                status: answer.status,
            });
        }
        if answer.truncated {
            return Err(FetchError::TooBig { url: want.url.to_string(), limit: max_bytes });
        }
        // Asked to continue and got the whole thing: the disk copy would
        // precede a second copy
        if already > 0 && !answer.partial {
            start_over = true;
        }
        sink.finish()?
    };

    if start_over {
        root.remove_file(&part)?;
        let mut sink = sink::FileSink::new(root, &part, 0, Sha256State::new());
        let answer = transfer
            .get(want.url, None, max_bytes, &mut sink)
            .ok_or_else(|| FetchError::Unreachable { url: want.url.to_string() })?;
        if answer.status != 200 {
            return Err(FetchError::Status {
                url: want.url.to_string(),
                status: answer.status,
            });
        }
        if answer.truncated {
            return Err(FetchError::TooBig { url: want.url.to_string(), limit: max_bytes });
        }
        let (digest, total) = sink.finish()?;
        return settle(want, root, &part, digest, total, 0);
    }

    settle(want, root, &part, digest, total, already)
}

/// Check what arrived and put it where it belongs.
fn settle<R: Root + ?Sized>(
    want: &Wanted<'_>,
    root: &mut R,
    part: &str,
    digest: [u8; 32],
    total: u64,
    already: u64,
) -> Result<Fetched, FetchError> {
    if let Some(size) = want.size {
        if total < size {
            // Kept: fewer bytes than promised is an interrupted transfer, and
            // the next attempt continues
            return Err(FetchError::Short {
                url: want.url.to_string(),
                had: total,
                wanted: size,
            });
        }
    }
    if let Some(expect) = want.sha256 {
        if digest != expect {
            // No retry: resuming onto these bytes could never hash right
            let _ = root.remove_file(part);
            return Err(FetchError::WrongHash { url: want.url.to_string() });
        }
    }

    root.rename(part, want.dest)?;
    Ok(if already > 0 {
        Fetched::Resumed { had: already, bytes: total - already }
    } else {
        Fetched::Downloaded { bytes: total }
    })
}
