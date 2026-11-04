// Turning an EDID timing into the display engine's own `Mode`/`Raster`.


// The modeset modules are re-exported flat from `mod.rs`

// Mode, from the EDID

/// A display mode in the terms EDID and DRM both use: active area, sync pulse
/// position, total. The hardware wants another form; see 'raster'
#[derive(Debug, Clone, Copy, Default)]
pub struct Mode {
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

impl Mode {
    /// Build a mode from a timing the kernel's EDID parser produced
    /// (`drivers::gpu::edid`)
    pub fn from_timing(t: &crate::drivers::gpu::edid::Timing) -> Mode {
        Mode {
            clock_khz: t.clock_khz,
            h_display: t.h_display,
            h_sync_start: t.h_sync_start,
            h_sync_end: t.h_sync_end,
            h_total: t.h_total,
            v_display: t.v_display,
            v_sync_start: t.v_sync_start,
            v_sync_end: t.v_sync_end,
            v_total: t.v_total,
            nhsync: t.nhsync,
            nvsync: t.nvsync,
            interlace: t.interlace,
        }
    }

    /// The same timing in the display layer's terms (`drivers::gpu::kms`),
    /// which are EDID's: the inverse of `from_timing`
    pub fn to_timing(&self) -> crate::drivers::gpu::edid::Timing {
        crate::drivers::gpu::edid::Timing {
            clock_khz: self.clock_khz,
            h_display: self.h_display,
            h_sync_start: self.h_sync_start,
            h_sync_end: self.h_sync_end,
            h_total: self.h_total,
            v_display: self.v_display,
            v_sync_start: self.v_sync_start,
            v_sync_end: self.v_sync_end,
            v_total: self.v_total,
            nhsync: self.nhsync,
            nvsync: self.nvsync,
            interlace: self.interlace,
        }
    }

    /// Refresh rate in hundredths of a hertz, so 240 Hz reads as 24000 and a
    /// 59.94 Hz mode does not round to 60
    pub fn refresh_centihz(&self) -> u32 {
        let total = self.h_total as u64 * self.v_total as u64;
        if total == 0 {
            return 0;
        }
        ((self.clock_khz as u64 * 1000 * 100) / total) as u32
    }

    /// Convert to what the head methods take.
    ///
    /// DRM counts a mode from the start of the active area; the hardware counts
    /// from one unit into the sync pulse. Every field shifts, and "active"
    /// means the total, not the visible size
    pub fn raster(&self) -> Raster {
        // Blanking runs from where it starts to the end of the line, for any
        // sane mode sync_start..total
        let h_blank_end = self.h_total;
        let v_blank_end = self.v_total;
        Raster {
            h_active: self.h_total,
            h_synce: self.h_sync_end - self.h_sync_start - 1,
            h_blanke: h_blank_end - self.h_sync_start - 1,
            h_blanks: (h_blank_end - self.h_sync_start - 1) + self.h_display,
            v_active: self.v_total,
            v_synce: self.v_sync_end - self.v_sync_start - 1,
            v_blanke: v_blank_end - self.v_sync_start - 1,
            v_blanks: (v_blank_end - self.v_sync_start - 1) + self.v_display,
        }
    }
}

/// In hardware terms '*_active' is the 'total', not the visible size
#[derive(Debug, Clone, Copy, Default)]
pub struct Raster {
    pub h_active: u32,
    pub h_synce: u32,
    pub h_blanke: u32,
    pub h_blanks: u32,
    pub v_active: u32,
    pub v_synce: u32,
    pub v_blanke: u32,
    pub v_blanks: u32,
}

impl Mode {
    /// Every timing an EDID offered, in the display engine's own terms, best
    /// first.
    ///
    /// The ranking is `drivers::gpu::edid`'s: most pixels, then fastest, with
    /// interlaced modes dropped since nothing here draws fields. The whole
    /// list: a monitor can be asked for any mode it offered.
    ///
    /// Takes a parsed `Edid`: callers also want the monitor's name and what it
    /// says about HDMI
    pub fn list_from(e: &crate::drivers::gpu::edid::Edid) -> alloc::vec::Vec<Mode> {
        let mut t: alloc::vec::Vec<_> = e.modes[..e.n_modes]
            .iter()
            .copied()
            .filter(|m| !m.interlace)
            .collect();
        // Descending, so `modes[0]` is what `Edid::best` returns
        t.sort_by_key(|m| ::core::cmp::Reverse((m.pixels(), m.refresh_centihz())));
        t.iter().map(Mode::from_timing).collect()
    }
}
