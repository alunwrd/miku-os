// The payloads of the RPCs and events this driver builds or reads. Layouts only

use alloc::vec::Vec;

use crate::drivers::gpu::nvidia::le;

/// `rpc_post_event_v17_00`: the envelope of every RM event notification.
///
/// `notify_index` says which, and `event_data` is its own payload. Offsets
/// confirmed by compiling the C, and the tail is not 4-aligned: `bNotifyList`
/// is a byte at 28, so the data starts at 29
#[derive(Debug)]
pub struct PostEvent<'a> {
    /// Which client and which event object the notification is for. Nothing
    /// routes on them: `notify_index` tells the two subscriptions apart
    #[allow(dead_code)]
    pub h_client: u32,
    #[allow(dead_code)]
    pub h_event: u32,
    /// The notifier id, masked out of `NV01_EVENT_CLIENT_RM | id`
    pub notify_index: u32,
    #[allow(dead_code)]
    pub status: u32,
    pub event_data: &'a [u8],
}

/// `NV01_EVENT_CLIENT_RM`, the flag RM's own notifications carry in
/// `notifyIndex`
pub const NV01_EVENT_CLIENT_RM: u32 = 0x0400_0000;

impl<'a> PostEvent<'a> {
    pub const HEADER: usize = 29;

    pub fn parse(payload: &'a [u8]) -> Option<Self> {
        if payload.len() < Self::HEADER {
            return None;
        }
        let size = crate::drivers::gpu::nvidia::le::u32_or0(payload, 24) as usize;
        // Clamped: `eventDataSize` is a number GSP-RM put in a buffer, and
        // every length in this transport is checked against what arrived
        let end = (Self::HEADER + size).min(payload.len());
        Some(Self {
            h_client: crate::drivers::gpu::nvidia::le::u32_or0(payload, 0),
            h_event: crate::drivers::gpu::nvidia::le::u32_or0(payload, 4),
            notify_index: crate::drivers::gpu::nvidia::le::u32_or0(payload, 8)
                & !NV01_EVENT_CLIENT_RM,
            status: crate::drivers::gpu::nvidia::le::u32_or0(payload, 20),
            event_data: &payload[Self::HEADER..end],
        })
    }
}

/// Printable runs of at least 'min_len' characters, in order. GSP-RM's
/// diagnostic payloads (NOCAT records, libos print buffers, Xid strings) are
/// token streams with ASCII embedded
pub fn ascii_runs(bytes: &[u8], min_len: usize) -> Vec<alloc::string::String> {
    let mut out = Vec::new();
    let mut run = alloc::string::String::new();
    for &b in bytes.iter().chain(core::iter::once(&0u8)) {
        if (0x20..0x7f).contains(&b) {
            run.push(b as char);
        } else {
            if run.len() >= min_len {
                out.push(core::mem::take(&mut run));
            }
            run.clear();
        }
    }
    out
}

/// `rpc_os_error_log_v17_00`, 615: u32 exceptType, u32
/// preemptiveRemovalPreviousXid, u32 runlistId, u32 chid, char errString[0x100]
/// (offsets compiled from the header). This is the Xid, the most informative
/// thing GSP-RM emits when its init fails. Returns
/// `(exceptType, chid, errString)`; an Xid naming a channel means that channel
/// is gone
pub fn os_error_log(payload: &[u8]) -> (u32, u32, alloc::string::String) {
    if payload.len() < 16 {
        return (0, 0, alloc::string::String::new());
    }
    let except_type = le::u32_at(payload, 0);
    let chid = le::u32_at(payload, 12);
    let text = payload[16..]
        .iter()
        .take_while(|&&b| b != 0)
        .filter(|&&b| (0x20..0x7f).contains(&b))
        .map(|&b| b as char)
        .collect();
    (except_type, chid, text)
}

// GSP_RC_TRIGGERED payload.
//
// `rpc_rc_triggered_v17_02`: the robust-channel report GSP-RM posts when it
// kills a channel. 64 bytes in 615 (the channel's instance block at the end).
// It names which channel died and why.
//
//   u32 nv2080EngineType@0   u32 chid@4        u32 gfid@8
//   u32 exceptLevel@0xc      u32 exceptType@0x10
//   u32 scope@0x14           u16 partitionAttributionId@0x18
//   u32 mmuFaultAddrLo@0x1c  u32 mmuFaultAddrHi@0x20
//   u32 mmuFaultType@0x24
//
// Shared by `blit_postmortem` and the live event handler.

/// One robust-channel report from GSP-RM
#[derive(Debug, Clone, Copy)]
pub struct RcRecord {
    pub engine: u32,
    pub chid: u32,
    pub except_level: u32,
    pub except_type: u32,
    pub scope: u32,
    /// The faulting address, when an MMU fault raised this RC. Zero otherwise,
    /// also a legal address; `is_mmu_fault` is the test
    pub fault_addr: u64,
    pub fault_type: u32,
}

impl RcRecord {
    /// The fixed part of the record. A shorter payload is a record this driver
    /// does not know
    pub const SIZE: usize = 0x28;

    pub fn parse(payload: &[u8]) -> Option<Self> {
        if payload.len() < Self::SIZE {
            return None;
        }
        Some(Self {
            engine: le::u32_at(payload, 0),
            chid: le::u32_at(payload, 4),
            except_level: le::u32_at(payload, 0x0c),
            except_type: le::u32_at(payload, 0x10),
            scope: le::u32_at(payload, 0x14),
            fault_addr: ((le::u32_at(payload, 0x20) as u64) << 32)
                | le::u32_at(payload, 0x1c) as u64,
            fault_type: le::u32_at(payload, 0x24),
        })
    }

    /// Did the MMU raise this? A fault type of zero with no address is a
    /// non-fault RC (a timeout, a method error)
    pub fn is_mmu_fault(&self) -> bool {
        self.fault_type != 0 || self.fault_addr != 0
    }

    /// One line, shared by the live handler and the postmortem
    pub fn describe(&self) -> alloc::string::String {
        alloc::format!(
            "RC engine {:#x} chid {} exceptLevel {:#x} exceptType {:#x} scope {} \
             fault {:#018x} type {:#x}",
            self.engine, self.chid, self.except_level, self.except_type,
            self.scope, self.fault_addr, self.fault_type
        )
    }
}
// GSP_POST_NOCAT_RECORD payload
//
// rpc_gsp_post_nocat_record_v01_00 is a single u32 'data' field that is the
// head of an NV2080CtrlNocatJournalInsertRecord (ctrl2080nvd.h), so the record
// starts at payload offset 0:
//
//   u32 flags@0  u64 timestamp@8  u8 recType@16  u32 bugcheck@20
//   char source[65]@24  u32 subsystem@92  u64 errorCode@96
//   char faultingEngine[65]@104  u32 tdrReason@172
//   u32 diagBufferLen@176  u8 diagBuffer[1024]@180        (1208 bytes)
//
// Scraping ASCII from the raw payload also finds every unrelated byte run in
// the 1 KiB diagnostic buffer

/// NV2080_NOCAT_JOURNAL_REC_TYPE_*
pub const NOCAT_REC_TYPE: [&str; 7] = [
    "UNKNOWN", "BUGCHECK", "ENGINE", "TDR", "RC", "ASSERT", "ANY",
];

pub const NOCAT_REC_TYPE_ASSERT: u8 = 5;

/// RM_NOCAT_ASSERT_DIAG_BUFFER (g_journal_nvoc.h): u32 callStack[10] plus u32
/// count, 44 bytes. Every entry is relative to the GSP-RM load address, so the
/// values are image offsets
pub const ASSERT_CALL_STACK_SIZE: usize = 10;
pub const ASSERT_DIAG_BUFFER_SIZE: usize = ASSERT_CALL_STACK_SIZE * 4 + 4;

pub struct NocatRecord {
    pub rec_type: u8,
    pub bugcheck: u32,
    pub subsystem: u32,
    pub error_code: u64,
    pub source: alloc::string::String,
    pub faulting_engine: alloc::string::String,
    pub diag_len: u32,
    /// diagBuffer clamped to diagBufferLen and to what the element carried
    pub diag: alloc::vec::Vec<u8>,
}

impl NocatRecord {
    pub fn rec_type_name(&self) -> &'static str {
        NOCAT_REC_TYPE
            .get(self.rec_type as usize)
            .copied()
            .unwrap_or("?")
    }

    pub fn is_assert(&self) -> bool {
        self.rec_type == NOCAT_REC_TYPE_ASSERT
    }

    /// _rcdbNocatReportAssert packs the assert site into errorCode:
    ///   low 32 bits  = breakpointAddrHint - loadAddress (image offset)
    ///   high 32 bits = source line number
    /// so the pair identifies the failing NV_ASSERT with no symbols. Only on an
    /// ASSERT record
    pub fn assert_site(&self) -> Option<(u32, u32)> {
        self.is_assert().then(|| {
            (
                (self.error_code & 0xFFFF_FFFF) as u32,
                (self.error_code >> 32) as u32,
            )
        })
    }

    /// Call stack from the assert diag buffer: image offsets, innermost first,
    /// trailing zero slots dropped. Returns (frames, repeat_count)
    pub fn assert_call_stack(&self) -> Option<(alloc::vec::Vec<u32>, u32)> {
        if !self.is_assert() || self.diag.len() < ASSERT_DIAG_BUFFER_SIZE {
            return None;
        }
        let mut frames = alloc::vec::Vec::new();
        for i in 0..ASSERT_CALL_STACK_SIZE {
            let v = le::u32_at(&self.diag, i * 4);
            if v == 0 {
                break;
            }
            frames.push(v);
        }
        Some((frames, le::u32_at(&self.diag, ASSERT_CALL_STACK_SIZE * 4)))
    }

    /// True when the diag buffer is a binary blob, not text. An assert's call
    /// stack yields no useful ASCII, and past diagBufferLen lie stale firmware
    /// strings
    pub fn diag_is_binary(&self) -> bool {
        self.is_assert()
    }

    /// `(step, status)` when this is GSP-RM's report on a display step, the
    /// ENGINE record the card posts at boot and at every unload.
    ///
    /// Nothing in OpenRM posts it; it was read from the 570.144 image
    /// (`dev/display-nvkms.md`, decision 7), not looked for in 615's, and a
    /// record of another shape is left `None`. One function in GSP-RM takes a
    /// step code and a value and encodes them with RM's protobuf encoder
    /// (`prbEnc*`) as `{328: {4: step, 5: value}}`. For step 0 and 0x25-0x2d it
    /// also posts this record: source "Display Subsystem", engine "Display
    /// Engine", errorCode = the step, the encoding as the diagnostic buffer.
    /// Every caller passes the step's NV_STATUS as the value: 0x27, 0x28 and
    /// 0x2d at the end of a step whether it worked or not, 0x29 and 0x2a-0x2c
    /// after one failed. So the record is a report, and only a non-zero status
    /// is a complaint.
    ///
    /// The other display reporter ("Display subsystem", lower-case s, Xid-like
    /// codes such as 0x73, a larger encoding) is not this shape and stays
    /// `None`, as does anything not decoding to these two fields with the step
    /// equal to errorCode
    pub fn display_step(&self) -> Option<(u32, u32)> {
        if self.rec_type != NOCAT_REC_TYPE_ENGINE || self.source != "Display Subsystem" {
            return None;
        }
        let b = &self.diag;
        let mut i = 0;
        let key = prb_varint(b, &mut i)?;
        if key & 7 != 2 {
            return None;
        }
        let len = prb_varint(b, &mut i)? as usize;
        if i.checked_add(len)? != b.len() {
            return None;
        }
        let (mut step, mut status) = (None, None);
        while i < b.len() {
            let key = prb_varint(b, &mut i)?;
            if key & 7 != 0 {
                return None;
            }
            let v = u32::try_from(prb_varint(b, &mut i)?).ok()?;
            match key >> 3 {
                4 => step = Some(v),
                5 => status = Some(v),
                _ => return None,
            }
        }
        let step = step?;
        (u64::from(step) == self.error_code).then_some((step, status?))
    }
}

/// `NV2080_NOCAT_JOURNAL_REC_TYPE_ENGINE`
pub const NOCAT_REC_TYPE_ENGINE: u8 = 2;

/// One protobuf varint at `*i`, advancing it
fn prb_varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*i)?;
        *i += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// NUL-terminated, printable-only string out of a fixed-size field
fn fixed_str(bytes: &[u8], off: usize, len: usize) -> alloc::string::String {
    if off >= bytes.len() {
        return alloc::string::String::new();
    }
    let end = (off + len).min(bytes.len());
    bytes[off..end]
        .iter()
        .take_while(|&&b| b != 0)
        .filter(|&&b| (0x20..0x7f).contains(&b))
        .map(|&b| b as char)
        .collect()
}

/// Decode one GSP_POST_NOCAT_RECORD payload. None when the element is too short
/// for the fixed part
pub fn nocat_record(payload: &[u8]) -> Option<NocatRecord> {
    if payload.len() < 180 {
        return None;
    }
    let diag_len = le::u32_at(payload, 176);
    let avail = payload.len() - 180;
    let take = (diag_len as usize).min(avail).min(1024);
    Some(NocatRecord {
        rec_type: payload[16],
        bugcheck: le::u32_at(payload, 20),
        subsystem: le::u32_at(payload, 92),
        error_code: u64::from_le_bytes(payload[96..104].try_into().ok()?),
        source: fixed_str(payload, 24, 65),
        faulting_engine: fixed_str(payload, 104, 65),
        diag_len,
        diag: payload[180..180 + take].to_vec(),
    })
}

/// rpc_ucode_libos_print_v1e_08: u32 ucodeEngDesc, u32 libos_print_buf_size,
/// u8 libos_print_buf[]. Returns the buffer, bounded by the payload
pub fn libos_print_buf(payload: &[u8]) -> &[u8] {
    if payload.len() < 8 {
        return &[];
    }
    let len = (le::u32_at(payload, 4) as usize).min(payload.len() - 8);
    &payload[8..8 + len]
}
