//! Write-ahead record for crash recovery
//!
//! Per-file atomicity comes from `rename`; transaction atomicity from this
//! file. There is no `renameat2(RENAME_EXCHANGE)`, so an interrupted
//! transaction has changed some paths, and the journal describes them so they
//! can be undone.
//!
//! Every record has its length and a four-byte check, since the last one may be
//! half-written. A torn tail must be detected, or a reader would undo an
//! operation that never happened.
//!
//! The check is the front of a SHA-256, the machine's one hash.

use alloc::string::String;
use alloc::vec::Vec;

use super::{put_u16, put_u32, u16_at, u32_at, u64_at};

pub const KIND_BEGIN: u16 = 1;
pub const KIND_STAGED: u16 = 2;
pub const KIND_BACKUP: u16 = 3;
pub const KIND_CREATED: u16 = 4;
pub const KIND_PLACED: u16 = 5;
pub const KIND_REMOVED: u16 = 6;
pub const KIND_REMOVED_DIR: u16 = 8;
pub const KIND_COMMIT: u16 = 7;

const FRAME_HEAD: usize = 4;
const FRAME_TAIL: usize = 4;
/// Largest record payload
pub const MAX_PAYLOAD: usize = 2048;

/// What happened, in the order it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    /// A transaction starts; nothing earlier belongs to it
    Begin { serial: u64, time: u64, ops: u32 },
    /// New content at `staged`, not yet in place
    Staged { dest: String, staged: String },
    /// The old `dest` is also at `backup`
    Backup { dest: String, backup: String },
    /// Nothing was at `dest` before this transaction
    Created { dest: String },
    /// `dest` holds the new content
    Placed { dest: String },
    /// `dest` was removed; its content is at `backup`
    Removed { dest: String, backup: String },
    /// An empty directory was removed. Records how to make it again
    RemovedDir { dest: String, mode: u16, uid: u16, gid: u16 },
    /// Everything above it happened, and stays
    Commit { serial: u64 },
}

impl Record {
    pub fn encode(&self) -> Vec<u8> {
        let (kind, payload) = match self {
            Record::Begin { serial, time, ops } => {
                let mut p = Vec::with_capacity(20);
                p.extend_from_slice(&serial.to_le_bytes());
                p.extend_from_slice(&time.to_le_bytes());
                p.extend_from_slice(&ops.to_le_bytes());
                (KIND_BEGIN, p)
            }
            Record::Staged { dest, staged } => (KIND_STAGED, two(dest, staged)),
            Record::Backup { dest, backup } => (KIND_BACKUP, two(dest, backup)),
            Record::Created { dest } => (KIND_CREATED, one(dest)),
            Record::Placed { dest } => (KIND_PLACED, one(dest)),
            Record::Removed { dest, backup } => (KIND_REMOVED, two(dest, backup)),
            Record::RemovedDir { dest, mode, uid, gid } => {
                let mut p = one(dest);
                p.extend_from_slice(&mode.to_le_bytes());
                p.extend_from_slice(&uid.to_le_bytes());
                p.extend_from_slice(&gid.to_le_bytes());
                (KIND_REMOVED_DIR, p)
            }
            Record::Commit { serial } => (KIND_COMMIT, serial.to_le_bytes().to_vec()),
        };

        let mut out = alloc::vec![0u8; FRAME_HEAD + payload.len() + FRAME_TAIL];
        put_u16(&mut out, 0, kind);
        put_u16(&mut out, 2, payload.len() as u16);
        out[FRAME_HEAD..FRAME_HEAD + payload.len()].copy_from_slice(&payload);
        let check = check_of(&out[..FRAME_HEAD + payload.len()]);
        put_u32(&mut out, FRAME_HEAD + payload.len(), check);
        out
    }
}

fn one(a: &str) -> Vec<u8> {
    let mut p = Vec::with_capacity(2 + a.len());
    p.extend_from_slice(&(a.len() as u16).to_le_bytes());
    p.extend_from_slice(a.as_bytes());
    p
}

fn two(a: &str, b: &str) -> Vec<u8> {
    let mut p = one(a);
    p.extend_from_slice(&(b.len() as u16).to_le_bytes());
    p.extend_from_slice(b.as_bytes());
    p
}

fn check_of(bytes: &[u8]) -> u32 {
    let d = super::sha256(bytes);
    u32::from_le_bytes([d[0], d[1], d[2], d[3]])
}

fn take_str(p: &[u8], at: &mut usize) -> Option<String> {
    let len = u16_at(p, *at)? as usize;
    let start = *at + 2;
    let s = core::str::from_utf8(p.get(start..start + len)?).ok()?;
    *at = start + len;
    Some(String::from(s))
}

/// What a journal file held, and whether all of it was there
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Journal {
    pub records: Vec<Record>,
    /// The file ended mid-record, or one failed its check. Expected after a
    /// power loss; earlier records are still good
    pub torn: bool,
}

impl Journal {
    /// Read as far as the records are sound, and say where they stopped.
    pub fn parse(b: &[u8]) -> Journal {
        let mut records = Vec::new();
        let mut at = 0usize;
        loop {
            if at == b.len() {
                return Journal { records, torn: false };
            }
            if at + FRAME_HEAD > b.len() {
                return Journal { records, torn: true };
            }
            let kind = u16_at(b, at).unwrap();
            let len = u16_at(b, at + 2).unwrap() as usize;
            if len > MAX_PAYLOAD {
                return Journal { records, torn: true };
            }
            let end = at + FRAME_HEAD + len + FRAME_TAIL;
            if end > b.len() {
                return Journal { records, torn: true };
            }
            let stored = u32_at(b, at + FRAME_HEAD + len).unwrap();
            if stored != check_of(&b[at..at + FRAME_HEAD + len]) {
                return Journal { records, torn: true };
            }

            let p = &b[at + FRAME_HEAD..at + FRAME_HEAD + len];
            let rec = match decode(kind, p) {
                Some(r) => r,
                // A correctly framed record not understood here counts as torn;
                // continuing would undo operations in the wrong order
                None => return Journal { records, torn: true },
            };
            records.push(rec);
            at = end;
        }
    }

    /// Whether the transaction finished: the last `Commit` decides, since a
    /// file may hold a finished transaction followed by an interrupted one
    pub fn committed(&self) -> bool {
        matches!(self.records.last(), Some(Record::Commit { .. }))
    }

    /// Records of the last transaction in the file, committed or not
    pub fn last_transaction(&self) -> &[Record] {
        let start = self
            .records
            .iter()
            .rposition(|r| matches!(r, Record::Begin { .. }))
            .unwrap_or(0);
        &self.records[start..]
    }
}

fn decode(kind: u16, p: &[u8]) -> Option<Record> {
    let mut at = 0usize;
    Some(match kind {
        KIND_BEGIN => Record::Begin {
            serial: u64_at(p, 0)?,
            time: u64_at(p, 8)?,
            ops: u32_at(p, 16)?,
        },
        KIND_STAGED => Record::Staged {
            dest: take_str(p, &mut at)?,
            staged: take_str(p, &mut at)?,
        },
        KIND_BACKUP => Record::Backup {
            dest: take_str(p, &mut at)?,
            backup: take_str(p, &mut at)?,
        },
        KIND_CREATED => Record::Created { dest: take_str(p, &mut at)? },
        KIND_PLACED => Record::Placed { dest: take_str(p, &mut at)? },
        KIND_REMOVED => Record::Removed {
            dest: take_str(p, &mut at)?,
            backup: take_str(p, &mut at)?,
        },
        KIND_REMOVED_DIR => {
            let dest = take_str(p, &mut at)?;
            Record::RemovedDir {
                dest,
                mode: u16_at(p, at)?,
                uid: u16_at(p, at + 2)?,
                gid: u16_at(p, at + 4)?,
            }
        }
        KIND_COMMIT => Record::Commit { serial: u64_at(p, 0)? },
        _ => return None,
    })
}
