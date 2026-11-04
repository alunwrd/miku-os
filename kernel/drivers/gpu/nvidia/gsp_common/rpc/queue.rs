// The two rings, and the transport framing messages onto them
//
// MSGQ 2.0, as GSP-RM 615 speaks it (`dev/gsp-615.md`, sections 1-2; `msgq.c`,
// `message_queue_cpu.c`, `message_queue_priv.h`):
//
// - Each direction is a ring of 4 KiB slots behind a 32-byte header its
//   producer writes (`msgqTxHeader`: version 2.0, size, slot size, slot count,
//   where the slots start). Ours is the command queue, GSP-RM's the status
//   queue.
// - The read and write pointers are GSP registers: QUEUE_HEAD/TAIL for the
//   command queue, MSGQ_HEAD/TAIL for the status queue. They count records ever
//   sent, and a slot is the count modulo the ring. Writing QUEUE_HEAD also
//   tells GSP-RM to look.
// - A record is MCTP-framed: magic "MCTP", the size of everything after the
//   magic's 16 bytes counted from the start, an MCTP transport header (version
//   1, start and end of message), an NVDM header naming what follows (an RM RPC
//   or a GMCAPI command), the payload's size, and the payload. A record longer
//   than a slot takes as many consecutive slots as its size says.
// - The reader overwrites the magic of a record it has read with "SEEN", and
//   skips one already saying SEEN: a stale record, the one integrity check the
//   ABI has.

use core::sync::atomic::{AtomicU32, Ordering};

use alloc::vec;
use alloc::vec::Vec;

use crate::drivers::gpu::nvidia::gsp_common::bootargs::GspBootArgs;
use crate::drivers::gpu::nvidia::le;
use crate::drivers::gpu::nvidia::mmio::MmioRegion;

use super::fns::FN_CONTINUATION_RECORD;
use super::wait::wait_for_msgq;
use super::RpcError;

/// Slot size (`queueElementSizeMin`, `RM_PAGE_SIZE`)
pub const GSP_PAGE: usize = 0x1000;
/// The record header before the payload: magic, MCTP payload size, MCTP header,
/// NVDM header, NVDM payload size, reserved
/// (`GSP_MSG_QUEUE_ELEMENT_SIZE_NO_ENCRYPTION`)
pub const MSG_HDR_SIZE: usize = 24;
/// RPC header size (`rpc_message_header_v`)
pub const RPC_HDR_SIZE: usize = 32;
/// The largest record: 16 slots (`queueElementSizeMax`)
pub const MSG_MAX_SIZE: usize = 16 * GSP_PAGE;
/// The largest payload a record carries (`pRpc->maxRpcSize`)
pub const MAX_PAYLOAD: usize = MSG_MAX_SIZE - MSG_HDR_SIZE;
/// Ceiling for an assembled multi-record reply
pub const RPC_MAX_ASSEMBLED: usize = 1 << 20;

pub const RPC_HEADER_VERSION: u32 = 0x0300_0000;
/// NV_VGPU_MSG_SIGNATURE_VALID ("VRPC")
pub const RPC_SIGNATURE: u32 = 0x4350_5256;

/// NV_VGPU_MSG_RESULT_RPC_PENDING. An outbound request carries this in both
/// rpc_result and rpc_result_private; zero would say "already finished"
pub const RPC_RESULT_PENDING: u32 = 0xFFFF_FFFF;

/// "MCTP", the magic of a record not yet read
pub const MCTP_MAGIC: u32 = 0x4D43_5450;
/// "SEEN", what the reader leaves in its place
pub const MCTP_MAGIC_SEEN: u32 = 0x5345_454E;
/// MCTP transport header: version 1 (3:0), end of message (30), start of
/// message (31). One packet a record: a long RPC is several records, each its
/// own message (`GspMsgQueueSendCommand`)
const MCTP_TRANSPORT: u32 = 0x1 | 1 << 30 | 1 << 31;
/// MCTP message type "vendor defined, PCI" (6:0) and NVIDIA's vendor id (23:8)
const NVDM_VENDOR: u32 = 0x7e | 0x10de << 8;

/// What a record carries (`nvdm_format.h`)
pub const NVDM_RM_RPC: u8 = 0x25;
pub const NVDM_GMCAPI: u8 = 0x26;

/// The ring version this speaks (`MSGQ_VERSION_MAJOR`)
pub const MSGQ_VERSION_MAJOR: u16 = 2;
/// A ring's header (`msgqTxHeader`)
pub const MSGQ_HEADER_SIZE: usize = 32;

/// GSP-RM's records may be stale this many times in a row before the read gives
/// up (`seenMsgIgnores`)
const STALE_IGNORES: u32 = 16;

/// The four pointer registers of queue 0 (`dev_gsp.h`;
/// `gpuGetGspMsgQueueRegisters_TU102`)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueReg {
    /// Our write counter for the command queue; writing it rings GSP-RM
    CmdHead = 0,
    /// How far GSP-RM has read the command queue
    CmdTail = 1,
    /// GSP-RM's write counter for the status queue
    MsgHead = 2,
    /// How far we have read the status queue
    MsgTail = 3,
}

impl QueueReg {
    pub const fn addr(self) -> u32 {
        match self {
            QueueReg::CmdHead => 0x0011_0c00,
            QueueReg::CmdTail => 0x0011_0c04,
            QueueReg::MsgHead => 0x0011_0c80,
            QueueReg::MsgTail => 0x0011_0c84,
        }
    }
}

/// Wall-clock for timeouts comes from the CPU TSC, never PTIMER over BAR0:
/// GSP-RM engages PRI lockdown windows during init, and PRI reads inside one
/// return junk or stall
pub(super) fn tsc_elapsed_ns(start_tsc: u64) -> u64 {
    let khz = crate::kcore::time::tsc_khz().max(1);
    crate::kcore::time::rdtsc()
        .wrapping_sub(start_tsc)
        .saturating_mul(1_000_000)
        / khz
}

/// Milliseconds since `start_tsc`, for a wait that shows its progress
pub fn elapsed_ms(start_tsc: u64) -> u64 {
    tsc_elapsed_ns(start_tsc) / 1_000_000
}

/// One inbound record, read whole.
///
/// An RM RPC (`NVDM_RM_RPC`) gives its function, both result words and its
/// sequence number from `rpc_message_header_v`. A GMCAPI record gives its
/// command id as `function` (ids are above 0xFFFF, so the two never collide:
/// `GMCAPI_CMD_RESERVED_LAST`), `response` from the command's flags, the
/// handler's status as `rpc_result` and the low half of its 64-bit sequence
pub struct RpcMessage {
    pub nvdm: u8,
    pub function: u32,
    pub rpc_result: u32,
    pub sequence: u32,
    /// A GMCAPI record that answers a command
    pub response: bool,
    /// Payload bytes, after the RPC or GMCAPI header
    pub payload: Vec<u8>,
}

impl RpcMessage {
    /// An RM RPC record with these fields, for code that makes messages up
    /// (the dispatcher's selftest)
    pub fn rm(function: u32, payload: Vec<u8>) -> Self {
        Self { nvdm: NVDM_RM_RPC, function, rpc_result: 0, sequence: 0, response: false, payload }
    }
}

/// GSP-RM's ring, as its header says, once linked (`msgqRxLink`)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RxGeometry {
    pub entry_off: u32,
    pub msg_size: u32,
    pub msg_count: u32,
}

/// What the host keeps about both rings (`msgqMetadata`): the counters it owns,
/// its cached copies of GSP-RM's, and the RPC sequence number. Here since a
/// `GspRpc` is rebuilt for every call
#[derive(Clone, Copy, Debug, Default)]
pub struct QueueState {
    /// Records ever written to the command queue (`txWritePtr`)
    pub tx_write: u32,
    /// Last QUEUE_TAIL read (`txReadPtr`)
    pub tx_read: u32,
    /// Records ever read from the status queue (`rxReadPtr`)
    pub rx_read: u32,
    /// Last MSGQ_HEAD read (`rxWritePtr`)
    pub rx_write: u32,
    /// GSP-RM's ring, `None` until linked
    pub rx: Option<RxGeometry>,
    /// The next RPC sequence number (`pRpc->sequence`), shared by RM RPCs and
    /// GMCAPI commands
    pub sequence: u32,
}

/// The memory a transport runs over: our ring's slots, GSP-RM's ring's header
/// and slots, and the state above.
///
/// `GspBootArgs` is the real one. The selftest builds one from `Vec`s with
/// pointer registers of its own, so the framing runs in QEMU
pub trait QueueMem {
    /// Slots in the command queue
    fn cmdq_count(&self) -> u32;
    /// One command-queue slot, `slot < cmdq_count()`
    fn cmdq_slot_mut(&mut self, slot: u32) -> &mut [u8];
    /// The status queue's header, as GSP-RM wrote it (zero until it has)
    fn msgq_header(&self) -> [u8; MSGQ_HEADER_SIZE];
    /// The status queue's size, which its header has to agree with
    fn msgq_size(&self) -> u32;
    /// One status-queue slot under the geometry it was linked with
    fn msgq_slot(&self, rx: RxGeometry, slot: u32) -> &[u8];
    fn msgq_slot_mut(&mut self, rx: RxGeometry, slot: u32) -> &mut [u8];
    fn state(&self) -> &QueueState;
    fn state_mut(&mut self) -> &mut QueueState;
    /// A pointer register, for rings that are not a card's. `None` on the card,
    /// where they are GSP registers read through BAR0
    fn fake_reg(&self, _r: QueueReg) -> Option<u32> {
        None
    }
    /// Set one; false on the card
    fn set_fake_reg(&mut self, _r: QueueReg, _v: u32) -> bool {
        false
    }
    /// GSP-RM's libos log buffers: INIT, INTR, RM, as `kgsp::logs::TAGS` names
    /// them. Rings of no card have none
    fn log_buffers(&self) -> [&[u8]; 3] {
        [&[], &[], &[]]
    }
}

impl QueueMem for GspBootArgs {
    fn cmdq_count(&self) -> u32 { GspBootArgs::cmdq_count(self) }
    fn cmdq_slot_mut(&mut self, slot: u32) -> &mut [u8] { GspBootArgs::cmdq_slot_mut(self, slot) }
    fn msgq_header(&self) -> [u8; MSGQ_HEADER_SIZE] { GspBootArgs::msgq_header(self) }
    fn msgq_size(&self) -> u32 { GspBootArgs::msgq_size(self) }
    fn msgq_slot(&self, rx: RxGeometry, slot: u32) -> &[u8] { GspBootArgs::msgq_slot(self, rx, slot) }
    fn msgq_slot_mut(&mut self, rx: RxGeometry, slot: u32) -> &mut [u8] { GspBootArgs::msgq_slot_mut(self, rx, slot) }
    fn state(&self) -> &QueueState { GspBootArgs::queue_state(self) }
    fn state_mut(&mut self) -> &mut QueueState { GspBootArgs::queue_state_mut(self) }
    fn log_buffers(&self) -> [&[u8]; 3] {
        [self.loginit_slice(), self.logintr_slice(), self.logrm_slice()]
    }
}

/// Records found stale (already marked SEEN) and skipped, since boot
static RX_STALE: AtomicU32 = AtomicU32::new(0);

/// How many stale records the reader has skipped since boot. OpenRM prints
/// each; the log and `nvidia intr` show a count
pub fn stale_records() -> u32 {
    RX_STALE.load(Ordering::Relaxed)
}

/// Why the status queue could not be linked
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// GSP-RM has not written its header yet - try again
    NotYet,
    /// Its header disagrees with what we gave it: (`msgqRxLink`'s -6..-10)
    Mismatch { major: u16, size: u32, msg_size: u32, msg_count: u32, entry_off: u32 },
}

/// The RM RPC header of an outbound record (`rpcWriteCommonHeader`): version,
/// signature, length, function, both results pending, sequence, gfid 0
fn rpc_header(function: u32, length: u32, sequence: u32) -> [u8; RPC_HDR_SIZE] {
    let mut h = [0u8; RPC_HDR_SIZE];
    le::put_u32(&mut h, 0, RPC_HEADER_VERSION);
    le::put_u32(&mut h, 4, RPC_SIGNATURE);
    le::put_u32(&mut h, 8, length);
    le::put_u32(&mut h, 12, function);
    le::put_u32(&mut h, 16, RPC_RESULT_PENDING);
    le::put_u32(&mut h, 20, RPC_RESULT_PENDING);
    le::put_u32(&mut h, 24, sequence);
    h
}

/// The transport over a live set of rings and, on the card, BAR0
pub struct GspRpc<'a, Q: QueueMem = GspBootArgs> {
    /// The card's registers. `None` for rings that are not a card's
    pub(super) bar0: Option<&'a MmioRegion>,
    ba: &'a mut Q,
}

impl<'a> GspRpc<'a> {
    pub fn new(bar0: &'a MmioRegion, ba: &'a mut GspBootArgs) -> Self {
        Self { bar0: Some(bar0), ba }
    }
}

impl<'a, Q: QueueMem> GspRpc<'a, Q> {
    /// A transport over rings that belong to no card, for the selftest
    pub fn over(ba: &'a mut Q) -> Self {
        Self { bar0: None, ba }
    }

    /// Print what GSP-RM has logged since the last look (`kgspDumpGspLogs`)
    pub fn dump_logs(&self) {
        crate::drivers::gpu::nvidia::gsp_common::kgsp::logs::dump(self.ba.log_buffers());
    }

    fn reg(&self, r: QueueReg) -> u32 {
        if let Some(v) = self.ba.fake_reg(r) {
            return v;
        }
        self.bar0.map_or(0, |b| b.read32(r.addr()))
    }

    fn set_reg(&mut self, r: QueueReg, v: u32) {
        if self.ba.set_fake_reg(r, v) {
            return;
        }
        if let Some(b) = self.bar0 {
            b.write32(r.addr(), v);
        }
    }

    /// Whether GSP-RM has written past what we read, from the register alone:
    /// what a sleeping waiter may look at
    pub(super) fn peek(&self) -> bool {
        self.ba.state().rx.is_some() && self.reg(QueueReg::MsgHead) != self.ba.state().rx_read
    }

    /// The next RPC sequence number (`rpcSendMessage`, `gpuSendGmcRpc`)
    pub fn next_sequence(&mut self) -> u32 {
        let st = self.ba.state_mut();
        let s = st.sequence;
        st.sequence = s.wrapping_add(1);
        s
    }

    /// Our ring is ready: its counters at zero and QUEUE_HEAD = 0
    /// (`msgqTxCreate`). The header is in place from `GspBootArgs::build`
    pub fn tx_create(&mut self) {
        let st = self.ba.state_mut();
        st.tx_write = 0;
        st.tx_read = 0;
        crate::mm::dma::DmaBuffer::write_barrier();
        self.set_reg(QueueReg::CmdHead, 0);
    }

    /// Whether GSP-RM's ring is linked
    pub fn rx_linked(&self) -> bool {
        self.ba.state().rx.is_some()
    }

    /// Take GSP-RM's ring as its header describes it, if written and agreeing
    /// with ours (`msgqRxLink`): same size, same slot size, version 2, a slot
    /// count that fills it
    pub fn rx_link(&mut self) -> Result<RxGeometry, LinkError> {
        core::sync::atomic::fence(Ordering::SeqCst);
        let h = self.ba.msgq_header();
        let major = le::u16_at(&h, 0);
        let size = le::u32_at(&h, 4);
        let msg_size = le::u32_at(&h, 8);
        let msg_count = le::u32_at(&h, 12);
        let entry_off = le::u32_at(&h, 16);
        if major == 0 && size == 0 {
            return Err(LinkError::NotYet);
        }
        let want = self.ba.msgq_size();
        let ok = major == MSGQ_VERSION_MAJOR
            && size == want
            && msg_size == GSP_PAGE as u32
            && entry_off as u64 + msg_size as u64 <= size as u64
            && msg_count != 0
            && msg_count == (size - entry_off) / msg_size;
        if !ok {
            return Err(LinkError::Mismatch { major, size, msg_size, msg_count, entry_off });
        }
        let rx = RxGeometry { entry_off, msg_size, msg_count };
        let st = self.ba.state_mut();
        st.rx = Some(rx);
        st.rx_read = 0;
        st.rx_write = 0;
        crate::mm::dma::DmaBuffer::write_barrier();
        self.set_reg(QueueReg::MsgTail, 0);
        Ok(rx)
    }

    // Send

    /// Free slots in the command queue (`msgqTxGetFreeSpace`). A QUEUE_TAIL
    /// reading behind what is possible, reset to 0 under us, keeps the cached
    /// value or, if the queue was full, counts it empty
    fn cmdq_free(&mut self) -> u32 {
        let count = self.ba.cmdq_count();
        let mut read = self.reg(QueueReg::CmdTail);
        let st = *self.ba.state();
        if st.tx_write.wrapping_sub(read) > count {
            read = if read == 0 && st.tx_write.wrapping_sub(st.tx_read) == count {
                st.tx_write
            } else {
                st.tx_read
            };
        }
        self.ba.state_mut().tx_read = read;
        count - st.tx_write.wrapping_sub(read)
    }

    /// Put one framed record (whole slots) in the command queue and ring
    /// GSP-RM (`GspMsgQueueSendCommand`, `msgqTxSubmitBuffers`)
    fn cmdq_push(&mut self, record: &[u8], timeout_ns: u64) -> Result<(), RpcError> {
        debug_assert!(record.len() % GSP_PAGE == 0);
        let pages = (record.len() / GSP_PAGE) as u32;
        let t0 = crate::kcore::time::rdtsc();
        while self.cmdq_free() < pages {
            if tsc_elapsed_ns(t0) >= timeout_ns {
                return Err(RpcError::CmdqTimeout);
            }
            core::hint::spin_loop();
        }
        let count = self.ba.cmdq_count();
        let w = self.ba.state().tx_write;
        for p in 0..pages {
            let src = &record[p as usize * GSP_PAGE..(p as usize + 1) * GSP_PAGE];
            self.ba.cmdq_slot_mut(w.wrapping_add(p) % count).copy_from_slice(src);
        }
        // The slots are in memory before the head says they are there
        crate::mm::dma::DmaBuffer::write_barrier();
        let w = w.wrapping_add(pages);
        self.ba.state_mut().tx_write = w;
        self.set_reg(QueueReg::CmdHead, w);
        Ok(())
    }

    /// Frame one NVDM payload and send it
    fn push_nvdm(&mut self, nvdm: u8, payload: &[u8], timeout_ns: u64) -> Result<(), RpcError> {
        if payload.len() > MAX_PAYLOAD {
            return Err(RpcError::TooLarge(payload.len()));
        }
        let mctp_size = MSG_HDR_SIZE + payload.len();
        let mut record = vec![0u8; mctp_size.div_ceil(GSP_PAGE) * GSP_PAGE];
        le::put_u32(&mut record, 0, MCTP_MAGIC);
        le::put_u32(&mut record, 4, mctp_size as u32);
        le::put_u32(&mut record, 8, MCTP_TRANSPORT);
        le::put_u32(&mut record, 12, NVDM_VENDOR | (nvdm as u32) << 24);
        le::put_u32(&mut record, 16, payload.len() as u32);
        record[MSG_HDR_SIZE..mctp_size].copy_from_slice(payload);
        self.cmdq_push(&record, timeout_ns)
    }

    /// Send an RM RPC, cut into CONTINUATION_RECORDs when longer than a record,
    /// as `_issueRpcLarge` cuts it: the first record as much as fits, every
    /// later one its own header and the next piece. Every record takes a
    /// sequence number. Returns how many records went out and the first one's
    /// sequence, what the replies carry
    pub fn send(&mut self, function: u32, payload: &[u8], timeout_ns: u64) -> Result<(u32, u32), RpcError> {
        // `_kgspRpcSendMessage` asks first
        crate::drivers::gpu::nvidia::gsp_common::kgsp::health::sanity(self.bar0)?;
        if payload.len() > RPC_MAX_ASSEMBLED {
            return Err(RpcError::TooLarge(payload.len()));
        }
        let cap = MAX_PAYLOAD - RPC_HDR_SIZE;
        let mut off = 0;
        let mut records = 0u32;
        let mut first = None;
        while records == 0 || off < payload.len() {
            let chunk = (payload.len() - off).min(cap);
            let func = if records == 0 { function } else { FN_CONTINUATION_RECORD };
            let seq = self.next_sequence();
            first.get_or_insert(seq);
            let mut body = Vec::with_capacity(RPC_HDR_SIZE + chunk);
            body.extend_from_slice(&rpc_header(func, (RPC_HDR_SIZE + chunk) as u32, seq));
            body.extend_from_slice(&payload[off..off + chunk]);
            self.push_nvdm(NVDM_RM_RPC, &body, timeout_ns)?;
            off += chunk;
            records += 1;
        }
        Ok((records, first.unwrap_or(0)))
    }

    /// Send a GMCAPI command (`gpuSendGmcRpc_KERNEL`): its 40-byte header,
    /// then `data`. Returns the sequence the response will carry
    pub fn send_gmcapi(&mut self, command: u32, data: &[u8], max_response: u32, timeout_ns: u64)
        -> Result<u32, RpcError>
    {
        crate::drivers::gpu::nvidia::gsp_common::kgsp::health::sanity(self.bar0)?;
        let seq = self.next_sequence();
        let mut body = Vec::with_capacity(crate::drivers::gpu::nvidia::gsp_common::gmcapi::HEADER + data.len());
        body.extend_from_slice(&crate::drivers::gpu::nvidia::gsp_common::gmcapi::request(command, data.len() as u32, seq, max_response));
        body.extend_from_slice(data);
        self.push_nvdm(NVDM_GMCAPI, &body, timeout_ns)?;
        Ok(seq)
    }

    // Receive

    /// Records GSP-RM has written and we have not read
    /// (`msgqRxGetReadAvailable`, with the same repair as `cmdq_free`)
    fn msgq_used(&mut self) -> u32 {
        let Some(rx) = self.ba.state().rx else { return 0 };
        let mut write = self.reg(QueueReg::MsgHead);
        let st = *self.ba.state();
        if write.wrapping_sub(st.rx_read) > rx.msg_count {
            write = if write == 0 && st.rx_write.wrapping_sub(st.rx_read) == rx.msg_count {
                st.rx_read
            } else {
                st.rx_write
            };
        }
        self.ba.state_mut().rx_write = write;
        write.wrapping_sub(st.rx_read)
    }

    /// Step past `pages` slots and tell GSP-RM (`msgqRxMarkConsumed`)
    fn msgq_consume(&mut self, pages: u32) {
        let r = self.ba.state().rx_read.wrapping_add(pages);
        self.ba.state_mut().rx_read = r;
        crate::mm::dma::DmaBuffer::write_barrier();
        self.set_reg(QueueReg::MsgTail, r);
    }

    /// Wait for the next record, read it whole and consume it
    /// (`GspMsgQueueReceiveStatus`).
    ///
    /// Its size comes from its own header, checked against the ring before
    /// anything is copied. A record marked SEEN is skipped, at most
    /// `STALE_IGNORES` in a row; one read is marked SEEN in the ring. The
    /// checks are OpenRM's: MCTP version 1, vendor NVIDIA, magic MCTP or SEEN,
    /// a size inside one record
    pub fn recv(&mut self, timeout_ns: u64) -> Result<RpcMessage, RpcError> {
        let t0 = crate::kcore::time::rdtsc();
        let mut stale = 0;
        loop {
            while self.msgq_used() == 0 {
                if tsc_elapsed_ns(t0) >= timeout_ns {
                    return Err(RpcError::RecvTimeout);
                }
                wait_for_msgq(tsc_elapsed_ns(t0), || self.peek());
            }
            let rx = self.ba.state().rx.ok_or(RpcError::NotLinked)?;
            let first_slot = self.ba.state().rx_read % rx.msg_count;
            let head = self.ba.msgq_slot(rx, first_slot);
            let magic = le::u32_at(head, 0);
            let mctp_size = le::u32_at(head, 4) as usize;
            let transport = le::u32_at(head, 8);
            let nvdm_header = le::u32_at(head, 12);
            if mctp_size < MSG_HDR_SIZE || mctp_size > MSG_MAX_SIZE {
                // Nothing else in it can be trusted, its length least: one slot
                // is consumed so the ring keeps moving
                self.msgq_consume(1);
                return Err(RpcError::BadLength(mctp_size as u32));
            }
            let pages = mctp_size.div_ceil(GSP_PAGE) as u32;
            // The rest of a long record may still be on its way
            while self.msgq_used() < pages {
                if tsc_elapsed_ns(t0) >= timeout_ns {
                    return Err(RpcError::RecvTimeout);
                }
                core::hint::spin_loop();
            }
            if transport & 0xf != 1 || (nvdm_header >> 8) & 0xffff != 0x10de
                || (magic != MCTP_MAGIC && magic != MCTP_MAGIC_SEEN)
            {
                self.msgq_consume(pages);
                return Err(RpcError::BadFrame { magic, transport, nvdm: nvdm_header });
            }
            if magic == MCTP_MAGIC_SEEN {
                RX_STALE.fetch_add(1, Ordering::Relaxed);
                self.msgq_consume(pages);
                stale += 1;
                if stale > STALE_IGNORES {
                    return Err(RpcError::Stale);
                }
                continue;
            }
            le::put_u32(self.ba.msgq_slot_mut(rx, first_slot), 0, MCTP_MAGIC_SEEN);

            // The NVDM payload, across the record's slots
            let nvdm_len = (le::u32_at(self.ba.msgq_slot(rx, first_slot), 16) as usize)
                .min(mctp_size - MSG_HDR_SIZE);
            let mut payload = vec![0u8; nvdm_len];
            let mut done = 0;
            let mut at = MSG_HDR_SIZE;
            while done < nvdm_len {
                let p = (at / GSP_PAGE) as u32;
                let o = at % GSP_PAGE;
                let take = (GSP_PAGE - o).min(nvdm_len - done);
                let slot = self.ba.state().rx_read.wrapping_add(p) % rx.msg_count;
                payload[done..done + take].copy_from_slice(&self.ba.msgq_slot(rx, slot)[o..o + take]);
                done += take;
                at += take;
            }
            self.msgq_consume(pages);
            return parse_record((nvdm_header >> 24) as u8, payload);
        }
    }

    /// Whether anything is waiting to be read, without waiting for it
    pub(super) fn pending(&mut self) -> bool {
        self.msgq_used() != 0
    }
}

/// Take one NVDM payload apart into a message
fn parse_record(nvdm: u8, mut payload: Vec<u8>) -> Result<RpcMessage, RpcError> {
    use crate::drivers::gpu::nvidia::gsp_common::gmcapi;
    match nvdm {
        NVDM_RM_RPC => {
            if payload.len() < RPC_HDR_SIZE {
                return Err(RpcError::BadLength(payload.len() as u32));
            }
            let version = le::u32_at(&payload, 0);
            let signature = le::u32_at(&payload, 4);
            if version != RPC_HEADER_VERSION || signature != RPC_SIGNATURE {
                return Err(RpcError::BadRpcHeader { version, signature });
            }
            let length = le::u32_at(&payload, 8) as usize;
            if length < RPC_HDR_SIZE || length > payload.len() {
                return Err(RpcError::BadLength(length as u32));
            }
            let function = le::u32_at(&payload, 12);
            let rpc_result = le::u32_at(&payload, 16);
            let sequence = le::u32_at(&payload, 24);
            payload.truncate(length);
            payload.drain(..RPC_HDR_SIZE);
            Ok(RpcMessage { nvdm, function, rpc_result, sequence, response: false, payload })
        }
        NVDM_GMCAPI => {
            let h = gmcapi::Header::parse(&payload).ok_or(RpcError::BadLength(payload.len() as u32))?;
            let end = (gmcapi::HEADER + h.size as usize).min(payload.len());
            payload.truncate(end);
            payload.drain(..gmcapi::HEADER);
            Ok(RpcMessage {
                nvdm,
                function: h.id(),
                rpc_result: if h.is_response() { h.status_or_max } else { 0 },
                sequence: h.sequence as u32,
                response: h.is_response(),
                payload,
            })
        }
        other => Err(RpcError::UnknownNvdm(other)),
    }
}
