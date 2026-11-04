// Two rings in plain memory, and a stand-in for GSP-RM at the other end
//
// QEMU has no GSP, and most of what can go wrong in the transport is
// arithmetic: which slot a record starts in, how a long message is cut into
// records, what the pointer registers count, when a record is stale. `Loopback`
// is a `QueueMem` of two `Vec`s and four fake pointer registers, and `reflect`
// moves every record sent into the ring read from: GSP-RM answering each
// request with itself. `answering` does that as soon as something is sent,
// marking each record a success, so a whole `call` runs against it. The
// selftest drives the real `GspRpc` over it (`selftest.rs`, `rpc_transport`).

use alloc::vec;
use alloc::vec::Vec;

use crate::drivers::gpu::nvidia::le;

use super::queue::{
    QueueMem, QueueReg, QueueState, RxGeometry, GSP_PAGE, MSGQ_HEADER_SIZE, MSG_HDR_SIZE, NVDM_GMCAPI,
    NVDM_RM_RPC,
};

pub struct Loopback {
    count: u32,
    cmdq: Vec<u8>,
    msgq: Vec<u8>,
    /// QUEUE_HEAD, QUEUE_TAIL, MSGQ_HEAD, MSGQ_TAIL
    regs: [u32; 4],
    state: QueueState,
    /// The status queue's header, as `publish_header` left it
    header: [u8; MSGQ_HEADER_SIZE],
    /// Reflect everything the moment it is sent, as a success
    echo: bool,
}

impl Loopback {
    /// Both rings `count` slots deep, the status queue already linked
    pub fn new(count: u32) -> Self {
        let bytes = count as usize * GSP_PAGE;
        let rx = RxGeometry { entry_off: 0, msg_size: GSP_PAGE as u32, msg_count: count };
        Self {
            count,
            cmdq: vec![0; bytes],
            msgq: vec![0; bytes],
            regs: [0; 4],
            state: QueueState { rx: Some(rx), ..Default::default() },
            header: [0; MSGQ_HEADER_SIZE],
            echo: false,
        }
    }

    /// Both rings, the status queue not linked and its header not written: the
    /// moment after the boot command, before GSP-RM has come up
    pub fn unlinked(count: u32) -> Self {
        let mut q = Self::new(count);
        q.state.rx = None;
        q
    }

    /// Play GSP-RM writing its ring's header (`msgqTxCreate` on its side)
    pub fn publish_header(&mut self, major: u16, size: u32, msg_size: u32, msg_count: u32, entry_off: u32) {
        let h = &mut self.header;
        h[0..2].copy_from_slice(&major.to_le_bytes());
        le::put_u32(h, 4, size);
        le::put_u32(h, 8, msg_size);
        le::put_u32(h, 12, msg_count);
        le::put_u32(h, 16, entry_off);
    }

    /// Rings whose far end answers every record the moment it is sent, with
    /// the same record marked a success - enough for a whole `call`
    pub fn answering(count: u32) -> Self {
        Self { echo: true, ..Self::new(count) }
    }

    /// Every slot sent and not yet reflected, in order, for a test to take
    /// apart
    pub fn sent(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = self.regs[QueueReg::CmdTail as usize];
        while i != self.regs[QueueReg::CmdHead as usize] {
            out.extend_from_slice(self.slot(&self.cmdq, i));
            i = i.wrapping_add(1);
        }
        out
    }

    /// Play GSP-RM: consume everything sent and publish it, unchanged, where
    /// replies are read. Returns how many slots moved
    pub fn reflect(&mut self) -> u32 {
        self.reflect_as(false)
    }

    /// Move every record sent, whole, and if `answer` mark it a success as a
    /// reply is: an RM RPC's two result words zero, a GMCAPI command's response
    /// flag set and status zero
    fn reflect_as(&mut self, answer: bool) -> u32 {
        let mut moved = 0;
        loop {
            let tail = self.regs[QueueReg::CmdTail as usize];
            if tail == self.regs[QueueReg::CmdHead as usize] {
                break;
            }
            let size = le::u32_at(self.slot(&self.cmdq, tail), 4) as usize;
            let pages = size.div_ceil(GSP_PAGE).max(1) as u32;
            let mut record = Vec::with_capacity(pages as usize * GSP_PAGE);
            for p in 0..pages {
                record.extend_from_slice(self.slot(&self.cmdq, tail.wrapping_add(p)));
            }
            if answer {
                match (le::u32_at(&record, 12) >> 24) as u8 {
                    NVDM_RM_RPC => {
                        le::put_u32(&mut record, MSG_HDR_SIZE + 16, 0);
                        le::put_u32(&mut record, MSG_HDR_SIZE + 20, 0);
                    }
                    NVDM_GMCAPI => {
                        let c = le::u32_at(&record, MSG_HDR_SIZE);
                        le::put_u32(&mut record, MSG_HDR_SIZE, c | super::super::gmcapi::FLAG_RESPONSE);
                        le::put_u32(&mut record, MSG_HDR_SIZE + 16, 0);
                    }
                    _ => {}
                }
            }
            let head = self.regs[QueueReg::MsgHead as usize];
            for p in 0..pages {
                let at = (head.wrapping_add(p) % self.count) as usize * GSP_PAGE;
                self.msgq[at..at + GSP_PAGE].copy_from_slice(&record[p as usize * GSP_PAGE..(p as usize + 1) * GSP_PAGE]);
            }
            self.regs[QueueReg::MsgHead as usize] = head.wrapping_add(pages);
            self.regs[QueueReg::CmdTail as usize] = tail.wrapping_add(pages);
            moved += pages;
        }
        moved
    }

    /// Publish `record` (whole slots) in the status queue as GSP-RM would: a
    /// reply made by hand, for cases `reflect` cannot make
    pub fn inject(&mut self, record: &[u8]) {
        let head = self.regs[QueueReg::MsgHead as usize];
        let pages = (record.len() / GSP_PAGE) as u32;
        for p in 0..pages {
            let at = (head.wrapping_add(p) % self.count) as usize * GSP_PAGE;
            self.msgq[at..at + GSP_PAGE].copy_from_slice(&record[p as usize * GSP_PAGE..(p as usize + 1) * GSP_PAGE]);
        }
        self.regs[QueueReg::MsgHead as usize] = head.wrapping_add(pages);
    }

    /// The four pointer registers: QUEUE_HEAD, QUEUE_TAIL, MSGQ_HEAD, MSGQ_TAIL
    pub fn regs(&self) -> [u32; 4] {
        self.regs
    }

    /// Step the status queue's read counter back `pages` slots, so a record
    /// already read, marked SEEN, arrives again: a stale record
    pub fn replay(&mut self, pages: u32) {
        self.state.rx_read = self.state.rx_read.wrapping_sub(pages);
        self.regs[QueueReg::MsgTail as usize] = self.state.rx_read;
    }

    /// Flip one byte of the next record waiting to be read, `offset` bytes into
    /// it: a record damaged on the way
    pub fn damage(&mut self, offset: usize) {
        let at = (self.state.rx_read % self.count) as usize * GSP_PAGE + offset;
        self.msgq[at] ^= 0xff;
    }

    /// The magic of the record `back` records before the read counter
    pub fn magic_behind(&self, back: u32) -> u32 {
        le::u32_at(self.slot(&self.msgq, self.state.rx_read.wrapping_sub(back)), 0)
    }

    fn slot<'a>(&self, ring: &'a [u8], idx: u32) -> &'a [u8] {
        let at = (idx % self.count) as usize * GSP_PAGE;
        &ring[at..at + GSP_PAGE]
    }
}

impl QueueMem for Loopback {
    fn cmdq_count(&self) -> u32 { self.count }
    fn cmdq_slot_mut(&mut self, slot: u32) -> &mut [u8] {
        let at = (slot % self.count) as usize * GSP_PAGE;
        &mut self.cmdq[at..at + GSP_PAGE]
    }
    fn msgq_header(&self) -> [u8; MSGQ_HEADER_SIZE] { self.header }
    fn msgq_size(&self) -> u32 { self.count * GSP_PAGE as u32 }
    fn msgq_slot(&self, _rx: RxGeometry, slot: u32) -> &[u8] { self.slot(&self.msgq, slot) }
    fn msgq_slot_mut(&mut self, _rx: RxGeometry, slot: u32) -> &mut [u8] {
        let at = (slot % self.count) as usize * GSP_PAGE;
        &mut self.msgq[at..at + GSP_PAGE]
    }
    fn state(&self) -> &QueueState { &self.state }
    fn state_mut(&mut self) -> &mut QueueState { &mut self.state }
    fn fake_reg(&self, r: QueueReg) -> Option<u32> { Some(self.regs[r as usize]) }
    fn set_fake_reg(&mut self, r: QueueReg, v: u32) -> bool {
        self.regs[r as usize] = v;
        if r == QueueReg::CmdHead && self.echo {
            self.reflect_as(true);
        }
        true
    }
}
