// What a head should be doing to colour
//
// One struct: a limited-range claim on the wire with full-range pixels behind
// it is the washed-out picture, two correct calls in the wrong order. NVKMS
// keeps this per-head and per-display as attributes, validated before anything
// is programmed.
//
// Everything defaults to what the hardware does by itself.

use super::{Curve, Range};

/// How more bits than the panel has become fewer.
///
/// A 6-bit panel fed 8 bits bands every gradient; dithering trades that for
/// noise the eye does not resolve
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dither {
    /// What the display does today
    Off,
    /// Spatial, the cheapest and what a static desktop wants
    Spatial,
    /// Temporal, which trades a still image's accuracy for a moving one's
    Temporal,
}

/// How many bits a component gets on the wire
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// 24 bpp, 8 bits a component - what every mode this driver has set uses
    Bpc8,
    /// 30 bpp, 10 bits a component. A panel that cannot take it says so in its
    /// EDID
    Bpc10,
}

impl Depth {
    /// Bits per component on the wire, what a link's bandwidth is sized by
    pub fn bits(self) -> u32 {
        match self {
            Depth::Bpc8 => 8,
            Depth::Bpc10 => 10,
        }
    }
}

/// Everything a head's colour is
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HeadColour {
    /// The ramp in the output LUT, after composition and before the wire
    pub curve: Curve,
    /// What the transmitter tells the monitor to expect. The curve's black and
    /// white points agree with it through `matching()`
    pub range: Range,
    pub dither: Dither,
    pub depth: Depth,
    /// A gain per channel in thousandths, red green blue. `[1000; 3]` leaves
    /// every channel alone, the head with OCSC0 switched off.
    ///
    /// The output CSC's first use: a diagonal matrix is the one shape that
    /// cannot be catastrophically wrong, and values near identity are small,
    /// visible, reversible changes
    pub balance: [u16; 3],
}

impl HeadColour {
    /// What the hardware does with no help: identity ramp, full range, no
    /// dither, eight bits a component
    pub const DEFAULT: HeadColour = HeadColour {
        curve: Curve::IDENTITY,
        range: Range::Full,
        dither: Dither::Off,
        depth: Depth::Bpc8,
        balance: [1000; 3],
    };

    /// What one input value comes out as, per channel, in the same 10-bit
    /// scale.
    ///
    /// The eye is a bad instrument when the console prints in cyan: cyan with
    /// its blue reduced looks green. A number says what happened.
    ///
    /// The hardware's order: the output LUT, then the output matrix. The clamp
    /// and the transmitter's claim come after and do not change the value
    pub fn sample(&self, input: u32) -> [u32; 3] {
        let after_lut = self.curve.at(input.min(1023)) as i64;
        let m = super::Matrix::gains(self.balance);
        [0, 1, 2].map(|i| {
            let v = (after_lut * m.c[i][i] as i64 + (1 << 15)) >> 16;
            v.clamp(0, 1023) as u32
        })
    }

    /// Is this what the head would do with nothing programmed? A state changing
    /// nothing leaves the hardware untouched
    pub fn is_default(&self) -> bool {
        self.curve.is_identity()
            && self.range == Range::Full
            && self.dither == Dither::Off
            && self.depth == Depth::Bpc8
            && self.balance == [1000; 3]
    }

    /// The output matrix this state wants, or `None` when the stage should stay
    /// switched off.
    ///
    /// `None`, not an identity matrix: a disabled stage and an enabled stage
    /// doing nothing differ to the hardware, and disabled is what works here
    pub fn output_matrix(&self) -> Option<super::Matrix> {
        if self.balance == [1000; 3] {
            None
        } else {
            Some(super::Matrix::gains(self.balance))
        }
    }

    /// Do the pixels and the claim agree?
    ///
    /// False is not an error (a person may want to see a mismatch), but it is
    /// the commonest way a colour pipeline is wrong with every register right
    pub fn matching(&self) -> bool {
        let (black, white) = self.range.points();
        self.curve.black == black && self.curve.white == white
    }
}
