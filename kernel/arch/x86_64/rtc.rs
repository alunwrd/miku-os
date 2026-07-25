// CMOS/MC146818 real-time clock
//
// The kernel had no RTC driver at all: wall-clock time only ever arrived
// from NTP. On a machine with no network - or before DHCP finishes - the
// system genuinely did not know the date, and since the ext driver stamps
// inode times from the wall clock, every file created during that window
// got a timestamp of 0. Journal recovery and fsck both look at those times
//
// Reading the CMOS is a handful of port accesses, so there is no reason for
// the machine to be blind about the date until a DHCP lease shows up

use x86_64::instructions::port::Port;

const CMOS_ADDR: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

const REG_SECONDS: u8 = 0x00;
const REG_MINUTES: u8 = 0x02;
const REG_HOURS:   u8 = 0x04;
const REG_DAY:     u8 = 0x07;
const REG_MONTH:   u8 = 0x08;
const REG_YEAR:    u8 = 0x09;
const REG_STATUS_A: u8 = 0x0A;
const REG_STATUS_B: u8 = 0x0B;
/// ACPI FADT tells us where the century lives; 0x32 is the near-universal
/// location and the only one we try
const REG_CENTURY: u8 = 0x32;

const STATUS_A_UPDATE_IN_PROGRESS: u8 = 0x80;
const STATUS_B_24_HOUR: u8 = 0x02;
const STATUS_B_BINARY:  u8 = 0x04;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct DateTime {
    pub year:   u16,
    pub month:  u8,
    pub day:    u8,
    pub hour:   u8,
    pub minute: u8,
    pub second: u8,
}

/// The high bit of the CMOS address port disables NMI. Boot code clears it
/// deliberately, so preserve whatever state it is in rather than turning NMI
/// back on behind the caller's back
unsafe fn cmos_read(reg: u8) -> u8 {
    let mut addr: Port<u8> = Port::new(CMOS_ADDR);
    let mut data: Port<u8> = Port::new(CMOS_DATA);
    let nmi_bit = addr.read() & 0x80;
    addr.write(nmi_bit | (reg & 0x7F));
    data.read()
}

unsafe fn update_in_progress() -> bool {
    cmos_read(REG_STATUS_A) & STATUS_A_UPDATE_IN_PROGRESS != 0
}

#[inline]
fn from_bcd(v: u8) -> u8 {
    (v & 0x0F) + ((v >> 4) * 10)
}

unsafe fn read_raw() -> DateTime {
    DateTime {
        second: cmos_read(REG_SECONDS),
        minute: cmos_read(REG_MINUTES),
        hour:   cmos_read(REG_HOURS),
        day:    cmos_read(REG_DAY),
        month:  cmos_read(REG_MONTH),
        year:   cmos_read(REG_YEAR) as u16,
    }
}

/// Read the wall-clock date/time out of the CMOS
///
/// The chip updates its registers once a second and does not latch them, so
/// a naive read can catch a rollover mid-way (23:59:59 -> 00:00:00 gives
/// 23:00:00). The standard cure, used here: wait out any update in progress,
/// then read twice and only accept a pair of identical samples
pub fn read() -> Option<DateTime> {
    unsafe {
        // Bounded wait - a missing or wedged RTC must not hang the boot
        let mut spins = 0u32;
        while update_in_progress() {
            spins += 1;
            if spins > 1_000_000 {
                crate::serial_println!("[rtc] update flag stuck, ignoring CMOS clock");
                return None;
            }
            core::hint::spin_loop();
        }

        let mut prev = read_raw();
        let mut century = cmos_read(REG_CENTURY);
        for _ in 0..16 {
            while update_in_progress() {
                core::hint::spin_loop();
            }
            let cur = read_raw();
            let cur_century = cmos_read(REG_CENTURY);
            if cur == prev && cur_century == century {
                let status_b = cmos_read(REG_STATUS_B);
                return decode(cur, century, status_b);
            }
            prev = cur;
            century = cur_century;
        }
        crate::serial_println!("[rtc] readings never settled, ignoring CMOS clock");
        None
    }
}

fn decode(raw: DateTime, century_raw: u8, status_b: u8) -> Option<DateTime> {
    let bcd = status_b & STATUS_B_BINARY == 0;
    let hour_24 = status_b & STATUS_B_24_HOUR != 0;

    // In 12-hour mode bit 7 of the hour register is the PM flag and must be
    // stripped before any BCD conversion
    let hour_raw = raw.hour;
    let pm = !hour_24 && (hour_raw & 0x80) != 0;
    let hour_val = hour_raw & 0x7F;

    let conv = |v: u8| if bcd { from_bcd(v) } else { v };

    let mut hour = conv(hour_val);
    if !hour_24 {
        // 12 AM is 0, 12 PM is 12
        hour %= 12;
        if pm {
            hour += 12;
        }
    }

    let year_lo = conv(raw.year as u8) as u16;
    let century = conv(century_raw) as u16;
    // A plausible century means the register exists; otherwise assume 20xx
    let year = if (19..=21).contains(&century) {
        century * 100 + year_lo
    } else {
        2000 + year_lo
    };

    let dt = DateTime {
        year,
        month:  conv(raw.month),
        day:    conv(raw.day),
        hour,
        minute: conv(raw.minute),
        second: conv(raw.second),
    };

    if !(1..=12).contains(&dt.month)
        || !(1..=31).contains(&dt.day)
        || dt.hour > 23
        || dt.minute > 59
        || dt.second > 60
        || !(1970..=2200).contains(&dt.year)
    {
        crate::serial_println!(
            "[rtc] implausible CMOS reading {:04}-{:02}-{:02} {:02}:{:02}:{:02}, ignoring",
            dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
        );
        return None;
    }
    Some(dt)
}

/// Days from 1970-01-01 to the given civil date. Howard Hinnant's algorithm.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;                                   // [0, 399]
    let mp = ((m + 9) % 12) as i64;                            // Mar=0
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;               // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;           // [0, 146096]
    era * 146097 + doe - 719468
}

pub fn to_unix(dt: &DateTime) -> u64 {
    let days = days_from_civil(dt.year as i64, dt.month as u32, dt.day as u32);
    let secs = days * 86400
        + dt.hour as i64 * 3600
        + dt.minute as i64 * 60
        + dt.second as i64;
    if secs < 0 { 0 } else { secs as u64 }
}
