//! Bytes go to disk and SHA-256 in one pass, so a package is not held in the
//! heap.
//!
//! Writes go through `Root::append` in batches, since each `append` is durable
//! and one per TLS record would mean an `fsync` per 16 KB.

use alloc::string::String;
use alloc::vec::Vec;

use mikunet::crypto::Sha256State;
use mikunet::http::wire::BodySink;

use crate::apply::root::{Root, RootError};

/// Bytes held before a write
pub const FLUSH_AT: usize = 256 * 1024;

pub struct FileSink<'a, R: Root + ?Sized> {
    root: &'a mut R,
    path: String,
    buf: Vec<u8>,
    hash: Sha256State,
    written: u64,
    failed: Option<RootError>,
}

impl<'a, R: Root + ?Sized> FileSink<'a, R> {
    /// Starts writing at the end of what is there. `resume_hash` is the hash of
    /// what is already written, or `None` to start from nothing
    pub fn new(root: &'a mut R, path: &str, already: u64, resume_hash: Sha256State) -> Self {
        FileSink {
            root,
            path: String::from(path),
            buf: Vec::with_capacity(FLUSH_AT),
            hash: resume_hash,
            written: already,
            failed: None,
        }
    }

    pub fn written(&self) -> u64 {
        self.written
    }

    /// Flushes the rest and returns the digest of everything through it. It
    /// covers the whole file only when given the earlier hash; otherwise the
    /// caller re-reads the file
    pub fn finish(mut self) -> Result<([u8; 32], u64), RootError> {
        if let Some(e) = self.failed.take() {
            return Err(e);
        }
        if !self.buf.is_empty() {
            let buf = core::mem::take(&mut self.buf);
            self.root.append(&self.path, &buf)?;
        }
        Ok((self.hash.finalize(), self.written))
    }
}

impl<R: Root + ?Sized> BodySink for FileSink<'_, R> {
    fn chunk(&mut self, bytes: &[u8]) -> bool {
        if self.failed.is_some() {
            return false;
        }
        self.hash.update(bytes);
        self.buf.extend_from_slice(bytes);
        self.written += bytes.len() as u64;
        if self.buf.len() >= FLUSH_AT {
            let buf = core::mem::take(&mut self.buf);
            if let Err(e) = self.root.append(&self.path, &buf) {
                // A disk refusing bytes ends the transfer
                self.failed = Some(e);
                return false;
            }
            self.buf = Vec::with_capacity(FLUSH_AT);
        }
        true
    }
}
