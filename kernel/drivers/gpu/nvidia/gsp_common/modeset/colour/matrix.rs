// Colour space conversion, as the hardware takes it
//
// Every CSC stage on this chip, the six in the window and OCSC0 and OCSC1 in
// the head, is a 3x4 matrix: three rows of three coefficients and an offset.
// The fixed point depends on the stage, so a matrix is kept here as a rational
// and converted at the point of use.
//
// This says what the coefficients are; `core.rs` and `window.rs` say where they
// go.

/// A 3x4 colour matrix: `out[i] = sum(c[i][j] * in[j]) + off[i]`.
///
/// Coefficients in the Q16 of the colour arithmetic: `1.0` is `0x1_0000`
#[derive(Clone, Copy)]
pub struct Matrix {
    pub c: [[i32; 3]; 3],
    pub off: [i32; 3],
}

/// One in Q16, an identity coefficient
pub const ONE: i32 = 1 << 16;

impl Matrix {
    /// Pass everything through
    pub const IDENTITY: Matrix = Matrix {
        c: [[ONE, 0, 0], [0, ONE, 0], [0, 0, ONE]],
        off: [0, 0, 0],
    };

    /// Is this the matrix the hardware would apply by itself?
    pub fn is_identity(&self) -> bool {
        self.c == Matrix::IDENTITY.c && self.off == [0, 0, 0]
    }

    /// Scale and offset each component alike.
    ///
    /// A range conversion: full-range content into a television's window is
    /// `scale = (940-64)/1023`, `offset = 64`. The head's CSC multiplies and
    /// adds
    pub fn scale_offset(scale_q16: i32, offset_q16: i32) -> Matrix {
        Matrix {
            c: [[scale_q16, 0, 0], [0, scale_q16, 0], [0, 0, scale_q16]],
            off: [offset_q16; 3],
        }
    }
    /// A gain per channel: colour balance.
    ///
    /// Gains in thousandths: 1000 leaves a channel alone, 900 takes a tenth
    /// out. A warm panel is cooled by lowering red
    pub fn gains(milli: [u16; 3]) -> Matrix {
        let q = |m: u16| ((m as i64 * ONE as i64) / 1000) as i32;
        Matrix {
            c: [[q(milli[0]), 0, 0], [0, q(milli[1]), 0], [0, 0, q(milli[2])]],
            off: [0, 0, 0],
        }
    }

    /// One coefficient in the format of the head's CSC stages.
    ///
    /// `nvCscCoefConvertS514`
    /// (`refsrc/openrm/src/nvidia-modeset/include/nvkms-evo3.h:83`): "more
    /// concisely, `(NvS32)floor(x * 65536.0 + 2.0)`", clamped to a signed
    /// 21-bit range and masked to bits 20:2. Our coefficients are Q16, so the
    /// conversion is the rounding, the clamp and the mask.
    ///
    /// The field is 20:0 but the bottom two bits are not part of the number:
    /// S5.14 with nineteen significant bits
    pub fn coefficient(q16: i32) -> u32 {
        let y = (q16 as i64 + 2).clamp(-0x10_0000, 0xf_ffff) as i32;
        (y as u32) & 0x001f_fffc
    }
}

/// What a monitor is told to expect on the wire.
///
/// The claim the transmitter makes; a claim disagreeing with the pixels gives a
/// washed-out or crushed picture. The matrix making the pixels match is chosen
/// from it
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Range {
    /// 0..1023 across the whole scale, for a computer monitor
    Full,
    /// 64..940, what broadcast video is carried in and a television expects
    Limited,
}

impl Range {
    /// The black and white points this range puts on a 10-bit scale
    pub fn points(self) -> (u16, u16) {
        match self {
            Range::Full => (0, 1023),
            Range::Limited => (64, 940),
        }
    }
}
