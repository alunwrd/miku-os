// GSP-RM RPC transport over the CMDQ/MSGQ shared region, r535/r570 line :D
//
// Modeled on nouveau rm/r535/rpc.c. One ring element = one 4 KiB page:
//
//   +--------------------------------+  page boundary
//   | element header, 48 B           |  auth_tag[16] aad[16] checksum
//   |                                |  sequence elem_count pad
//   +--------------------------------+
//   | RPC header, 32 B ("VGPU")      |  version sig length function
//   |                                |  result result_priv seq gfid
//   +--------------------------------+
//   | payload (length - 32 bytes)    |
//   +--------------------------------+
//
// A message longer than one page occupies elem_count consecutive pages
// (wrapping at the ring end); longer than 16 pages it is split into
// multiple elements, the follow-ups carrying function 71
// (CONTINUATION_RECORD) whose payloads are concatenated on receive
//
// The element checksum is a XOR of all u64 words over the page-aligned
// element (checksum field zeroed), folded to 32 bits. GSP verifies it on
// the cmdq; we do not verify it on the msgq (nouveau does not either)
//
// After bumping the cmdq write pointer the host must ring the GSP
// doorbell (falcon +0xC00 <- 0), or GSP-RM will not look at the queue

#![allow(dead_code)]

use alloc::vec;
use alloc::vec::Vec;

use crate::drivers::gpu::nvidia::gtx1650::bootargs::GspBootArgs;
use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::serial_println;

/// GSP page / ring element granularity
pub const GSP_PAGE: usize = 0x1000;
/// Element header size (r535_gsp_msg through 'pad')
pub const MSG_HDR_SIZE: usize = 48;
/// RPC header size (nvfw_gsp_rpc through 'cpuRmGfid')
pub const RPC_HDR_SIZE: usize = 32;
/// Max element size: 16 pages (GSP_MSG_MAX_SIZE)
pub const MSG_MAX_SIZE: usize = 16 * GSP_PAGE;
/// Ceiling for an assembled multi-element reply
pub const RPC_MAX_ASSEMBLED: usize = 1 << 20;

pub const RPC_HEADER_VERSION: u32 = 0x0300_0000;
/// NV_VGPU_MSG_SIGNATURE_VALID. Confirmed on live GB206 silicon: the
/// first GSP-RM message carried exactly this word
pub const RPC_SIGNATURE: u32 = 0x4350_5256;

// NV_VGPU_MSG_FUNCTION_* (r570 nvrm/rpcfn.h; stable across r535/r570)
pub const FN_GET_GSP_STATIC_INFO: u32 = 65;
pub const FN_CONTINUATION_RECORD: u32 = 71;
pub const FN_GSP_SET_SYSTEM_INFO: u32 = 72;
pub const FN_SET_REGISTRY: u32 = 73;

// NV_VGPU_MSG_EVENT_* (GSP -> host async events; r570 nvrm/msgfn.h)
pub const EV_FIRST_EVENT: u32 = 0x1000;
pub const EV_GSP_INIT_DONE: u32 = 0x1001;
pub const EV_GSP_RUN_CPU_SEQUENCER: u32 = 0x1002;
pub const EV_POST_EVENT: u32 = 0x1003;
pub const EV_RC_TRIGGERED: u32 = 0x1004;
pub const EV_MMU_FAULT_QUEUED: u32 = 0x1005;
pub const EV_OS_ERROR_LOG: u32 = 0x1006;
pub const EV_UCODE_LIBOS_PRINT: u32 = 0x100C;
pub const EV_GSP_LOCKDOWN_NOTICE: u32 = 0x101C;
pub const EV_GSP_POST_NOCAT_RECORD: u32 = 0x1020;

/// Human name for the events the driver actually encounters (nouveau
/// drops NOCAT records on r570 too)
pub fn event_name(function: u32) -> &'static str {
    match function {
        EV_GSP_INIT_DONE => "GSP_INIT_DONE",
        EV_GSP_RUN_CPU_SEQUENCER => "RUN_CPU_SEQUENCER",
        EV_POST_EVENT => "POST_EVENT",
        EV_RC_TRIGGERED => "RC_TRIGGERED",
        EV_MMU_FAULT_QUEUED => "MMU_FAULT_QUEUED",
        EV_OS_ERROR_LOG => "OS_ERROR_LOG",
        EV_UCODE_LIBOS_PRINT => "UCODE_LIBOS_PRINT",
        EV_GSP_LOCKDOWN_NOTICE => "GSP_LOCKDOWN_NOTICE",
        EV_GSP_POST_NOCAT_RECORD => "GSP_POST_NOCAT_RECORD",
        _ => "?",
    }
}

/// GSP doorbell: falcon window offset the host writes 0 to after bumping
/// the cmdq write pointer (nouveau: nvkm_falcon_wr32(falcon, 0xc00, 0)).
/// The GSP falcon base is 0x110000 on every GSP-era chip
pub const PGSP_DOORBELL: u32 = 0x0011_0C00;

/// Wall-clock for timeouts comes from the CPU TSC, never from PTIMER over
/// BAR0: GSP-RM engages PRI lockdown windows during its init (the
/// GSP_LOCKDOWN_NOTICE event flood on Blackwell) and PRI reads inside a
/// window return junk or stall, freezing a PTIMER-based timeout forever
fn tsc_elapsed_ns(start_tsc: u64) -> u64 {
    let khz = crate::kcore::time::tsc_khz().max(1);
    crate::kcore::time::rdtsc()
        .wrapping_sub(start_tsc)
        .saturating_mul(1_000_000)
        / khz
}

#[derive(Debug, Clone, Copy)]
pub enum RpcError {
    /// Message would not fit even split into continuation elements
    TooLarge(usize),
    /// GSP never freed enough cmdq pages
    CmdqTimeout,
    /// No reply within the timeout
    RecvTimeout,
    /// Reply framing was not VGPU / expected version
    BadFrame { version: u32, signature: u32 },
    /// A continuation element did not carry FN_CONTINUATION_RECORD
    BadContinuation { function: u32 },
    /// The reply's rpc_result was non-zero
    GspStatus { function: u32, status: u32 },
}

/// One fully-assembled inbound RPC
pub struct RpcMessage {
    pub function: u32,
    pub rpc_result: u32,
    pub rpc_result_private: u32,
    pub sequence: u32,
    /// Payload bytes (after the 32-byte RPC header)
    pub payload: Vec<u8>,
}

/// XOR-fold checksum over the page-aligned element (nouveau
/// r535_gsp_cmdq_push): u64 XOR of the whole buffer, upper^lower 32
fn element_checksum(buf: &[u8]) -> u32 {
    let mut sum: u64 = 0;
    let mut i = 0;
    while i + 8 <= buf.len() {
        sum ^= u64::from_le_bytes(buf[i..i + 8].try_into().unwrap());
        i += 8;
    }
    ((sum >> 32) as u32) ^ (sum as u32)
}

fn put_u32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn get_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

/// RPC driver bound to a live boot-args region and BAR0 (doorbell +
/// PTIMER). Chip-independent: both the Turing and Blackwell drivers can
/// construct one once GSP-RM is up
pub struct GspRpc<'a> {
    bar0: &'a MmioRegion,
    ba: &'a mut GspBootArgs,
    cmdq_seq: u32,
    rpc_seq: u32,
}

impl<'a> GspRpc<'a> {
    pub fn new(bar0: &'a MmioRegion, ba: &'a mut GspBootArgs) -> Self {
        Self { bar0, ba, cmdq_seq: 0, rpc_seq: 0 }
    }

     // Send

    /// Free element pages in the cmdq (one-slot-empty convention)
    fn cmdq_free(&self) -> u32 {
        let n = self.ba.queue_entries();
        let wptr = self.ba.cmdq_write_ptr();
        let rptr = self.ba.cmdq_gsp_read_ptr();
        let mut free = rptr + n - wptr - 1;
        if free >= n { free -= n; }
        free
    }

    /// Write one framed element (already checksummed, page-aligned) into
    /// the cmdq, wrapping at the ring end, and ring the doorbell
    fn cmdq_push(&mut self, element: &[u8], timeout_ns: u32) -> Result<(), RpcError> {
        debug_assert!(element.len() % GSP_PAGE == 0);
        let n = self.ba.queue_entries();
        let pages = (element.len() / GSP_PAGE) as u32;

        // Wait for enough free pages
        let t0 = crate::kcore::time::rdtsc();
        while self.cmdq_free() < pages {
            if tsc_elapsed_ns(t0) >= timeout_ns as u64 {
                return Err(RpcError::CmdqTimeout);
            }
            core::hint::spin_loop();
        }

        let mut wptr = self.ba.cmdq_write_ptr();
        for p in 0..pages {
            let src = &element[(p as usize) * GSP_PAGE..(p as usize + 1) * GSP_PAGE];
            self.ba.cmdq_page_mut(wptr).copy_from_slice(src);
            wptr = (wptr + 1) % n;
        }
        self.ba.set_cmdq_write_ptr(wptr);
        // Doorbell so GSP-RM actually polls the queue
        self.bar0.write32(PGSP_DOORBELL, 0);
        Ok(())
    }

    /// Frame + send one RPC. Splits into continuation elements when the
    /// message exceeds 16 pages (nouveau r535_gsp_rpc_push)
    pub fn send(&mut self, function: u32, payload: &[u8], timeout_ns: u32) -> Result<u32, RpcError> {
        let rpc_seq = self.rpc_seq;
        self.rpc_seq = self.rpc_seq.wrapping_add(1);

        if payload.len() > RPC_MAX_ASSEMBLED {
            return Err(RpcError::TooLarge(payload.len()));
        }

        let mut off = 0usize; // consumed payload bytes
        let mut first = true;
        while first || off < payload.len() {
            // Payload capacity of this element
            let cap = MSG_MAX_SIZE - MSG_HDR_SIZE - RPC_HDR_SIZE;
            let chunk = (payload.len() - off).min(cap);
            let func = if first { function } else { FN_CONTINUATION_RECORD };
            // Every element's RPC length covers only its own chunk; on a
            // split message the peer knows the full size from the function
            // (nouveau r535_gsp_rpc_push clamps rpc->length the same way)
            let wire_len = RPC_HDR_SIZE + chunk;

            let elem_len = (MSG_HDR_SIZE + RPC_HDR_SIZE + chunk).div_ceil(GSP_PAGE) * GSP_PAGE;
            let mut elem = vec![0u8; elem_len];

            // Element header
            put_u32(&mut elem, 36, self.cmdq_seq); // sequence
            self.cmdq_seq = self.cmdq_seq.wrapping_add(1);
            put_u32(&mut elem, 40, (elem_len / GSP_PAGE) as u32); // elem_count

            // RPC header
            let r = MSG_HDR_SIZE;
            put_u32(&mut elem, r, RPC_HEADER_VERSION);
            put_u32(&mut elem, r + 4, RPC_SIGNATURE);
            put_u32(&mut elem, r + 8, wire_len as u32);
            put_u32(&mut elem, r + 12, func);
            put_u32(&mut elem, r + 24, rpc_seq);

            // Payload chunk
            elem[r + RPC_HDR_SIZE..r + RPC_HDR_SIZE + chunk]
                .copy_from_slice(&payload[off..off + chunk]);

            // Checksum over the whole aligned element
            let csum = element_checksum(&elem);
            put_u32(&mut elem, 32, csum);

            self.cmdq_push(&elem, timeout_ns)?;
            off += chunk;
            first = false;
        }
        Ok(rpc_seq)
    }

    // Receive

    /// Used element pages currently readable in the msgq
    fn msgq_used(&self) -> u32 {
        let n = self.ba.queue_entries();
        let wptr = self.ba.msgq_gsp_write_ptr();
        let rptr = self.ba.msgq_host_read_ptr();
        let mut used = wptr + n - rptr;
        if used >= n { used -= n; }
        used
    }

    /// Copy the element byte range [skip, skip + out.len()) into 'out'
    /// (byte 0 = start of the element header; pages wrap at the ring end),
    /// then advance the read pointer past the whole element
    fn msgq_copy_element(&mut self, skip: usize, out: &mut [u8], elem_bytes: usize) {
        let n = self.ba.queue_entries();
        let pages = elem_bytes.div_ceil(GSP_PAGE) as u32;
        let mut rptr = self.ba.msgq_host_read_ptr();
        let want_end = skip + out.len();

        let mut copied = 0usize; // element bytes walked so far
        for _ in 0..pages {
            let page_len = GSP_PAGE.min(elem_bytes - copied);
            // Overlap of this page's [copied, copied + page_len) with
            // the wanted [skip, want_end)
            let gs = copied.max(skip);
            let ge = (copied + page_len).min(want_end);
            if gs < ge {
                let page = self.ba.msgq_page(rptr);
                out[gs - skip..ge - skip].copy_from_slice(&page[gs - copied..ge - copied]);
            }
            copied += page_len;
            rptr = (rptr + 1) % n;
        }
        self.ba.set_msgq_host_read_ptr(rptr);
    }

    /// Wait for the next element and return its header fields without
    /// consuming it: (rpc_length, function)
    fn msgq_peek(&self, timeout_ns: u32) -> Result<(u32, u32), RpcError> {
        let t0 = crate::kcore::time::rdtsc();
        while self.msgq_used() == 0 {
            if tsc_elapsed_ns(t0) >= timeout_ns as u64 {
                return Err(RpcError::RecvTimeout);
            }
            core::hint::spin_loop();
        }
        let page = self.ba.msgq_page(self.ba.msgq_host_read_ptr());
        let version = get_u32(page, MSG_HDR_SIZE);
        let signature = get_u32(page, MSG_HDR_SIZE + 4);
        if version != RPC_HEADER_VERSION || signature != RPC_SIGNATURE {
            return Err(RpcError::BadFrame { version, signature });
        }
        let length = get_u32(page, MSG_HDR_SIZE + 8);
        let function = get_u32(page, MSG_HDR_SIZE + 12);
        Ok((length, function))
    }

    /// Receive one complete RPC, assembling continuation records
    /// (nouveau r535_gsp_msgq_recv)
    pub fn recv(&mut self, timeout_ns: u32) -> Result<RpcMessage, RpcError> {
        let (length, function) = self.msgq_peek(timeout_ns)?;
        let total_len = (length as usize).min(RPC_MAX_ASSEMBLED).max(RPC_HDR_SIZE);

        // Header fields from the first element
        let first_page = self.ba.msgq_page(self.ba.msgq_host_read_ptr());
        let rpc_result = get_u32(first_page, MSG_HDR_SIZE + 16);
        let rpc_result_private = get_u32(first_page, MSG_HDR_SIZE + 20);
        let sequence = get_u32(first_page, MSG_HDR_SIZE + 24);

        let mut payload = vec![0u8; total_len - RPC_HDR_SIZE];

        // The element byte range we want starts after both headers.
        // A single element carries up to 16 pages, which covers every
        // reply the sequencer currently makes; a longer split reply
        // would arrive as CONTINUATION_RECORD elements, handled below
        let first_elem_rpc = (total_len + MSG_HDR_SIZE).min(MSG_MAX_SIZE) - MSG_HDR_SIZE;
        let first_chunk = first_elem_rpc - RPC_HDR_SIZE;
        self.msgq_copy_element(
            MSG_HDR_SIZE + RPC_HDR_SIZE,
            &mut payload[..first_chunk],
            MSG_HDR_SIZE + first_elem_rpc,
        );

        // Continuations for the rest
        let mut have = first_chunk;
        while have < payload.len() {
            let (clen, cfun) = self.msgq_peek(timeout_ns)?;
            if cfun != FN_CONTINUATION_RECORD {
                return Err(RpcError::BadContinuation { function: cfun });
            }
            let chunk = (clen as usize).saturating_sub(RPC_HDR_SIZE)
                .min(payload.len() - have);
            self.msgq_copy_element(
                MSG_HDR_SIZE + RPC_HDR_SIZE,
                &mut payload[have..have + chunk],
                MSG_HDR_SIZE + clen as usize,
            );
            have += chunk;
        }

        Ok(RpcMessage { function, rpc_result, rpc_result_private, sequence, payload })
    }

    /// Send a command and wait for its reply, logging and skipping any
    /// async events GSP posts in between (nouveau r535_gsp_rpc_push +
    /// r535_gsp_msg_recv retry loop)
    pub fn call(
        &mut self,
        function: u32,
        payload: &[u8],
        timeout_ns: u32,
    ) -> Result<RpcMessage, RpcError> {
        self.send(function, payload, timeout_ns)?;
        let t0 = crate::kcore::time::rdtsc();
        loop {
            let msg = self.recv(timeout_ns)?;
            if msg.function == function {
                if msg.rpc_result != 0 {
                    return Err(RpcError::GspStatus {
                        function,
                        status: msg.rpc_result,
                    });
                }
                return Ok(msg);
            }
            // Event or unrelated message: log and keep draining
            serial_println!(
                "[gsp/rpc] skipping message function={:#x} result={:#x} len={}",
                msg.function, msg.rpc_result, msg.payload.len()
            );
            if tsc_elapsed_ns(t0) >= timeout_ns as u64 {
                return Err(RpcError::RecvTimeout);
            }
        }
    }

    /// Busy-wait for 'ns' of wall clock (bounded settle delay for
    /// fire-and-forget commands)
    pub fn wait_quiesce(&self, ns: u32) {
        let t0 = crate::kcore::time::rdtsc();
        while tsc_elapsed_ns(t0) < ns as u64 {
            core::hint::spin_loop();
        }
    }

    /// Drain everything currently queued (boot-time events like
    /// GSP_INIT_DONE / RUN_CPU_SEQUENCER / OS_ERROR_LOG). Returns how many
    /// messages were consumed
    pub fn drain_events(&mut self) -> usize {
        let mut count = 0;
        while self.msgq_used() != 0 {
            match self.recv(1_000_000) {
                Ok(m) => {
                    serial_println!(
                        "[gsp/rpc] event function={:#x}{} len={}",
                        m.function,
                        if m.function == EV_GSP_INIT_DONE { " (GSP_INIT_DONE)" } else { "" },
                        m.payload.len()
                    );
                    count += 1;
                }
                Err(e) => {
                    serial_println!("[gsp/rpc] drain: {:?}", e);
                    break;
                }
            }
        }
        count
    }
}
