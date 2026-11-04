// Reading a monitor's EDID
//
// A display describes itself in 128 bytes: who made it, what it is called, and
// the timings it can be driven at. This decodes that block with no device, no
// bus, no allocation, since the bytes arrive over DDC from a real panel, out of
// a GSP RPC on the NVIDIA path, and from a memory window on QEMU's display.
//
// What "best" means
//
// EDID puts the panel's native timing first, but a 144 Hz monitor commonly
// lists its native resolution at 60 Hz first, with the fast timings in the
// descriptors after it. So modes are ranked: most pixels first, and between two
// of the same size the faster one.
//
// Where the timings come from
//
// Detailed timings (the base block, a CTA-861 extension, a DisplayID
// extension's Type I and Type VII blocks) carry everything a display engine
// needs. The rest name a mode and leave the porches to a table: CTA-861 video
// codes (`edid/cea_vic.rs`), and the base block's established and standard
// timings, which are VESA DMT modes (`edid/dmt.rs`). Both tables are generated
// from OpenRM's, and a named mode is added only when no detailed timing
// describes the same format: a VIC for 1080p at 59.94 Hz is the same mode as
// the panel's own 1080p60 (CTA-861-E, note 3). A standard timing not in DMT
// would need CVT or GTF arithmetic (`NvTiming_CalcCVT`) and is skipped.
//
// Reference: VESA E-EDID 1.4, sections 3.6 and 3.10; OpenRM 570.144
// `src/common/modeset/timing` (`nvt_edid.c`, `nvt_edidext_861.c`,
// `nvt_displayid20.c`, `nvt_edidext_displayid.c`)

mod cea_vic;
mod dmt;

/// One timing, in the terms EDID and DRM both use: active area, where the sync
/// pulse sits inside the blanking, and the total
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timing {
    /// Pixel clock in kHz
    pub clock_khz: u32,
    pub h_display: u32,
    pub h_sync_start: u32,
    pub h_sync_end: u32,
    pub h_total: u32,
    pub v_display: u32,
    pub v_sync_start: u32,
    pub v_sync_end: u32,
    pub v_total: u32,
    /// Sync polarities, negative-true when set
    pub nhsync: bool,
    pub nvsync: bool,
    pub interlace: bool,
}

impl Timing {
    /// Refresh rate in hundredths of a hertz, so 240 Hz reads as 24000 and a
    /// 59.94 Hz mode does not round to 60
    pub fn refresh_centihz(&self) -> u32 {
        let total = self.h_total as u64 * self.v_total as u64;
        if total == 0 {
            return 0;
        }
        ((self.clock_khz as u64 * 1000 * 100) / total) as u32
    }

    pub fn pixels(&self) -> u64 {
        self.h_display as u64 * self.v_display as u64
    }

    /// Parse one 18-byte detailed timing descriptor.
    ///
    /// The fields are packed into shared high-nibble bytes to fit eighteen. A
    /// descriptor with a zero pixel clock is a display descriptor, carrying the
    /// name or the range limits
    pub fn from_descriptor(d: &[u8]) -> Option<Timing> {
        if d.len() < 18 {
            return None;
        }
        let clock_khz = (u16::from_le_bytes([d[0], d[1]]) as u32) * 10;
        if clock_khz == 0 {
            return None;
        }
        let h_display = ((d[4] as u32 & 0xf0) << 4) | d[2] as u32;
        let h_blank = ((d[4] as u32 & 0x0f) << 8) | d[3] as u32;
        let v_display = ((d[7] as u32 & 0xf0) << 4) | d[5] as u32;
        let v_blank = ((d[7] as u32 & 0x0f) << 8) | d[6] as u32;
        // Front porch and pulse width; the two high bits of each live in d[11]
        let h_sync_off = ((d[11] as u32 & 0xc0) << 2) | d[8] as u32;
        let h_sync_width = ((d[11] as u32 & 0x30) << 4) | d[9] as u32;
        let v_sync_off = ((d[11] as u32 & 0x0c) << 2) | ((d[10] as u32 & 0xf0) >> 4);
        let v_sync_width = ((d[11] as u32 & 0x03) << 4) | (d[10] as u32 & 0x0f);
        if h_display == 0 || v_display == 0 || h_blank == 0 || v_blank == 0 {
            return None;
        }
        // Flags. Bits 4:3 == 0b11 means digital separate sync, the only kind a
        // DP or HDMI panel uses, and only then do bits 2 and 1 carry vertical
        // and horizontal polarity
        let flags = d[17];
        let separate = flags & 0x18 == 0x18;
        Some(Timing {
            clock_khz,
            h_display,
            h_sync_start: h_display + h_sync_off,
            h_sync_end: h_display + h_sync_off + h_sync_width,
            h_total: h_display + h_blank,
            v_display,
            v_sync_start: v_display + v_sync_off,
            v_sync_end: v_display + v_sync_off + v_sync_width,
            v_total: v_display + v_blank,
            nhsync: separate && flags & 0x02 == 0,
            nvsync: separate && flags & 0x04 == 0,
            interlace: flags & 0x80 != 0,
        })
    }
}

/// Timings we remember. The detailed ones first (the base block's four, six per
/// CTA-861 extension, twelve per DisplayID block), then the named ones: a CTA
/// video block's codes, eight standard timings, sixteen established ones. Past
/// this the rest are dropped
pub const MAX_MODES: usize = 48;

/// An extension block's tag, byte 0
const EXT_TAG_CEA_861: u8 = 0x02;
const EXT_TAG_DISPLAYID: u8 = 0x70;

/// Data block tags, in the top three bits of a block's first byte
const CEA_TAG_VIDEO: u8 = 2;
const CEA_TAG_VENDOR: u8 = 3;
const CEA_TAG_EXTENDED: u8 = 7;
/// Extended tags, the block's second byte
const CEA_EXT_COLORIMETRY: u8 = 5;
const CEA_EXT_HDR_STATIC: u8 = 6;

/// DisplayID data block tags: 1.3's Type I detailed timing, 2.0's Type VII
const DID_TYPE_I_TIMING: u8 = 0x03;
const DID_TYPE_VII_TIMING: u8 = 0x22;

/// The most short video descriptors kept from the CTA blocks
const MAX_SVDS: usize = 64;

/// What the sink says about HDR: the CTA-861 HDR static metadata block
/// (extended tag 6). Kept as the codes it sent
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HdrStatic {
    pub present: bool,
    /// ET_0..: bit 0 traditional SDR gamma, 1 traditional HDR gamma, 2 SMPTE
    /// ST 2084 (PQ), 3 HLG
    pub eotf: u8,
    /// SM_0: static metadata descriptor type 1
    pub metadata_types: u8,
    /// Desired content max luminance, max frame-average and min luminance, as
    /// coded; 0 when the block stopped before them
    pub max_luminance: u8,
    pub max_frame_avg: u8,
    pub min_luminance: u8,
}

/// `2^(k/32)` in millionths, `k` = 0..31: the fractional half of the CTA
/// luminance code, `50 * 2^(code/32)` cd/m^2
const POW2_32NDS: [u32; 32] = [
    1_000_000, 1_021_897, 1_044_274, 1_067_140, 1_090_508, 1_114_387, 1_138_789, 1_163_725,
    1_189_207, 1_215_247, 1_241_858, 1_269_051, 1_296_840, 1_325_237, 1_354_256, 1_383_910,
    1_414_214, 1_445_181, 1_476_826, 1_509_164, 1_542_211, 1_575_981, 1_610_490, 1_645_755,
    1_681_793, 1_718_619, 1_756_252, 1_794_709, 1_834_008, 1_874_168, 1_915_207, 1_957_144,
];

impl HdrStatic {
    /// A luminance code in cd/m^2: `50 * 2^(code/32)`
    pub fn code_to_nits(code: u8) -> u32 {
        let whole = 50u64 << (code >> 5);
        (whole * POW2_32NDS[(code & 31) as usize] as u64 / 1_000_000) as u32
    }

    pub fn max_nits(&self) -> u32 {
        if self.max_luminance == 0 { 0 } else { Self::code_to_nits(self.max_luminance) }
    }

    /// `max * (code / 255)^2 / 100`, in thousandths of a cd/m^2
    pub fn min_millinits(&self) -> u32 {
        let c = self.min_luminance as u64;
        (self.max_nits() as u64 * 1000 * c * c / (255 * 255) / 100) as u32
    }
}

/// IEEE registration ids, little-endian in the first three payload bytes of a
/// vendor block. `nvtiming.h:883`
const IEEE_HDMI_LLC: u32 = 0x00_0C03;
const IEEE_HDMI_FORUM: u32 = 0xC4_5DD8;

/// What the sink says it can do over HDMI.
///
/// All of it comes from vendor blocks in a CEA-861 extension, so a monitor with
/// no extension says none of it, and `hdmi` false then means "did not say", not
/// "is DVI". The driver asks the board as well before it turns HDMI on
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HdmiCaps {
    /// An HDMI 1.4 vendor block was present: the sink says it is an HDMI
    /// receiver
    pub hdmi: bool,
    /// Highest TMDS clock the sink will take, in kHz. The larger of what the
    /// HDMI 1.4 and HDMI Forum blocks said; zero when neither did
    pub max_tmds_khz: u32,
    /// The sink implements SCDC, so the source may talk to it above 340 MHz
    pub scdc: bool,
    /// It also accepts scrambling below 340 MHz, separate from descrambling
    /// above it
    pub scramble_below_340: bool,
}

/// What one display said about itself
#[derive(Clone, Copy)]
pub struct Edid {
    /// The three-letter PnP id, as letters
    pub manufacturer: [u8; 3],
    pub product: u16,
    pub serial: u32,
    /// The model name from descriptor type 0xFC, if it has one
    pub name: [u8; 13],
    pub name_len: usize,
    /// Detailed timings, in the order the monitor listed them. The first is
    /// what EDID calls preferred, the panel's native mode
    pub modes: [Timing; MAX_MODES],
    pub n_modes: usize,
    /// Vertical refresh limits from descriptor type 0xFD, in Hz. Zero when the
    /// monitor did not say
    pub v_min_hz: u8,
    pub v_max_hz: u8,
    /// The block said it is a digital input (DP, HDMI, DVI) rather than analog
    pub digital: bool,
    /// Bits per component the panel accepts, from byte 0x14 bits 6:4.
    ///
    /// Zero means the monitor did not say, which EDID allows and is not the
    /// same as eight
    pub bpc: u8,
    /// How many extension blocks the base block claimed, whether or not they
    /// arrived. A claim of two with 128 bytes sent means a truncated EDID
    pub n_extensions: u8,
    /// How many of them were read and understood
    pub n_cea: u8,
    /// What the sink says about HDMI, out of the extensions
    pub hdmi: HdmiCaps,
    /// CEA-861 monitor support flags, byte 3 of the extension
    pub underscan: bool,
    pub basic_audio: bool,
    pub ycbcr444: bool,
    pub ycbcr422: bool,
    /// The CTA colorimetry block (extended tag 5): its two bytes, low first:
    /// xvYCC601, xvYCC709, sYCC601, opYCC601, opRGB, BT2020cYCC, BT2020YCC,
    /// BT2020RGB, then DCI-P3 in bit 15
    pub colorimetry: u16,
    pub hdr: HdrStatic,
    /// DisplayID extensions read
    pub n_displayid: u8,
}

impl Edid {
    /// Remember one timing, unless already remembered or out of room.
    ///
    /// A monitor's preferred timing normally appears both in the base block and
    /// as the first detailed timing of its CEA extension
    fn push_mode(&mut self, t: Timing) {
        if self.n_modes >= MAX_MODES {
            return;
        }
        if self.modes[..self.n_modes].contains(&t) {
            return;
        }
        self.modes[self.n_modes] = t;
        self.n_modes += 1;
    }

    /// A timing a table supplied for a mode the monitor only named. Skipped
    /// when a timing of the same size and scan is listed within half a hertz
    /// (CTA-861-E note 3: 60 Hz and 60/1.001 Hz are one mode)
    fn push_named(&mut self, t: Timing) {
        let same = self.modes[..self.n_modes].iter().any(|m| {
            m.h_display == t.h_display
                && m.v_display == t.v_display
                && m.interlace == t.interlace
                && m.refresh_centihz().abs_diff(t.refresh_centihz()) <= 50
        });
        if !same {
            self.push_mode(t);
        }
    }

    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }

    pub fn manufacturer_str(&self) -> &str {
        core::str::from_utf8(&self.manufacturer).unwrap_or("???")
    }

    /// The mode this monitor is best driven at: the most pixels, and between
    /// two of the same size the higher refresh
    pub fn best(&self) -> Option<Timing> {
        self.modes[..self.n_modes]
            .iter()
            .copied()
            // An interlaced mode is half a picture at a time, and nothing here
            // draws fields
            .filter(|m| !m.interlace)
            .max_by_key(|m| (m.pixels(), m.refresh_centihz()))
    }

    /// The mode the monitor itself puts first, which is its native one
    pub fn preferred(&self) -> Option<Timing> {
        (self.n_modes > 0).then(|| self.modes[0])
    }
}

/// Decode an EDID: the base block, and the extension blocks after it.
///
/// None when the bytes are not an EDID: a bad signature, or a base block whose
/// checksum does not hold. Extensions are best-effort past that
pub fn parse(b: &[u8]) -> Option<Edid> {
    parse_with(b, true)
}

/// The same, with the extensions optionally left unread.
/// `read_extensions = false` is for the boot line: the extensions hold the fast
/// timings
pub fn parse_with(b: &[u8], read_extensions: bool) -> Option<Edid> {
    if b.len() < 128 {
        return None;
    }
    if b[..8] != [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00] {
        return None;
    }
    // The whole block sums to zero in eight bits. Timings from an EDID that
    // does not are not programmed into a display engine
    if b[..128].iter().fold(0u8, |a, &x| a.wrapping_add(x)) != 0 {
        return None;
    }

    // Five bits a letter, big-endian, 1 = 'A'
    let id = u16::from_be_bytes([b[8], b[9]]);
    let letter = |shift: u16| -> u8 { b'A' + ((id >> shift) & 0x1f) as u8 - 1 };

    let mut e = Edid {
        manufacturer: [letter(10), letter(5), letter(0)],
        product: u16::from_le_bytes([b[10], b[11]]),
        serial: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
        name: [0; 13],
        name_len: 0,
        modes: [Timing::default(); MAX_MODES],
        n_modes: 0,
        v_min_hz: 0,
        v_max_hz: 0,
        digital: b[20] & 0x80 != 0,
        // 6:4, and the encoding counts up in twos from six with zero meaning
        // "not stated" and seven reserved
        bpc: if b[20] & 0x80 != 0 {
            match (b[20] >> 4) & 0x7 {
                1 => 6,
                2 => 8,
                3 => 10,
                4 => 12,
                5 => 14,
                6 => 16,
                _ => 0,
            }
        } else {
            0
        },
        n_extensions: b[126],
        n_cea: 0,
        hdmi: HdmiCaps::default(),
        underscan: false,
        basic_audio: false,
        ycbcr444: false,
        ycbcr422: false,
        colorimetry: 0,
        hdr: HdrStatic::default(),
        n_displayid: 0,
    };

    for slot in 0..4 {
        let d = &b[54 + slot * 18..54 + slot * 18 + 18];
        if let Some(t) = Timing::from_descriptor(d) {
            e.push_mode(t);
            continue;
        }
        // A display descriptor: the tag is at byte 3
        match d[3] {
            0xFC => {
                // The name, padded with 0x0A and spaces
                for &c in &d[5..18] {
                    if c == 0x0A {
                        break;
                    }
                    e.name[e.name_len] = c;
                    e.name_len += 1;
                }
                while e.name_len > 0 && e.name[e.name_len - 1] == b' ' {
                    e.name_len -= 1;
                }
            }
            0xFD => {
                e.v_min_hz = d[5];
                e.v_max_hz = d[6];
            }
            _ => {}
        }
    }

    // And whatever the extensions add. A block whose checksum fails is skipped:
    // the base block has been checked
    let mut svds = [0u8; MAX_SVDS];
    let mut n_svd = 0usize;
    for i in 0..if read_extensions { b[126] as usize } else { 0 } {
        let off = 128 * (i + 1);
        if b.len() < off + 128 {
            break;
        }
        let ext = &b[off..off + 128];
        if ext.iter().fold(0u8, |a, &x| a.wrapping_add(x)) != 0 {
            continue;
        }
        match ext[0] {
            EXT_TAG_CEA_861 => {
                parse_cea861(ext, &mut e, &mut svds, &mut n_svd);
                e.n_cea += 1;
            }
            EXT_TAG_DISPLAYID => {
                parse_displayid(&ext[1..127], &mut e);
                e.n_displayid += 1;
            }
            _ => {}
        }
    }

    // The named modes, after every detailed one
    for &svd in &svds[..n_svd] {
        if let Some(t) = vic_timing(svd) {
            e.push_named(t);
        }
    }
    let version = u16::from_be_bytes([b[18], b[19]]);
    for i in 0..8 {
        let code = u16::from_le_bytes([b[38 + 2 * i], b[39 + 2 * i]]);
        if let Some(t) = standard_timing(code, version) {
            e.push_named(t);
        }
    }
    let established = u16::from_be_bytes([b[35], b[36]]);
    for (j, slot) in dmt::ESTABLISHED.iter().enumerate() {
        if established & (0x8000 >> j) != 0 {
            if let Some(d) = slot {
                e.push_named(dmt_timing(d));
            }
        }
    }

    Some(e)
}

/// One short video descriptor as a timing (`parse861bShortTiming`,
/// `NvTiming_EnumCEA861bTiming`). Up to 192 the top bit is the "native" flag,
/// not part of the code (`NVT_GET_CTA_8BIT_VIC`). Interlaced formats,
/// pixel-repeated ones and the forbidden placeholders give nothing: this
/// display path sends none of them
fn vic_timing(svd: u8) -> Option<Timing> {
    let vic = if svd <= 192 { svd & 0x7f } else { svd };
    let v = cea_vic::VIC.get((vic as usize).checked_sub(1)?)?;
    if v.forbidden || v.interlaced || v.repetition & 1 == 0 {
        return None;
    }
    // `RRx1kToPclk`: total pixels a second, in 10 kHz units
    let pclk_10khz = (v.h_total as u64 * v.v_total as u64 * v.rrx1k as u64 / 10_000_000) as u32;
    Some(Timing {
        clock_khz: pclk_10khz * 10,
        h_display: v.h_visible as u32,
        h_sync_start: (v.h_visible + v.h_front_porch) as u32,
        h_sync_end: (v.h_visible + v.h_front_porch + v.h_sync_width) as u32,
        h_total: v.h_total as u32,
        v_display: v.v_visible as u32,
        v_sync_start: (v.v_visible + v.v_front_porch) as u32,
        v_sync_end: (v.v_visible + v.v_front_porch + v.v_sync_width) as u32,
        v_total: v.v_total as u32,
        nhsync: v.h_negative,
        nvsync: v.v_negative,
        interlace: false,
    })
}

fn dmt_timing(d: &dmt::Dmt) -> Timing {
    Timing {
        clock_khz: d.pclk_10khz * 10,
        h_display: d.h_visible as u32,
        h_sync_start: (d.h_visible + d.h_front_porch) as u32,
        h_sync_end: (d.h_visible + d.h_front_porch + d.h_sync_width) as u32,
        h_total: d.h_total as u32,
        v_display: d.v_visible as u32,
        v_sync_start: (d.v_visible + d.v_front_porch) as u32,
        v_sync_end: (d.v_visible + d.v_front_porch + d.v_sync_width) as u32,
        v_total: d.v_total as u32,
        nhsync: d.h_negative,
        nvsync: d.v_negative,
        interlace: false,
    }
}

/// One standard timing (`parseEdidStandardTimingDescriptor`): width in units
/// of 8 from 256, the aspect ratio, the refresh from 60. Only when DMT has
/// that exact mode (`NvTiming_CalcDMT`); OpenRM would compute the rest with
/// CVT or GTF, which this does not
fn standard_timing(code: u16, version: u16) -> Option<Timing> {
    // 0x0101 is "unused"; zero is an EDID that got it wrong
    if code & 0xff == 0x01 || code == 0 {
        return None;
    }
    let width = ((code & 0xff) as u32 + 31) << 3;
    let rr = ((code >> 8) & 0x3f) as u32 + 60;
    let height = match code >> 8 & 0xc0 {
        0x00 => if version < 0x103 { width } else { width * 5 / 8 },
        0x40 => width * 3 / 4,
        0x80 => width * 4 / 5,
        _ => width * 9 / 16,
    };
    dmt::DMT
        .iter()
        .find(|d| d.h_visible as u32 == width && d.v_visible as u32 == height && d.refresh as u32 == rr)
        .map(dmt_timing)
}

/// A DisplayID section inside an EDID extension (`getDisplayIdEDIDExtInfo`,
/// `parseDisplayIdSection`): version, byte count, product type, extension
/// count, then data blocks of {tag, revision, length}. The timing blocks are
/// read - 1.3's Type I and 2.0's Type VII, which share one twenty-byte layout
/// and differ in the unit of the pixel clock
fn parse_displayid(sec: &[u8], e: &mut Edid) {
    if sec.len() < 4 {
        return;
    }
    let end = (4 + sec[1] as usize).min(sec.len());
    let mut p = 4usize;
    while p + 3 <= end {
        let (tag, rev, len) = (sec[p], sec[p + 1], sec[p + 2] as usize);
        let body_end = p + 3 + len;
        if body_end > end {
            break;
        }
        let body = &sec[p + 3..body_end];
        match tag {
            // Type I: pixel clock in 10 kHz units
            DID_TYPE_I_TIMING if len % 20 == 0 => {
                for d in body.chunks_exact(20) {
                    if let Some(t) = displayid_detailed(d, 10) {
                        e.push_mode(t);
                    }
                }
            }
            // Type VII: 1 kHz units. A non-zero "payload bytes" field means
            // descriptors longer than twenty bytes, a layout OpenRM refuses too
            // (`parseDisplayId20Timing7`)
            DID_TYPE_VII_TIMING if (rev >> 4) & 0x7 == 0 && len % 20 == 0 => {
                for d in body.chunks_exact(20) {
                    if let Some(t) = displayid_detailed(d, 1) {
                        e.push_mode(t);
                    }
                }
            }
            _ => {}
        }
        p = body_end;
    }
}

/// One DisplayID detailed timing descriptor (`parseDisplayIdTiming1Descriptor`,
/// `parseDisplayId20Timing7Descriptor`): every field stored minus one, a
/// front-porch high byte that carries the sync polarity in its top bit, and a
/// timing whose total does not cover its porch and pulse refused (bug 2714607
/// in OpenRM's words)
fn displayid_detailed(d: &[u8], khz_per_unit: u32) -> Option<Timing> {
    let u24 = |i: usize| (d[i] as u32 | (d[i + 1] as u32) << 8 | (d[i + 2] as u32) << 16) + 1;
    let u16_1 = |i: usize| (d[i] as u32 | (d[i + 1] as u32) << 8) + 1;
    let porch = |i: usize| (d[i] as u32 | ((d[i + 1] & 0x7f) as u32) << 8) + 1;
    let clock_khz = u24(0) * khz_per_unit;
    let interlace = d[3] & 0x10 != 0;
    let (h_visible, h_blank, h_fp, h_sw) = (u16_1(4), u16_1(6), porch(8), u16_1(10));
    let (v_visible, v_blank, v_fp, v_sw) = (u16_1(12), u16_1(14), porch(16), u16_1(18));
    let h_total = h_visible + h_blank;
    let v_total = v_visible + v_blank;
    if h_total < h_visible + h_fp + h_sw || v_total < v_visible + v_fp + v_sw {
        return None;
    }
    Some(Timing {
        clock_khz,
        h_display: h_visible,
        h_sync_start: h_visible + h_fp,
        h_sync_end: h_visible + h_fp + h_sw,
        h_total,
        v_display: v_visible,
        v_sync_start: v_visible + v_fp,
        v_sync_end: v_visible + v_fp + v_sw,
        v_total,
        nhsync: d[9] & 0x80 == 0,
        nvsync: d[17] & 0x80 == 0,
        interlace,
    })
}

/// One CEA-861 extension block, all 128 bytes.
///
/// Layout, from the spec and `nvt_edidext_861.c`: byte 1 is the revision, byte
/// 2 is where the detailed timings start (0 means none), byte 3 carries the
/// monitor support flags, bytes 4 up to that offset are a collection of data
/// blocks, and everything from the offset to byte 126 is eighteen-byte timings
/// padded with zeros.
///
/// The video block (tag 2) lists modes as VIC numbers, collected into `svds`
/// and turned into timings after every detailed timing is in, from
/// `edid/cea_vic.rs`. The colorimetry and HDR static metadata blocks (extended
/// tags 5 and 6) are kept as sent. The audio block (tag 1) is not read: this
/// system has no audio
fn parse_cea861(ext: &[u8], e: &mut Edid, svds: &mut [u8; MAX_SVDS], n_svd: &mut usize) {
    let dtd_start = ext[2] as usize;

    // Byte 3 is reserved in revision 1 and only carries the flags from 2 on
    if ext[1] >= 2 {
        e.underscan = ext[3] & 0x80 != 0;
        e.basic_audio = ext[3] & 0x40 != 0;
        e.ycbcr444 = ext[3] & 0x20 != 0;
        e.ycbcr422 = ext[3] & 0x10 != 0;
    }

    // The data block collection, if any. `dtd_start` of 4 means the timings
    // begin immediately and there are no data blocks; 0 means no timings either
    if dtd_start > 4 {
        let mut p = 4usize;
        while p < dtd_start && p < 127 {
            let tag = ext[p] >> 5;
            let len = (ext[p] & 0x1f) as usize;
            // A zero-length block cannot be stepped over, and a block past
            // where the timings start disagrees with its own header. Either way
            // the collection stops being readable
            if len == 0 || p + 1 + len > dtd_start {
                break;
            }
            let body = &ext[p + 1..p + 1 + len];
            if tag == CEA_TAG_VENDOR && body.len() >= 3 {
                // The IEEE id is little-endian, three bytes
                let oui = u32::from_le_bytes([body[0], body[1], body[2], 0]);
                parse_vendor_block(oui, &body[3..], e);
            } else if tag == CEA_TAG_VIDEO {
                for &svd in body {
                    if *n_svd < MAX_SVDS {
                        svds[*n_svd] = svd;
                        *n_svd += 1;
                    }
                }
            } else if tag == CEA_TAG_EXTENDED && !body.is_empty() {
                match body[0] {
                    CEA_EXT_COLORIMETRY if body.len() >= 2 => {
                        e.colorimetry = body[1] as u16 | (body.get(2).copied().unwrap_or(0) as u16) << 8;
                    }
                    CEA_EXT_HDR_STATIC if body.len() >= 3 => {
                        e.hdr = HdrStatic {
                            present: true,
                            eotf: body[1],
                            metadata_types: body[2],
                            max_luminance: body.get(3).copied().unwrap_or(0),
                            max_frame_avg: body.get(4).copied().unwrap_or(0),
                            min_luminance: body.get(5).copied().unwrap_or(0),
                        };
                    }
                    _ => {}
                }
            }
            p += 1 + len;
        }
    }

    // The detailed timings, the same eighteen-byte descriptor as the base
    // block. The run ends at the zero padding, where `Timing::from_descriptor`
    // refuses a zero pixel clock
    if dtd_start >= 4 {
        let mut p = dtd_start;
        while p + 18 <= 127 {
            let Some(t) = Timing::from_descriptor(&ext[p..p + 18]) else {
                break;
            };
            e.push_mode(t);
            p += 18;
        }
    }
}

/// One vendor-specific data block, past its IEEE id.
///
/// Byte layouts are `nvtiming.h:4149` (HDMI 1.4) and `:4222` (HDMI Forum)
fn parse_vendor_block(oui: u32, payload: &[u8], e: &mut Edid) {
    match oui {
        IEEE_HDMI_LLC => {
            // The block existing is the claim. Everything after byte 1 is
            // optional and short blocks are legal, so each field is taken only
            // if it arrived
            e.hdmi.hdmi = true;
            // Bytes 0-1 are the source physical address, byte 2 the deep-colour
            // flags, byte 3 the clock ceiling in units of 5 MHz
            if payload.len() >= 4 && payload[3] != 0 {
                let khz = payload[3] as u32 * 5_000;
                e.hdmi.max_tmds_khz = e.hdmi.max_tmds_khz.max(khz);
            }
        }
        IEEE_HDMI_FORUM => {
            if payload.len() >= 2 && payload[1] != 0 {
                let khz = payload[1] as u32 * 5_000;
                e.hdmi.max_tmds_khz = e.hdmi.max_tmds_khz.max(khz);
            }
            if payload.len() >= 3 {
                e.hdmi.scramble_below_340 = payload[2] & (1 << 3) != 0;
                e.hdmi.scdc = payload[2] & (1 << 7) != 0;
            }
        }
        _ => {}
    }
}
