//! Builds a `.mpk`
//!
//! Every rule about package contents (path limits, the `conf.d` prohibition,
//! the 64-byte symlink target) is enforced here, on the build host. The reader
//! checks them again.
//!
//! Output is deterministic: entries sorted by path, metadata in tag order,
//! payload after the table. A package rebuilt from a clean tree verifies
//! against the first signature.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use super::container::{FLAG_HAS_PAYLOAD, FORMAT_VERSION, HEADER_LEN, MAX_FILES};
use super::meta as meta_codec;
use super::table::{ENTRY_DEFAULT_CONFIG, CODEC_NONE, MAX_SYMLINK_TARGET};
use super::{sha256, FileEntry, FileKind, FileTable, Header};
use crate::meta::path::{self, PathError};
use crate::meta::PackageMeta;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// A path this package may not own, and why
    Path { path: String, why: PathError },
    /// Two entries claiming one path
    Duplicate { path: String },
    /// A symlink target longer than the kernel stores
    SymlinkTooLong { path: String, len: usize },
    /// A file bigger than the format's per-file field
    FileTooLarge { path: String, len: usize },
    TooManyFiles { count: usize },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::Path { path, why } => write!(f, "{path}: {why}"),
            BuildError::Duplicate { path } => {
                write!(f, "{path}: two entries claim this path")
            }
            BuildError::SymlinkTooLong { path, len } => write!(
                f,
                "{path}: the link target is {len} bytes and MikuOS stores {MAX_SYMLINK_TARGET} \
                 of one inline on the vnode - a longer one would be installed truncated, \
                 pointing somewhere else"
            ),
            BuildError::FileTooLarge { path, len } => {
                write!(f, "{path}: {len} bytes; a file in a package may be 4 GiB at most")
            }
            BuildError::TooManyFiles { count } => {
                write!(f, "{count} files; a package may hold {MAX_FILES} at most")
            }
        }
    }
}

/// Collect entries, then emit a container.
pub struct Builder {
    meta: PackageMeta,
    entries: Vec<(FileEntry, Vec<u8>)>,
}

impl Builder {
    pub fn new(meta: PackageMeta) -> Builder {
        Builder { meta, entries: Vec::new() }
    }

    pub fn add_file(
        &mut self,
        path: &str,
        mode: u16,
        uid: u16,
        gid: u16,
        content: &[u8],
    ) -> Result<(), BuildError> {
        self.push(path, FileKind::File, mode, uid, gid, content.to_vec())
    }

    pub fn add_dir(
        &mut self,
        path: &str,
        mode: u16,
        uid: u16,
        gid: u16,
    ) -> Result<(), BuildError> {
        self.push(path, FileKind::Directory, mode, uid, gid, Vec::new())
    }

    pub fn add_symlink(
        &mut self,
        path: &str,
        target: &str,
        uid: u16,
        gid: u16,
    ) -> Result<(), BuildError> {
        if target.len() > MAX_SYMLINK_TARGET {
            return Err(BuildError::SymlinkTooLong {
                path: String::from(path),
                len: target.len(),
            });
        }
        self.push(path, FileKind::Symlink, 0o777, uid, gid, target.as_bytes().to_vec())
    }

    fn push(
        &mut self,
        path: &str,
        kind: FileKind,
        mode: u16,
        uid: u16,
        gid: u16,
        content: Vec<u8>,
    ) -> Result<(), BuildError> {
        let checked = if kind == FileKind::Directory {
            path::check_dir(path)
        } else {
            path::check(path)
        };
        checked.map_err(|why| BuildError::Path {
            path: String::from(path),
            why,
        })?;
        if self.entries.iter().any(|(e, _)| e.path == path) {
            return Err(BuildError::Duplicate { path: String::from(path) });
        }
        if content.len() > u32::MAX as usize {
            return Err(BuildError::FileTooLarge {
                path: String::from(path),
                len: content.len(),
            });
        }
        if self.entries.len() >= MAX_FILES as usize {
            return Err(BuildError::TooManyFiles { count: self.entries.len() + 1 });
        }

        // A file under `default.d` may be replaced by the machine's owner, so
        // `verify` later says "changed" instead of "damaged"
        let flags = if is_shipped_default(path) { ENTRY_DEFAULT_CONFIG } else { 0 };

        self.entries.push((
            FileEntry {
                path: String::from(path),
                kind,
                codec: CODEC_NONE,
                mode,
                uid,
                gid,
                size: content.len() as u32,
                stored: content.len() as u32,
                payload_rel: 0, // assigned in `finish`, once the order is fixed
                sha256: sha256(&content),
                flags,
            },
            content,
        ));
        Ok(())
    }

    /// Emits the container. `signature` is a detached signature over the
    /// manifest hash, or nothing for an unsigned package
    pub fn finish(mut self, signature: Option<&[u8]>) -> Result<Vec<u8>, BuildError> {
        // Sorted by path, so output does not depend on directory walk order
        self.entries.sort_by(|a, b| a.0.path.cmp(&b.0.path));

        let mut payload: Vec<u8> = Vec::new();
        let mut entries: Vec<FileEntry> = Vec::with_capacity(self.entries.len());
        for (mut e, content) in self.entries {
            e.payload_rel = payload.len() as u64;
            payload.extend_from_slice(&content);
            entries.push(e);
        }
        let installed_size: u64 = entries
            .iter()
            .filter(|e| e.kind == FileKind::File)
            .map(|e| e.size as u64)
            .sum();

        let mut meta = self.meta;
        meta.installed_size = installed_size;

        let table = FileTable { entries };
        let meta_bytes = meta_codec::encode(&meta);
        let table_bytes = table.encode();
        let sig = signature.unwrap_or(&[]);

        let meta_off = HEADER_LEN as u32;
        let table_off = meta_off + meta_bytes.len() as u32;
        let sig_off = table_off + table_bytes.len() as u32;
        let payload_off = (sig_off as u64) + sig.len() as u64;

        let header = Header {
            format_version: FORMAT_VERSION,
            flags: if payload.is_empty() { 0 } else { FLAG_HAS_PAYLOAD },
            meta_off,
            meta_len: meta_bytes.len() as u32,
            table_off,
            table_len: table_bytes.len() as u32,
            // An absent section is a zero length at offset zero
            sig_off: if sig.is_empty() { 0 } else { sig_off },
            sig_len: sig.len() as u32,
            payload_off: if payload.is_empty() { 0 } else { payload_off },
            payload_len: payload.len() as u64,
            file_count: table.entries.len() as u32,
        };

        let mut out = Vec::with_capacity(payload_off as usize + payload.len());
        out.extend_from_slice(&header.encode());
        out.extend_from_slice(&meta_bytes);
        out.extend_from_slice(&table_bytes);
        out.extend_from_slice(sig);
        out.extend_from_slice(&payload);
        Ok(out)
    }

    /// Manifest hash of what `finish` would produce, so a signer can sign
    /// before the container exists. Builds the sections twice, to sign exactly
    /// what ships
    pub fn manifest_hash(&self) -> [u8; 32] {
        let mut clone = Builder {
            meta: self.meta.clone(),
            entries: self.entries.clone(),
        };
        clone.entries.sort_by(|a, b| a.0.path.cmp(&b.0.path));
        let mut payload_at = 0u64;
        let mut entries = Vec::with_capacity(clone.entries.len());
        for (mut e, content) in clone.entries {
            e.payload_rel = payload_at;
            payload_at += content.len() as u64;
            entries.push(e);
        }
        let installed_size: u64 = entries
            .iter()
            .filter(|e| e.kind == FileKind::File)
            .map(|e| e.size as u64)
            .sum();
        let mut meta = clone.meta;
        meta.installed_size = installed_size;
        super::manifest_hash(
            &meta_codec::encode(&meta),
            &FileTable { entries }.encode(),
        )
    }
}

/// Is this path under a directory the build ships and rewrites?
fn is_shipped_default(path: &str) -> bool {
    path.split('/').any(|c| c == "default.d")
}
