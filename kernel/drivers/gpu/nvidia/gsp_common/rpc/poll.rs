// Every message goes through one place
//
// `_kgspRpcRecvPoll` (`kernel_gsp.c:2062`) built on `_kgspRpcDrainEvents`
// (`:1764`) and `_kgspRpcDrainOneEvent` (`:1714`): read the ring until the
// message waited for turns up, handing everything else to the event dispatcher
// on the way.
//
// The reader decides nothing about what an event means; that is the
// dispatcher's (`kgsp::events` on the card, a closure in the selftest). A
// dispatcher records and returns. It never makes an RPC, and a poll started
// from inside one is refused; see `NestedPoll`.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::fns::FN_CONTINUATION_RECORD;
use super::queue::{tsc_elapsed_ns, GspRpc, QueueMem, RpcMessage};
use super::wait::wait_for_msgq;
use super::RpcError;

/// Why a message is being read (`KGSP_RPC_EVENT_HANDLER_CONTEXT_*`)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    /// Waiting for the reply to an RPC this driver sent
    Poll,
    /// Waiting for GSP_INIT's answer, when only the bootup events may be
    /// handled
    PollBootup,
    /// Waiting for nothing: draining what arrived on its own
    Interrupt,
}

/// A function no message carries, for a drain waiting for nothing;
/// `kgspRpcRecvEvents` passes `NV_VGPU_MSG_FUNCTION_NUM_FUNCTIONS` the same way
pub const NOTHING: u32 = u32::MAX;

/// What handles the events a poll reads on its way to the reply
pub type Dispatch<'d> = &'d mut dyn FnMut(&RpcMessage, Context);

/// Set while a poll is waiting for a reply (`bPollingForRpcResponse`)
static POLLING: AtomicBool = AtomicBool::new(false);

/// Held for the length of a poll. OpenRM asserts instead of refusing
/// (`kernel_gsp.c:2105`): a handler sending RPC-B while RPC-A is outstanding
/// gets A's reply while waiting for B's
struct Polling;

impl Polling {
    fn enter() -> Result<Self, RpcError> {
        if POLLING.swap(true, Ordering::Acquire) {
            return Err(RpcError::NestedPoll);
        }
        Ok(Polling)
    }
}

impl Drop for Polling {
    fn drop(&mut self) {
        POLLING.store(false, Ordering::Release);
    }
}

/// The call a reply is being waited for, `NOTHING` between calls, so an event
/// drained meanwhile can say what was in flight (`dev/klog.md`). GSP-RM posts a
/// NOCAT assert or a robust-channel report about the request it handles. One
/// slot: RPCs go one at a time, under the RM lock, and a nested poll is refused
static IN_FLIGHT_FN: AtomicU32 = AtomicU32::new(NOTHING);
/// For RM's three calls, what the call is about: the command of an
/// `RM_CONTROL`, the class of an `RM_ALLOC`, the handle a `FREE` releases
static IN_FLIGHT_ARG: AtomicU32 = AtomicU32::new(0);
/// The call before it, finished. An event GSP-RM posts on its own (a NOCAT
/// record from its display supervisor, say) is read during the next call, so
/// the call in flight is where it was seen; the one before bounds when it was
/// made
static LAST_FN: AtomicU32 = AtomicU32::new(NOTHING);
static LAST_ARG: AtomicU32 = AtomicU32::new(0);

/// From the request itself: `rpc_gsp_rm_control_v03_00` has `cmd` at 8,
/// `rpc_gsp_rm_alloc_v03_00` `hClass` at 12, `NVOS00_PARAMETERS` `hObjectOld`
/// at 8
fn call_arg(function: u32, payload: &[u8]) -> u32 {
    use crate::drivers::gpu::nvidia::gsp_common::rmapi::{FN_FREE, FN_GSP_RM_ALLOC, FN_GSP_RM_CONTROL};
    let at = match function {
        FN_GSP_RM_CONTROL | FN_FREE => 8,
        FN_GSP_RM_ALLOC => 12,
        _ => return 0,
    };
    crate::drivers::gpu::nvidia::le::u32_or0(payload, at)
}

/// Where an event was read, for its log line: `read while waiting on GSP_RM_CONTROL NVC372_CTRL_CMD_IS_MODE_POSSIBLE (0xc3720101), after GSP_RM_CONTROL NV0073_CTRL_CMD_DP_MAIN_LINK_CTRL (0x00731356)`, `between calls` when the event thread drained it, `during boot` while GSP-RM was coming up. GSP-RM made it between the two calls, not necessarily because of the first
pub fn read_during(ctx: Context) -> alloc::string::String {
    use crate::drivers::gpu::nvidia::gsp_common::rmapi::{class_str, ctrl_str, FN_FREE, FN_GSP_RM_ALLOC, FN_GSP_RM_CONTROL};
    let f = IN_FLIGHT_FN.load(Ordering::Acquire);
    match ctx {
        Context::PollBootup => return alloc::string::String::from("during boot"),
        Context::Interrupt => return alloc::string::String::from("between calls"),
        Context::Poll if f == NOTHING => return alloc::string::String::from("between calls"),
        Context::Poll => {}
    }
    let call = |f: u32, arg: u32| {
        let name = crate::drivers::gpu::nvidia::gsp_common::rm_names::function(f);
        match f {
            FN_GSP_RM_CONTROL => alloc::format!("{} {}", name, ctrl_str(arg)),
            FN_GSP_RM_ALLOC => alloc::format!("{} {}", name, class_str(arg)),
            FN_FREE => alloc::format!("{} {:#x}", name, arg),
            _ => alloc::format!("{}", name),
        }
    };
    let now = call(f, IN_FLIGHT_ARG.load(Ordering::Relaxed));
    match LAST_FN.load(Ordering::Acquire) {
        NOTHING => alloc::format!("read while waiting on {}", now),
        lf => alloc::format!("read while waiting on {}, after {}", now, call(lf, LAST_ARG.load(Ordering::Relaxed))),
    }
}

impl<Q: QueueMem> GspRpc<'_, Q> {
    /// Handle everything on the ring until `expected` turns up
    /// (`_kgspRpcDrainEvents`).
    ///
    /// `expected` is a function number or, above 0xFFFF, a GMCAPI command id;
    /// with `sequence` the record carries that too, as OpenRM matches a reply
    /// (`_kgspRpcDrainOneEvent`). A GMCAPI record matches only as the response.
    /// `Some` is the expected message, read and not handled; `None` means the
    /// ring is empty. Every event handled on the way is counted into `handled`
    pub fn drain(
        &mut self,
        expected: u32,
        sequence: Option<u32>,
        ctx: Context,
        dispatch: Dispatch,
        handled: &mut usize,
    ) -> Result<Option<RpcMessage>, RpcError> {
        while self.pending() {
            // Something is there, so the timeout only bounds a record whose
            // slots have not all landed
            let m = self.recv(1_000_000)?;
            let done = is_reply(&m, expected, sequence);
            if !done {
                dispatch(&m, ctx);
                *handled += 1;
            }
            // After every record, the reply included, and once more below with
            // the queue empty, as `_kgspRpcDrainEvents` dumps after each turn
            // of its loop
            self.dump_logs();
            if done {
                return Ok(Some(m));
            }
        }
        self.dump_logs();
        // If GSP-RM has died, nothing more is coming (`_kgspRpcDrainEvents`)
        if !crate::drivers::gpu::nvidia::gsp_common::kgsp::health::check(self.bar0) {
            return Err(RpcError::ResetRequired);
        }
        Ok(None)
    }

    /// Wait for `expected`, handling every event arriving first
    /// (`_kgspRpcRecvPoll`).
    ///
    /// `NOTHING` as `expected` drains for `timeout_ns` and then reports
    /// `RecvTimeout`, the normal ending for a caller wanting only the events
    pub fn recv_poll(
        &mut self,
        expected: u32,
        sequence: Option<u32>,
        ctx: Context,
        timeout_ns: u64,
        dispatch: Dispatch,
    ) -> Result<RpcMessage, RpcError> {
        let _polling = Polling::enter()?;
        let t0 = crate::kcore::time::rdtsc();
        let mut handled = 0;
        loop {
            // The clock is read before the drain, so a reply landing at the
            // last moment is taken
            let late = tsc_elapsed_ns(t0) >= timeout_ns;
            if let Some(m) = self.drain(expected, sequence, ctx, dispatch, &mut handled)? {
                return Ok(m);
            }
            // The flag on every turn, the card only when giving up: OpenRM's
            // check tests properties, and a register read in a spin loop is PRI
            // traffic for nothing. A GPU gone is told from one merely slow at
            // the end
            crate::drivers::gpu::nvidia::gsp_common::kgsp::health::sanity(None)?;
            if late {
                crate::drivers::gpu::nvidia::gsp_common::kgsp::health::sanity(self.bar0)?;
                return Err(RpcError::RecvTimeout);
            }
            wait_for_msgq(tsc_elapsed_ns(t0), || self.peek());
        }
    }

    /// Send `function` and wait for its reply, handing events arriving
    /// meanwhile to `dispatch`.
    ///
    /// A request longer than one record went out as several (`send`), each with
    /// its own sequence number, and the reply comes back the same way: a record
    /// of its own length with the first number, and CONTINUATION_RECORDs with
    /// the ones after, filling a reply the size of the request
    /// (`_issueRpcLarge`, the `bBidirectional` half)
    pub fn call_with(
        &mut self,
        function: u32,
        payload: &[u8],
        timeout_ns: u64,
        dispatch: Dispatch,
    ) -> Result<RpcMessage, RpcError> {
        let t0 = crate::kcore::time::rdtsc();
        let mut events = 0u32;
        let mut counting = |m: &RpcMessage, c: Context| {
            events += 1;
            dispatch(m, c)
        };
        IN_FLIGHT_ARG.store(call_arg(function, payload), Ordering::Relaxed);
        IN_FLIGHT_FN.store(function, Ordering::Release);
        let r = self.exchange(function, payload, timeout_ns, &mut counting);
        IN_FLIGHT_FN.store(NOTHING, Ordering::Release);
        LAST_ARG.store(IN_FLIGHT_ARG.load(Ordering::Relaxed), Ordering::Relaxed);
        LAST_FN.store(function, Ordering::Release);
        note_call(function, payload.len(), &r, tsc_elapsed_ns(t0) / 1000, events);
        r
    }

    /// `call_with` without the log line
    fn exchange(
        &mut self,
        function: u32,
        payload: &[u8],
        timeout_ns: u64,
        dispatch: Dispatch,
    ) -> Result<RpcMessage, RpcError> {
        let (records, first) = self.send(function, payload, timeout_ns)?;
        let mut reply = self.recv_poll(function, Some(first), Context::Poll, timeout_ns, dispatch)?;
        let mut next = first.wrapping_add(1);
        if records > 1 {
            while reply.payload.len() < payload.len() {
                let more = self.recv_poll(FN_CONTINUATION_RECORD, Some(next), Context::Poll, timeout_ns, dispatch)?;
                next = next.wrapping_add(1);
                // An empty continuation would never fill the reply
                if more.payload.is_empty() {
                    return Err(RpcError::BadLength(0));
                }
                let take = more.payload.len().min(payload.len() - reply.payload.len());
                reply.payload.extend_from_slice(&more.payload[..take]);
            }
        }
        if reply.rpc_result != 0 {
            return Err(RpcError::GspStatus { function, status: reply.rpc_result });
        }
        Ok(reply)
    }
}

/// Is `m` what a poll for `expected` (and `sequence`) waits for?
fn is_reply(m: &RpcMessage, expected: u32, sequence: Option<u32>) -> bool {
    use super::queue::{NVDM_GMCAPI, NVDM_RM_RPC};
    let kind = if expected > 0xFFFF {
        m.nvdm == NVDM_GMCAPI && m.response
    } else {
        m.nvdm == NVDM_RM_RPC
    };
    kind && m.function == expected && sequence.map_or(true, |s| s == m.sequence)
}

/// The kernel log's line for one RPC (`dev/klog.md`): DEBUG when it worked, and
/// a failure at the level of what it means. RM's own calls are logged by
/// `rmapi`, which knows the control or class
fn note_call(function: u32, sent: usize, r: &Result<RpcMessage, RpcError>, us: u64, events: u32) {
    use crate::drivers::gpu::nvidia::gsp_common::rmapi::{FN_FREE, FN_GSP_RM_ALLOC, FN_GSP_RM_CONTROL};
    if matches!(function, FN_FREE | FN_GSP_RM_ALLOC | FN_GSP_RM_CONTROL) {
        return;
    }
    let name = crate::drivers::gpu::nvidia::gsp_common::rm_names::function(function);
    match r {
        Ok(reply) => crate::kdebug!("[gsp/rpc] {} ({} B) -> {} B in {} us, {} event(s) on the way",
            name, sent, reply.payload.len(), us, events),
        Err(e) => note_failure(name, e, us),
    }
}

/// A transport failure, worded once for `note_call` and for `rmapi`
pub fn note_failure(what: &str, e: &RpcError, us: u64) {
    match e {
        // GSP-RM answered, and said no: the request, not the transport
        RpcError::GspStatus { status, .. } => crate::kwarn!("[gsp/rpc] {} refused: {} after {} us",
            what, super::rpc_result_str(*status), us),
        RpcError::ShortPayload => crate::kwarn!("[gsp/rpc] {}: reply shorter than its struct", what),
        // Nothing came back, or not a message: the transport or GSP-RM is in
        // trouble
        _ => crate::kerror!("[gsp/rpc] {} failed: {:?} after {} us", what, e, us),
    }
}

impl<'a> GspRpc<'a> {
    /// KernelGsp's dispatcher, what every reader on the card passes as its
    /// `Dispatch`
    pub fn events(&self) -> impl FnMut(&RpcMessage, Context) + 'a {
        |m, c| crate::drivers::gpu::nvidia::gsp_common::kgsp::events::dispatch(m, c)
    }

    /// `call_with`, on the card: events go to KernelGsp's dispatcher
    pub fn call(
        &mut self,
        function: u32,
        payload: &[u8],
        timeout_ns: u64,
    ) -> Result<RpcMessage, RpcError> {
        let mut dispatch = self.events();
        self.call_with(function, payload, timeout_ns, &mut dispatch)
    }
}
