// HID report descriptor reader, far enough to find a pointer
//
// A boot keyboard and a boot mouse send a fixed report defined by the
// specification. Two kinds of device are outside that: anything that declines
// the boot subclass, and anything that is not a mouse. A tablet, a touch screen
// and QEMU's `usb-tablet` report where the pointer is, and no boot protocol
// covers that, so their bytes are defined only by the descriptor.
//
// Not a general HID parser. It walks the short items, keeps the globals and
// locals a pointer needs, and records where the buttons, X, Y and the wheel sit
// in the report. Collections, feature reports and unknown usages are stepped
// over.
//
// Reference: HID 1.11, section 6.2.2 (item format) and appendix B

/// Item tags, in (tag << 4 | type) form: the prefix byte with the size bits
/// masked off
const MAIN_INPUT: u8 = 0x80;
const MAIN_COLLECTION: u8 = 0xA0;
const MAIN_END_COLLECTION: u8 = 0xC0;
const GLOBAL_USAGE_PAGE: u8 = 0x04;
const GLOBAL_LOGICAL_MIN: u8 = 0x14;
const GLOBAL_LOGICAL_MAX: u8 = 0x24;
const GLOBAL_REPORT_SIZE: u8 = 0x74;
const GLOBAL_REPORT_ID: u8 = 0x84;
const GLOBAL_REPORT_COUNT: u8 = 0x94;
const LOCAL_USAGE: u8 = 0x08;
const LOCAL_USAGE_MIN: u8 = 0x18;
const LOCAL_USAGE_MAX: u8 = 0x28;

const PAGE_GENERIC_DESKTOP: u32 = 0x01;
const PAGE_BUTTON: u32 = 0x09;
const USAGE_X: u32 = 0x30;
const USAGE_Y: u32 = 0x31;
const USAGE_WHEEL: u32 = 0x38;

/// An INPUT item's flags: bit 0 constant (padding), bit 1 variable, bit 2
/// relative
const INPUT_CONSTANT: u32 = 1 << 0;
const INPUT_RELATIVE: u32 = 1 << 2;

/// Where one field sits in a report, in bits from the start of the data
#[derive(Clone, Copy, Default)]
pub struct Field {
    pub bit: u16,
    pub size: u8,
    pub count: u8,
    /// Present at all. A pointer with no wheel is ordinary
    pub found: bool,
}

/// What a pointing device's reports look like
#[derive(Clone, Copy)]
pub struct Pointer {
    /// The report id this describes, or 0 when the descriptor uses none and no
    /// id byte precedes the data
    pub report_id: u8,
    pub buttons: Field,
    pub x: Field,
    pub y: Field,
    pub wheel: Field,
    /// X and Y are a position, not a movement
    pub absolute: bool,
    /// The largest value of X and Y, for scaling a position onto a screen.
    /// Meaningless when the axes are relative
    pub logical_max: i32,
    /// Bytes one report occupies, id byte included
    pub report_len: usize,
}

impl Pointer {
    /// Read one field out of a report, as an unsigned value
    fn raw(&self, data: &[u8], f: &Field) -> u32 {
        if !f.found || f.size == 0 || f.size > 32 {
            return 0;
        }
        let mut v: u32 = 0;
        for i in 0..f.size as usize {
            let bit = f.bit as usize + i;
            let (byte, off) = (bit / 8, bit % 8);
            if byte >= data.len() {
                break;
            }
            if data[byte] >> off & 1 != 0 {
                v |= 1 << i;
            }
        }
        v
    }

    /// Sign-extended, for a relative axis and a wheel
    fn signed(&self, data: &[u8], f: &Field) -> i32 {
        let v = self.raw(data, f);
        if f.size == 0 || f.size >= 32 {
            return v as i32;
        }
        let sign = 1u32 << (f.size - 1);
        if v & sign != 0 {
            (v | !((1u32 << f.size) - 1)) as i32
        } else {
            v as i32
        }
    }

    /// Decode one report into buttons, the two axes and the wheel. The axes are
    /// a position when `absolute`, normalised to 0..=0xFFFF, otherwise a
    /// movement. None when the report is not the one this describes
    pub fn decode(&self, report: &[u8]) -> Option<(u8, i32, i32, i32)> {
        let data = if self.report_id != 0 {
            if report.first() != Some(&self.report_id) {
                return None;
            }
            &report[1..]
        } else {
            report
        };
        let buttons = self.raw(data, &self.buttons) as u8;
        let wheel = self.signed(data, &self.wheel);
        if self.absolute {
            let max = self.logical_max.max(1) as i64;
            let x = (self.raw(data, &self.x) as i64 * 0xFFFF / max) as i32;
            let y = (self.raw(data, &self.y) as i64 * 0xFFFF / max) as i32;
            Some((buttons, x, y, wheel))
        } else {
            Some((buttons, self.signed(data, &self.x), self.signed(data, &self.y), wheel))
        }
    }
}

/// Find the first pointer described by a report descriptor
///
/// A device with several reports (a tablet that is also a mouse, a keyboard
/// with a trackpad) describes them one after another, and the first pointer is
/// the one the device leads with
pub fn find_pointer(desc: &[u8]) -> Option<Pointer> {
    let mut page: u32 = 0;
    let mut usage_stack = [0u32; 16];
    let mut n_usage = 0usize;
    let mut report_size: u32 = 0;
    let mut report_count: u32 = 0;
    let mut report_id: u8 = 0;
    let mut logical_max: i32 = 0;
    let mut bit: u16 = 0;
    let mut depth: i32 = 0;

    let mut p = Pointer {
        report_id: 0,
        buttons: Field::default(),
        x: Field::default(),
        y: Field::default(),
        wheel: Field::default(),
        absolute: false,
        logical_max: 0,
        report_len: 0,
    };

    let mut i = 0usize;
    while i < desc.len() {
        let prefix = desc[i];
        // Long items are unused; the size field says how far to step over one
        if prefix == 0xFE {
            let len = *desc.get(i + 1).unwrap_or(&0) as usize;
            i += 3 + len;
            continue;
        }
        let size = match prefix & 0x03 {
            0 => 0usize,
            1 => 1,
            2 => 2,
            _ => 4,
        };
        if i + 1 + size > desc.len() {
            break;
        }
        let mut val: u32 = 0;
        for b in 0..size {
            val |= (desc[i + 1 + b] as u32) << (8 * b);
        }
        let tag = prefix & 0xFC;

        match tag {
            GLOBAL_USAGE_PAGE => page = val,
            GLOBAL_LOGICAL_MIN => {}
            GLOBAL_LOGICAL_MAX => logical_max = val as i32,
            GLOBAL_REPORT_SIZE => report_size = val,
            GLOBAL_REPORT_COUNT => report_count = val,
            GLOBAL_REPORT_ID => {
                // A new report starts from bit zero; the caller accounts for
                // the id byte in front of the data
                report_id = val as u8;
                bit = 0;
            }
            LOCAL_USAGE | LOCAL_USAGE_MIN | LOCAL_USAGE_MAX => {
                if n_usage < usage_stack.len() {
                    // A usage given with its page in the high half names the
                    // page itself; the low half is the usage within it
                    usage_stack[n_usage] = if size == 4 { val & 0xFFFF } else { val };
                    n_usage += 1;
                }
            }
            MAIN_COLLECTION => {
                depth += 1;
                n_usage = 0;
            }
            MAIN_END_COLLECTION => {
                depth -= 1;
                n_usage = 0;
                // Out of the pointer's collection, with everything found
                if depth <= 0 && p.x.found && p.y.found {
                    break;
                }
            }
            MAIN_INPUT => {
                let width = report_size * report_count;
                if val & INPUT_CONSTANT == 0 {
                    let relative = val & INPUT_RELATIVE != 0;
                    // Buttons come as a usage range on the button page; the
                    // axes as one usage each on the desktop page
                    if page == PAGE_BUTTON && !p.buttons.found {
                        p.buttons = Field {
                            bit,
                            size: (report_size * report_count).min(8) as u8,
                            count: report_count as u8,
                            found: true,
                        };
                        p.report_id = report_id;
                    } else if page == PAGE_GENERIC_DESKTOP {
                        // Several usages in one item (X and Y almost always
                        // arrive as a pair): each at its own offset within the
                        // item
                        for (u, slot) in usage_stack[..n_usage].iter().enumerate() {
                            let at = bit + (u as u16) * report_size as u16;
                            let f = Field {
                                bit: at,
                                size: report_size as u8,
                                count: 1,
                                found: true,
                            };
                            match *slot {
                                USAGE_X if !p.x.found => {
                                    p.x = f;
                                    p.absolute = !relative;
                                    p.logical_max = logical_max;
                                    p.report_id = report_id;
                                }
                                USAGE_Y if !p.y.found => p.y = f,
                                USAGE_WHEEL if !p.wheel.found => p.wheel = f,
                                _ => {}
                            }
                        }
                    }
                }
                bit = bit.saturating_add(width as u16);
                n_usage = 0;
            }
            _ => {
                if tag & 0x0C == 0x08 {
                    // Any other local item belongs to the next main item and is
                    // ignored
                }
            }
        }
        i += 1 + size;
    }

    if !(p.x.found && p.y.found) {
        return None;
    }
    p.report_len = (bit as usize).div_ceil(8) + if p.report_id != 0 { 1 } else { 0 };
    Some(p)
}
