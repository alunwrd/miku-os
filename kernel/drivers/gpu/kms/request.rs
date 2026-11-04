// A change to the displays, asked for as a whole
//
// Built as DRM builds an atomic state: start from what the output is doing and
// change what was asked. A request to turn variable refresh on is a full head
// state with one field different, so a check sees mode, depth, sleep and
// refresh together (`NvKmsSetModeOneHeadRequest`, `nvkms-api.h`, has the same
// shape).

use alloc::string::String;
use alloc::vec::Vec;

use super::{HeadColour, Output, Vrr};

/// One head's whole state, as wanted
#[derive(Clone)]
pub struct HeadRequest {
    /// The output, by name
    pub output: String,
    /// A head drives it, lighting an output nothing drives
    pub lit: bool,
    /// Into the monitor's mode list; `None` keeps whatever is running
    pub mode: Option<usize>,
    pub asleep: bool,
    pub vrr: bool,
    /// `None` on a display with no colour pipeline
    pub colour: Option<HeadColour>,
}

impl HeadRequest {
    /// What `o` is doing now, as a request that would change nothing
    pub fn current(o: &Output) -> HeadRequest {
        match &o.head {
            Some(h) => HeadRequest {
                output: o.name.clone(),
                lit: true,
                mode: h.mode,
                asleep: h.asleep,
                vrr: matches!(h.vrr, Vrr::On { .. }),
                colour: h.colour,
            },
            None => HeadRequest {
                output: o.name.clone(),
                lit: false,
                mode: None,
                asleep: false,
                vrr: false,
                colour: None,
            },
        }
    }
}

/// Every head the change touches, and whether it is only to be checked
pub struct Request {
    pub heads: Vec<HeadRequest>,
    /// Check and stop: nothing on the screen changes (DRM's TEST_ONLY,
    /// NVKMS's `commit = FALSE`)
    pub test_only: bool,
}

impl Request {
    pub fn one(head: HeadRequest, test_only: bool) -> Request {
        Request { heads: alloc::vec![head], test_only }
    }
}
