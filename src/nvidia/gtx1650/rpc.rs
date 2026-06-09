// GSP-RM RPC framing on top of the CMDQ/MSGQ rings
//
// Once GSP-RM has booted and the queue pair has been handed to it, the
// host talks to GSP via fixed-format messages that begin with a
// 'RpcHeader' (8 u32 = 32 bytes), followed by a function-specific
// payload. The 32-bit 'function' field selects what GSP-RM does with
// the payload; the response carries the same function code and an
// 'rpc_result' status word.
//
// Numbers come from NVIDIA's open-gpu-kernel-modules headers:
//   src/common/sdk/nvidia/inc/ctrl/ctrl0080/ctrl0080gsp.h
//   r535 NV_VGPU_MSG_FUNCTION_* (kernel/inc/published/gsp/gspifrpc.h)

#![allow(dead_code)]

use core::sync::atomic::{AtomicU32, Ordering};

use super::msgq::{Msgq, MSGQ_ELEMENT_SIZE};

/// Per nouveau r535: header_version is fixed at 0x03000000 for r535
pub const RPC_HEADER_VERSION: u32 = 0x0300_0000;

/// "VGPU" little-endian - the booted GSP-RM checks for this in every
/// message to distinguish RPC traffic from line-noise
pub const RPC_SIGNATURE: u32 = 0x4756_5055;

/// r535 status code returned by GSP-RM when it accepts a command. Any
/// other value in the reply's 'rpc_result' indicates an error
pub const RPC_OK: u32 = 0x0000_0000;

/// Sentinel rpc_result in a freshly-sent message (host has not received
/// a reply yet; GSP overwrites this with 'RPC_OK' or an error code)
pub const RPC_PENDING: u32 = 0xFFFF_FFFF;

/// Subset of NV_VGPU_MSG_FUNCTION_* the driver currently exercises.
/// Numbers are stable across the r535 line and match NVIDIA's
/// open-gpu-kernel-modules headers
#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RpcFunction {
    /// First post-boot command. Carries host info (PCI ids, IOMMU
    /// state, driver version). GSP needs this before any other RPC
    SetSystemInfo = 0x0001,
    /// Pull GPU static info: name, FB size, engine bitmap, ...
    GetGspStaticInfo = 0x0002,
    /// SET_REGISTRY: pass a serialised registry blob (mostly empty for
    /// us) so GSP-RM honours host-side knobs
    SetRegistry = 0x0003,
    /// GSP-async event: GSP-RM finished booting and the runtime is up.
    /// Host should see this on MSGQ shortly after kicking booter_load
    GspInitDone = 0x108E,
}

impl RpcFunction {
    pub fn as_u32(self) -> u32 { self as u32 }
    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            0x0001 => Some(Self::SetSystemInfo),
            0x0002 => Some(Self::GetGspStaticInfo),
            0x0003 => Some(Self::SetRegistry),
            0x108E => Some(Self::GspInitDone),
            _ => None,
        }
    }
}

/// 32-byte RPC header. Bit-pattern compatible with gsprm::GspRpcHeader
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct RpcHeader {
    pub header_version: u32,
    pub signature:      u32,
    pub length:         u32,
    pub function:       u32,
    pub rpc_result:     u32,
    pub rpc_result_priv:u32,
    pub sequence:       u32,
    pub cpu_rm_gfid:    u32,
}

impl RpcHeader {
    pub const SIZE: usize = 32;

    pub fn new(function: u32, payload_len: u32, sequence: u32) -> Self {
        Self {
            header_version: RPC_HEADER_VERSION,
            signature:      RPC_SIGNATURE,
            length:         payload_len + Self::SIZE as u32,
            function,
            rpc_result:     RPC_PENDING,
            rpc_result_priv:RPC_PENDING,
            sequence,
            cpu_rm_gfid:    0,
        }
    }

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        let words = [
            self.header_version, self.signature, self.length, self.function,
            self.rpc_result, self.rpc_result_priv, self.sequence, self.cpu_rm_gfid,
        ];
        for (i, w) in words.iter().enumerate() {
            out[i*4..i*4+4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < Self::SIZE { return None; }
        let w = |i: usize| u32::from_le_bytes(b[i*4..i*4+4].try_into().unwrap());
        Some(Self {
            header_version:  w(0),
            signature:       w(1),
            length:          w(2),
            function:        w(3),
            rpc_result:      w(4),
            rpc_result_priv: w(5),
            sequence:        w(6),
            cpu_rm_gfid:     w(7),
        })
    }
}

#[derive(Debug, Copy, Clone)]
pub enum RpcError {
    /// Payload didn't fit into a 4 KiB ring slot
    PayloadTooLarge { size: usize, max: usize },
    /// CMDQ is full - GSP hasn't consumed enough commands
    CmdqFull,
    /// Polling for a reply timed out
    Timeout,
    /// Reply header had a bad signature or version
    BadHeader,
    /// Reply carried an error in rpc_result
    GspError { code: u32, function: u32 },
}

/// Frame a command into a CMDQ ring slot. Layout: RpcHeader at offset 0,
/// then the payload bytes. Returns the on-wire length (header + payload)
pub fn frame_into(
    slot: &mut [u8],
    function: u32,
    sequence: u32,
    payload: &[u8],
) -> Result<usize, RpcError> {
    let total = RpcHeader::SIZE + payload.len();
    if total > slot.len() {
        return Err(RpcError::PayloadTooLarge { size: total, max: slot.len() });
    }
    let hdr = RpcHeader::new(function, payload.len() as u32, sequence);
    let hb = hdr.to_bytes();
    slot[..hb.len()].copy_from_slice(&hb);
    slot[hb.len()..hb.len() + payload.len()].copy_from_slice(payload);
    // Zero the rest so GSP doesn't see stale bytes
    for b in &mut slot[total..] { *b = 0; }
    Ok(total)
}

/// Parse a MSGQ ring slot into (header, payload-slice). Verifies header
/// version + signature + length bounds
pub fn parse_frame(slot: &[u8]) -> Result<(RpcHeader, &[u8]), RpcError> {
    let hdr = RpcHeader::from_bytes(slot).ok_or(RpcError::BadHeader)?;
    if hdr.header_version != RPC_HEADER_VERSION || hdr.signature != RPC_SIGNATURE {
        return Err(RpcError::BadHeader);
    }
    let plen = (hdr.length as usize).saturating_sub(RpcHeader::SIZE);
    let end = RpcHeader::SIZE + plen;
    if end > slot.len() { return Err(RpcError::BadHeader); }
    Ok((hdr, &slot[RpcHeader::SIZE..end]))
}

/// Wrapper that owns one 'Msgq' and a monotonic sequence counter
pub struct RpcDriver {
    msgq: Msgq,
    seq:  AtomicU32,
}

impl RpcDriver {
    pub fn new(msgq: Msgq) -> Self {
        Self { msgq, seq: AtomicU32::new(1) }
    }

    /// Sysmem phys of the shared queue region. Goes to GSP-RM via
    /// SetSystemInfo so it knows where to find us
    pub fn queue_phys(&self) -> u64 { self.msgq.phys_base() }

    pub fn queue(&self) -> &Msgq { &self.msgq }

    /// Send a command into the CMDQ. Returns the sequence number we used
    pub fn send(&mut self, function: RpcFunction, payload: &[u8]) -> Result<u32, RpcError> {
        let wp = self.msgq.cmdq_write_ptr();
        let rp = self.msgq.cmdq_read_ptr();
        let n = self.msgq.entries() as u32;
        // Full iff (wp + 1) % n == rp (one-slot-empty convention)
        if (wp + 1) % n == rp { return Err(RpcError::CmdqFull); }

        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let slot_idx = (wp as usize) % self.msgq.entries();
        let slot = self.msgq.cmdq_slot_mut(slot_idx).expect("bounded slot");
        frame_into(slot, function.as_u32(), seq, payload)?;

        // SeqCst fence inside cmdq_advance_write makes the framed bytes
        // visible to GSP before the bump is observed
        self.msgq.cmdq_advance_write((wp + 1) % n);
        Ok(seq)
    }

    /// Poll the MSGQ for the next message, if any. Returns (header,
    /// payload-copy) and advances the host read pointer
    pub fn try_recv(&mut self) -> Option<(RpcHeader, [u8; MSGQ_ELEMENT_SIZE])> {
        let rp = self.msgq.msgq_read_ptr();
        let wp = self.msgq.msgq_write_ptr();
        if rp == wp { return None; }

        let slot_idx = (rp as usize) % self.msgq.entries();
        let slot = self.msgq.msgq_slot(slot_idx)?;
        let mut copy = [0u8; MSGQ_ELEMENT_SIZE];
        copy.copy_from_slice(slot);

        let n = self.msgq.entries() as u32;
        let (hdr, _) = parse_frame(&copy).ok()?;
        self.msgq.msgq_advance_read((rp + 1) % n);
        Some((hdr, copy))
    }
}

#[derive(Copy, Clone, Debug)]
pub struct RpcSelfTestReport {
    pub seq: u32,
    pub function: u32,
    pub length: u32,
    pub frame_decoded_ok: bool,
}

/// In-process round-trip: build a command frame for SET_SYSTEM_INFO,
/// parse it back, and confirm the header/payload survive. Does NOT
/// touch the GPU - this is a sanity check for the framing helpers
pub fn self_test() -> Result<RpcSelfTestReport, RpcError> {
    let mut buf = [0u8; MSGQ_ELEMENT_SIZE];
    let payload = [0xCAu8; 64];
    let seq = 0xDEAD_BEEF;
    let n = frame_into(&mut buf, RpcFunction::SetSystemInfo.as_u32(), seq, &payload)?;
    let (hdr, body) = parse_frame(&buf[..n])?;
    let ok = hdr.sequence == seq
        && hdr.function == RpcFunction::SetSystemInfo.as_u32()
        && body.len() == payload.len()
        && body == payload;
    Ok(RpcSelfTestReport {
        seq: hdr.sequence,
        function: hdr.function,
        length: hdr.length,
        frame_decoded_ok: ok,
    })
}
