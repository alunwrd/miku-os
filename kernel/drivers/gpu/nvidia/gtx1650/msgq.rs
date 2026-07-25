// GSP-RM command/message queue (CMDQ/MSGQ) infrastructure for TU116
//
// Once GSP-RM has booted from WPR2 it talks to the host through two
// ring buffers backed by shared sysmem:
//
//   CMDQ   host writes commands here, GSP reads them (host-producer)
//   MSGQ   GSP writes events/responses here, host reads them (gsp-producer)
//
// Layout (per nouveau drivers/gpu/drm/nouveau/nvkm/subdev/gsp/r535.c and
// nvkm_gsp_msgq_init):
//
//   page 0:                   CMDQ TxHeader (host-owned) + MSGQ RxHeader
//   page 1 .. 1+N:            CMDQ data ring (N entries, 0x1000 each)
//   page 1+N:                 MSGQ TxHeader (gsp-owned)  + CMDQ RxHeader
//   page 2+N .. 2+2N:         MSGQ data ring (N entries, 0x1000 each)
//
// With N = 63 the total is 128 pages = 512 KiB, one phys-contiguous
// allocation. Element size is one page (0x1000) which matches r535's
// GSP_MSG_QUEUE_ELEMENT_ALIGN_4K. Every message starts with a
// 'GspRpcHeader' (see gsprm.rs) followed by a function-specific payload
//
// The host hands the sysmem base of this region to GSP-RM via the
// SetSystemInfo / SET_REGISTRY RPCs after boot. This module owns the
// allocation, lays out the headers, and exposes typed accessors for the
// write/read pointers and ring-data slots. RPC framing lives in rpc.rs

#![allow(dead_code)]

use core::sync::atomic::{fence, Ordering};

use super::dma_buf::{DmaBuffer, DmaBufError};

/// Each msgq element is one 4 KiB page. The RPC header + payload must
/// fit within this; r535 keeps the same convention
pub const MSGQ_ELEMENT_SIZE: usize = 0x1000;

/// Default ring depth per direction (matches nouveau / r535)
pub const MSGQ_DEFAULT_ENTRIES: usize = 63;

/// Header page size (TxHeader + sibling RxHeader live in one 4K page)
pub const MSGQ_HDR_PAGE: usize = 0x1000;

const PAGE_SIZE: usize = 4096;

/// Producer-side header, host or gsp depending on direction. Layout
/// matches r535's msgqTxHeader (32 bytes)
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct MsgqTxHeader {
    pub version:    u32,
    pub size:       u32,
    pub msg_size:   u32,
    pub msg_count:  u32,
    pub write_ptr:  u32,
    pub flags:      u32,
    pub rx_hdr_off: u32,
    pub entry_off:  u32,
}

impl MsgqTxHeader {
    pub const SIZE: usize = 32;

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        let words = [
            self.version, self.size, self.msg_size, self.msg_count,
            self.write_ptr, self.flags, self.rx_hdr_off, self.entry_off,
        ];
        for (i, w) in words.iter().enumerate() {
            out[i*4..i*4+4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }
}

/// Consumer-side header (just a read pointer)
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct MsgqRxHeader {
    pub read_ptr: u32,
}

impl MsgqRxHeader {
    pub const SIZE: usize = 4;

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        self.read_ptr.to_le_bytes()
    }
}

#[derive(Debug, Copy, Clone)]
pub enum MsgqError {
    Alloc(DmaBufError),
}
impl From<DmaBufError> for MsgqError {
    fn from(e: DmaBufError) -> Self { MsgqError::Alloc(e) }
}

/// Owns the shared sysmem region for one direction-pair. The byte
/// offsets of every header and ring-data slot are precomputed at alloc
/// time so the volatile producer/consumer steps are simple
pub struct Msgq {
    /// Shared sysmem allocation backing both rings
    buf: DmaBuffer,
    /// Number of data entries per ring (same for CMDQ and MSGQ)
    entries: usize,

    // Byte offsets within 'buf' of the four headers and two ring bases.
    // Producer/consumer code consults these instead of recomputing
    cmdq_tx_off: usize,
    msgq_rx_off: usize,
    cmdq_data_off: usize,
    msgq_tx_off: usize,
    cmdq_rx_off: usize,
    msgq_data_off: usize,
}

impl Msgq {
    /// Allocate the shared region with 'entries' data slots per ring.
    /// Total pages = 2 header pages + 2 * entries data pages. With the
    /// default 'MSGQ_DEFAULT_ENTRIES = 63' that's 128 pages = 512 KiB
    pub fn alloc(entries: usize) -> Result<Self, MsgqError> {
        let entries = entries.max(1);
        let pages = 2 + 2 * entries;
        let mut buf = DmaBuffer::alloc(pages)?;
        buf.zero();

        let cmdq_tx_off   = 0;
        let msgq_rx_off   = MsgqTxHeader::SIZE; // adjacent in the header page
        let cmdq_data_off = MSGQ_HDR_PAGE;
        let msgq_tx_off   = MSGQ_HDR_PAGE + entries * MSGQ_ELEMENT_SIZE;
        let cmdq_rx_off   = msgq_tx_off + MsgqTxHeader::SIZE;
        let msgq_data_off = msgq_tx_off + MSGQ_HDR_PAGE;

        let mut q = Self {
            buf, entries,
            cmdq_tx_off, msgq_rx_off, cmdq_data_off,
            msgq_tx_off, cmdq_rx_off, msgq_data_off,
        };

        q.write_initial_headers();
        DmaBuffer::write_barrier();
        Ok(q)
    }

    /// Write the host-side header initial state. The MsgqTxHeader on the
    /// MSGQ side is GSP-owned (it fills it in on boot), so we leave it
    /// zeroed. write_ptr starts at zero everywhere
    fn write_initial_headers(&mut self) {
        let entries = self.entries as u32;
        let ring_size = entries * (MSGQ_ELEMENT_SIZE as u32);

        let cmdq_tx = MsgqTxHeader {
            version:    0,
            size:       ring_size,
            msg_size:   MSGQ_ELEMENT_SIZE as u32,
            msg_count:  entries,
            write_ptr:  0,
            flags:      1,
            rx_hdr_off: self.cmdq_rx_off as u32,
            entry_off:  self.cmdq_data_off as u32,
        };
        let bytes = cmdq_tx.to_bytes();
        self.buf.as_mut_slice()[self.cmdq_tx_off..self.cmdq_tx_off + bytes.len()]
            .copy_from_slice(&bytes);

        let msgq_rx = MsgqRxHeader { read_ptr: 0 };
        let bytes = msgq_rx.to_bytes();
        self.buf.as_mut_slice()[self.msgq_rx_off..self.msgq_rx_off + bytes.len()]
            .copy_from_slice(&bytes);
    }

    /// Sysmem phys address of the whole region (basis for all offsets)
    #[inline] pub fn phys_base(&self) -> u64 { self.buf.phys() }

    #[inline] pub fn cmdq_tx_phys(&self)   -> u64 { self.buf.phys() + self.cmdq_tx_off   as u64 }
    #[inline] pub fn cmdq_rx_phys(&self)   -> u64 { self.buf.phys() + self.cmdq_rx_off   as u64 }
    #[inline] pub fn msgq_tx_phys(&self)   -> u64 { self.buf.phys() + self.msgq_tx_off   as u64 }
    #[inline] pub fn msgq_rx_phys(&self)   -> u64 { self.buf.phys() + self.msgq_rx_off   as u64 }
    #[inline] pub fn cmdq_data_phys(&self) -> u64 { self.buf.phys() + self.cmdq_data_off as u64 }
    #[inline] pub fn msgq_data_phys(&self) -> u64 { self.buf.phys() + self.msgq_data_off as u64 }

    #[inline] pub fn entries(&self) -> usize { self.entries }
    #[inline] pub fn size(&self)    -> usize { self.buf.size() }

    /// Host-producer write pointer
    pub fn cmdq_write_ptr(&self) -> u32 {
        let s = self.buf.as_slice();
        let off = self.cmdq_tx_off + 16;
        u32::from_le_bytes(s[off..off + 4].try_into().unwrap())
    }
    /// gsp-consumer read pointer
    pub fn cmdq_read_ptr(&self) -> u32 {
        let s = self.buf.as_slice();
        let off = self.cmdq_rx_off;
        u32::from_le_bytes(s[off..off + 4].try_into().unwrap())
    }
    /// gsp-producer write pointer
    pub fn msgq_write_ptr(&self) -> u32 {
        let s = self.buf.as_slice();
        let off = self.msgq_tx_off + 16;
        u32::from_le_bytes(s[off..off + 4].try_into().unwrap())
    }
    /// host-consumer read pointer
    pub fn msgq_read_ptr(&self) -> u32 {
        let s = self.buf.as_slice();
        let off = self.msgq_rx_off;
        u32::from_le_bytes(s[off..off + 4].try_into().unwrap())
    }

    /// Advance the host write pointer after staging a command. SeqCst
    /// fence around the store so GSP sees a consistent ring slot before
    /// it sees the pointer bump
    pub fn cmdq_advance_write(&mut self, new_ptr: u32) {
        fence(Ordering::SeqCst);
        let off = self.cmdq_tx_off + 16;
        self.buf.as_mut_slice()[off..off + 4].copy_from_slice(&new_ptr.to_le_bytes());
        fence(Ordering::SeqCst);
    }

    /// Advance the host read pointer after consuming a message
    pub fn msgq_advance_read(&mut self, new_ptr: u32) {
        fence(Ordering::SeqCst);
        let off = self.msgq_rx_off;
        self.buf.as_mut_slice()[off..off + 4].copy_from_slice(&new_ptr.to_le_bytes());
        fence(Ordering::SeqCst);
    }

    /// Mutable byte slice for the i-th slot of the CMDQ data ring
    pub fn cmdq_slot_mut(&mut self, i: usize) -> Option<&mut [u8]> {
        if i >= self.entries { return None; }
        let start = self.cmdq_data_off + i * MSGQ_ELEMENT_SIZE;
        Some(&mut self.buf.as_mut_slice()[start..start + MSGQ_ELEMENT_SIZE])
    }

    /// Read-only slice for the i-th slot of the MSGQ data ring
    pub fn msgq_slot(&self, i: usize) -> Option<&[u8]> {
        if i >= self.entries { return None; }
        let start = self.msgq_data_off + i * MSGQ_ELEMENT_SIZE;
        Some(&self.buf.as_slice()[start..start + MSGQ_ELEMENT_SIZE])
    }
}

#[derive(Copy, Clone, Debug)]
pub struct MsgqSelfTestReport {
    pub phys_base: u64,
    pub entries: usize,
    pub size: usize,
    pub cmdq_tx_phys: u64,
    pub cmdq_rx_phys: u64,
    pub msgq_tx_phys: u64,
    pub msgq_rx_phys: u64,
    pub cmdq_data_phys: u64,
    pub msgq_data_phys: u64,
    pub ok: bool,
}

/// Allocate a queue pair, verify offsets line up to page boundaries, and
/// that the pointers round-trip. Useful as a sanity check before GSP-RM
/// is alive to talk back. Frees all sysmem on return
pub fn self_test() -> Result<MsgqSelfTestReport, MsgqError> {
    let mut q = Msgq::alloc(MSGQ_DEFAULT_ENTRIES)?;

    q.cmdq_advance_write(42);
    let wp = q.cmdq_write_ptr();

    let ok = wp == 42
        && q.cmdq_tx_phys() & 0xFFF == 0
        && q.cmdq_data_phys() & 0xFFF == 0
        && q.msgq_tx_phys() & 0xFFF == 0
        && q.msgq_data_phys() & 0xFFF == 0
        && q.cmdq_data_phys() == q.phys_base() + MSGQ_HDR_PAGE as u64
        && q.msgq_data_phys() == q.msgq_tx_phys() + MSGQ_HDR_PAGE as u64;

    Ok(MsgqSelfTestReport {
        phys_base: q.phys_base(),
        entries: q.entries(),
        size: q.size(),
        cmdq_tx_phys:   q.cmdq_tx_phys(),
        cmdq_rx_phys:   q.cmdq_rx_phys(),
        msgq_tx_phys:   q.msgq_tx_phys(),
        msgq_rx_phys:   q.msgq_rx_phys(),
        cmdq_data_phys: q.cmdq_data_phys(),
        msgq_data_phys: q.msgq_data_phys(),
        ok,
    })
}
