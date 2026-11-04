// What every lit head is doing, the one record of it
//
// The driver's table (`gsp_common::output::Output`) keeps what is hardware:
// which head, which SOR, the output LUTs, the surface. This keeps what the head
// was asked to be (`dev/kms-architecture.md`, K2).
//
// Who writes
//
// Whoever made the hardware take a change, right after it did: the driver at
// boot, after lighting a head, after the update, after variable refresh came up
// or down. The layer orders and reads. `set` is the one place a head changes.
//
// The lock
//
// A spin lock held for a copy, never across an RM call, the same rule as
// `gsp_common::output`.

use alloc::vec::Vec;
use spin::Mutex;

use super::{HeadColour, Timing};

/// One lit output's head, as it was last made to be
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Record {
    /// The driver's key for the output (a display id on NVIDIA)
    pub key: u32,
    /// The timing it rasters
    pub mode: Timing,
    /// `None` on a display with no colour pipeline
    pub colour: Option<HeadColour>,
    /// Told to sleep: no picture reaches the monitor
    pub asleep: bool,
    /// Variable refresh: the timeout in microseconds while it is on
    pub vrr: Option<u32>,
}

static HEADS: Mutex<Vec<Record>> = Mutex::new(Vec::new());

/// The record for `key`, if a head drives it
pub fn get(key: u32) -> Option<Record> {
    HEADS.lock().iter().find(|r| r.key == key).copied()
}

/// Every record, for a driver asking about the heads it is not changing
pub fn all() -> Vec<Record> {
    HEADS.lock().clone()
}

/// Write `key`'s record whole - a head just lit, or one the boot found
pub fn set(r: Record) {
    let mut t = HEADS.lock();
    match t.iter_mut().find(|x| x.key == r.key) {
        Some(x) => *x = r,
        None => t.push(r),
    }
}

/// Change part of `key`'s record. Nothing happens to a key with no record: a
/// change to a head nobody lit is a caller bug
pub fn update(key: u32, f: impl FnOnce(&mut Record)) -> bool {
    match HEADS.lock().iter_mut().find(|x| x.key == key) {
        Some(x) => {
            f(x);
            true
        }
        None => false,
    }
}

/// Every record gone: the driver's table is being rebuilt (enumeration) or torn
/// down (unload)
pub fn clear() {
    HEADS.lock().clear();
}
