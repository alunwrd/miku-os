// The colour curve the head's output LUT is built from
//
// A curve is:
//
//   * gamma: the exponent applied to each component. 1.0 is identity
//   * black and white points: where 0 and 1023 land. 0 and 1023 is full range;
//     64 and 940 is the limited range broadcast video is carried in, and a
//     monitor expecting it shows full-range content with crushed blacks
//
// There is no libm here, so the exponent is fixed point: Q16 throughout, `log2`
// by repeated squaring and `exp2` by a table of `2^(1/2^k)`. Both are exact to
// about four decimal places over a LUT index's range, three more than a 16-bit
// entry can carry.
//
// All pure arithmetic, decided in QEMU by `nvidia selftest`.

/// `2^(1/2^(k+1))` in Q16, for k = 0..15.
///
/// `round(2**(1/2**(k+1)) * 65536)`. Multiplying the entries the fraction's
/// bits select gives `2^0.5 = 1.41421509` against a true `1.41421356`, the
/// accuracy of this file
const EXP2_TABLE: [u64; 16] = [
    0x1_6a0a, 0x1_3070, 0x1_172c, 0x1_0b56, 0x1_059b, 0x1_02ca, 0x1_0164, 0x1_00b2,
    0x1_0059, 0x1_002c, 0x1_0016, 0x1_000b, 0x1_0006, 0x1_0003, 0x1_0001, 0x1_0001,
];

const ONE: u64 = 1 << 16;

/// `log2(x)` for `x` in `(0, 1]`, Q16 in and Q16 out, as a magnitude.
///
/// Returns how far below zero the logarithm is: every input is a fraction.
/// `x = 0` has no logarithm; `curve()` maps index 0 straight to the black point
fn log2_below(mut x: u64) -> u64 {
    // Normalise into [1, 2) and count what that cost
    let mut whole = 0u64;
    while x < ONE {
        x <<= 1;
        whole += 1;
    }
    // Bit at a time: square, and a result reaching 2 contributes this bit and
    // is halved
    let mut frac = 0u64;
    for i in 0..16 {
        x = (x * x) >> 16;
        if x >= 2 * ONE {
            x >>= 1;
            frac |= 1 << (15 - i);
        }
    }
    (whole << 16) - frac
}

/// `2^-y` for `y >= 0`, Q16 in and Q16 out
fn exp2_neg(y: u64) -> u64 {
    let whole = y >> 16;
    if whole >= 32 {
        return 0;
    }
    // The table builds `2^frac`, the reciprocal of what is wanted: `2^-y` is
    // `2^-whole / 2^frac`
    let frac = y & 0xffff;
    let mut acc = ONE;
    for (k, m) in EXP2_TABLE.iter().enumerate() {
        if frac & (1 << (15 - k)) != 0 {
            acc = (acc * m) >> 16;
        }
    }
    ((ONE << 16) / acc) >> whole
}

/// What a person asks a colour pipeline for
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Curve {
    /// The exponent, in thousandths. 1000 is identity; a CRT-shaped panel wants
    /// about 2200 and anything under 1000 lifts the shadows
    pub gamma_milli: u32,
    /// Where index 0 and index 1023 land, in the same 10-bit scale. `(0, 1023)`
    /// is full range; `(64, 940)` is the limited range a television expects
    pub black: u16,
    pub white: u16,
}

impl Curve {
    /// Identity
    pub const IDENTITY: Curve = Curve { gamma_milli: 1000, black: 0, white: 1023 };

    /// The limited range broadcast video is carried in, with no gamma of its
    /// own
    pub const LIMITED: Curve = Curve { gamma_milli: 1000, black: 64, white: 940 };

    /// Is this the curve the hardware would do by itself?
    pub fn is_identity(&self) -> bool {
        self.gamma_milli == 1000 && self.black == 0 && self.white == 1023
    }

    /// One entry of the ramp, in the LUT's 10-bit scale:
    /// `black + (white - black) * (i/1023)^gamma`, rounded. Both ends are exact
    /// whatever the gamma: `0^g` is 0 and `1^g` is 1
    pub fn at(&self, i: u32) -> u32 {
        let (lo, hi) = (self.black as u32, self.white as u32);
        let span = hi as i64 - lo as i64;
        if i == 0 {
            return lo;
        }
        if i >= 1023 {
            return hi;
        }
        let unit = if self.gamma_milli == 1000 {
            // The common case, exact: no logarithm, no table
            ((i as u64) << 16) / 1023
        } else {
            // x^g = 2^(g * log2(x)), and log2 of a fraction is negative
            let x = ((i as u64) << 16) / 1023;
            let l = log2_below(x);
            exp2_neg((l * self.gamma_milli as u64) / 1000)
        };
        let scaled = (span * unit as i64 + (1 << 15)) >> 16;
        (lo as i64 + scaled).clamp(0, 1023) as u32
    }
}
