// GTX 1650 on-die thermal sensor (PTHERM)
//
// Turing keeps the Pascal-era internal temperature sensor: a single
// fixed-point register that is live straight out of POST, with no VBIOS
// devinit or GSP firmware needed. Reading it is non-destructive, so this
// is one of the few things the driver can report about a real card today
//
// The slowdown / shutdown thresholds are also exposed, but they are only
// meaningful once the VBIOS thermal devinit table has run (which the
// driver does not do yet), so they read back as zero on a cold path

use crate::nvidia::mmio::MmioRegion;

use super::regs::{
    PTHERM_TEMP_SENSOR, PTHERM_TEMP_SHADOWED, PTHERM_TEMP_VALID,
    PTHERM_TEMP_VALUE_MASK, PTHERM_TEMP_VALUE_SHIFT,
    PTHERM_THRS_SHUTDOWN, PTHERM_THRS_SLOWDOWN,
};

#[derive(Copy, Clone, Debug)]
pub struct ThermReading {
    /// Raw NV_THERM_TEMP_SENSOR register value
    pub raw: u32,
    /// True if the sensor reported the value as valid
    pub valid: bool,
    /// True if the value came from the SHADOW latch (stale; the sensor was
    /// busy at sample time)
    pub shadowed: bool,
    /// Decoded temperature in integer degrees Celsius. Only meaningful when
    /// 'valid' is set
    pub celsius: i32,
    /// VBIOS-programmed slowdown threshold register (raw); zero if devinit
    /// has not run
    pub slowdown_raw: u32,
    /// VBIOS-programmed shutdown threshold register (raw); zero if devinit
    /// has not run
    pub shutdown_raw: u32,
}

impl ThermReading {
    /// Decode the slowdown threshold in degrees Celsius, if the register
    /// looks programmed. Same fixed-point layout as the live sensor
    pub fn slowdown_celsius(&self) -> Option<i32> {
        decode_threshold(self.slowdown_raw)
    }

    /// Decode the shutdown threshold in degrees Celsius, if programmed
    pub fn shutdown_celsius(&self) -> Option<i32> {
        decode_threshold(self.shutdown_raw)
    }
}

fn decode_threshold(raw: u32) -> Option<i32> {
    if raw == 0 || raw == 0xFFFF_FFFF {
        return None;
    }
    Some(((raw & PTHERM_TEMP_VALUE_MASK) >> PTHERM_TEMP_VALUE_SHIFT) as i32)
}

/// Read the internal temperature sensor and the (possibly unprogrammed)
/// slowdown / shutdown threshold registers. Pure reads, safe on a live
/// system at any point after BAR0 is mapped
pub fn read(bar0: &MmioRegion) -> ThermReading {
    let raw = bar0.read32(PTHERM_TEMP_SENSOR);
    let valid = raw & PTHERM_TEMP_VALID != 0;
    let shadowed = raw & PTHERM_TEMP_SHADOWED != 0;
    let celsius = ((raw & PTHERM_TEMP_VALUE_MASK) >> PTHERM_TEMP_VALUE_SHIFT) as i32;
    ThermReading {
        raw,
        valid,
        shadowed,
        celsius,
        slowdown_raw: bar0.read32(PTHERM_THRS_SLOWDOWN),
        shutdown_raw: bar0.read32(PTHERM_THRS_SHUTDOWN),
    }
}
