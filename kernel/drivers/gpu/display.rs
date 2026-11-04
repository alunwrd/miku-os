// Choosing the screen the system runs on
//
// Firmware hands over a framebuffer without asking the monitor. Under QEMU it
// is 800x600 at 24 bits, what GRUB picks when a multiboot2 kernel asks for no
// particular size; on a PC it is whatever the BIOS or GRUB last set.
//
// This uses Linux's order: find a display device that can be programmed, read
// the EDID off it, pick a mode, set it. On a machine whose only display is a
// real GPU nothing here can change the mode, as with `simpledrm`, and the
// firmware's framebuffer stays. The NVIDIA path is where a GB206 gets its own
// modeset.
//
// When
//
// Before the console exists, so the console is built at the chosen size. A mode
// set afterwards re-flows the text and re-targets the console
// (`mayland::console` does it).

use alloc::string::String;
use alloc::vec::Vec;

use super::kms::{Change, Connector, HeadState, Kms, Monitor, Output, State, Vrr};
use super::{bochs::Bochs, edid};

/// The screen, as everything above needs to see it
#[derive(Clone, Copy, Debug)]
pub struct Framebuffer {
    pub phys: u64,
    pub width: usize,
    pub height: usize,
    /// Bytes per scanline
    pub pitch: usize,
    pub bpp: u32,
}

/// What was decided, kept for the shell to print
static CHOSEN: spin::Mutex<Option<Chosen>> = spin::Mutex::new(None);

#[derive(Clone, Copy)]
pub struct Chosen {
    pub edid: Option<edid::Edid>,
    pub timing: Option<edid::Timing>,
    pub fb: Framebuffer,
    pub programmed: bool,
    /// A bochs display was found: the screen is a virtual machine's, not the
    /// firmware's framebuffer on whatever card is there
    pub bochs: bool,
    /// The frame the boot set up, in pixels. What was reserved for it (the
    /// console's copy of the panel, the wallpaper's memory) holds nothing
    /// larger, so no larger mode is taken on the fly
    pub boot: (usize, usize),
}

pub fn chosen() -> Option<Chosen> {
    *CHOSEN.lock()
}

/// Pick the best mode the monitor offers and put the display into it.
///
/// `current` is what firmware handed over, and comes back when nothing here can
/// do better, as on most real machines. Never fails: a display that cannot be
/// programmed keeps its mode
pub fn init(current: Framebuffer) -> Framebuffer {
    let Some(dev) = Bochs::find() else {
        crate::serial_println!(
            "[display] no programmable display - keeping the firmware's {}x{}",
            current.width,
            current.height
        );
        *CHOSEN.lock() = Some(Chosen {
            edid: None,
            timing: None,
            fb: current,
            programmed: false,
            bochs: false,
            boot: (current.width, current.height),
        });
        return current;
    };

    let parsed = dev.edid().as_ref().and_then(|b| edid::parse(b));
    match &parsed {
        Some(e) => crate::serial_println!(
            "[display] monitor {}{:04x} \"{}\"{}, {} timing(s) offered",
            e.manufacturer_str(),
            e.product,
            e.name_str(),
            if e.digital { ", digital" } else { "" },
            e.n_modes
        ),
        None => crate::serial_println!("[display] the display has no EDID to read"),
    }

    // The mode: the monitor's best, or what it is showing if it did not say
    let timing = parsed.as_ref().and_then(|e| e.best());
    let (w, h) = match timing {
        Some(t) => {
            let r = t.refresh_centihz();
            let native = parsed.as_ref().and_then(|e| e.preferred());
            crate::serial_println!(
                "[display] best of {} mode(s): {}x{} @ {}.{:02} Hz ({} kHz){}",
                parsed.map(|e| e.n_modes).unwrap_or(0),
                t.h_display,
                t.v_display,
                r / 100,
                r % 100,
                t.clock_khz,
                match native {
                    Some(n) if n != t => " - the monitor lists a slower one first",
                    _ => "",
                }
            );
            (t.h_display as usize, t.v_display as usize)
        }
        None => {
            crate::serial_println!(
                "[display] nothing to choose from - keeping {}x{}",
                current.width,
                current.height
            );
            *CHOSEN.lock() = Some(Chosen {
                edid: parsed,
                timing: None,
                fb: current,
                programmed: false,
                bochs: true,
                boot: (current.width, current.height),
            });
            return current;
        }
    };

    if (w, h, current.bpp) == (current.width, current.height, 32) {
        crate::serial_println!("[display] already in the best mode");
        *CHOSEN.lock() = Some(Chosen {
            edid: parsed,
            timing,
            fb: current,
            programmed: false,
            bochs: true,
            boot: (current.width, current.height),
        });
        return current;
    }

    // Thirty-two bits a pixel whatever the firmware chose: a client's buffer is
    // XRGB8888, so a 32-bit panel takes a row of one as a single copy and a
    // 24-bit panel takes it a pixel at a time
    let pitch = dev.set_mode(w as u32, h as u32, 32) as usize;
    let (got_w, got_h, got_bpp) = dev.current();
    if got_w as usize != w || got_h as usize != h {
        crate::serial_println!(
            "[display] asked for {}x{} and got {}x{}, going with what it did",
            w,
            h,
            got_w,
            got_h
        );
    }

    let fb = Framebuffer {
        phys: dev.fb_phys,
        width: got_w as usize,
        height: got_h as usize,
        pitch: if pitch == 0 {
            got_w as usize * 4
        } else {
            pitch
        },
        bpp: got_bpp as u32,
    };
    crate::serial_println!(
        "[display] {}x{} at {} bpp, {} bytes a line, framebuffer {:#x}",
        fb.width,
        fb.height,
        fb.bpp,
        fb.pitch,
        fb.phys
    );

    *CHOSEN.lock() = Some(Chosen {
        edid: parsed,
        timing,
        fb,
        programmed: true,
        bochs: true,
        boot: (fb.width, fb.height),
    });
    fb
}

// The display layer's view of this (`drivers::gpu::kms`)
//
// The firmware's framebuffer sets nothing: one mode, no driver, every change
// refused with the reason. bochs changes mode on the fly (K3 of
// `dev/kms-architecture.md`), so the whole chain (a WM's `monitor =` line, the
// protocol, the layer, a driver, the compositor following the new size) runs in
// QEMU. The layer holds the compositor's frames around it; this sets the mode,
// re-flows the console onto the panel at its new geometry and tells the layout
// the new size. Sleep, variable refresh and colour stay refused: the device has
// none of them.

/// QEMU's bochs display: one output, named as bochs-drm names it
pub struct BochsKms;
/// The framebuffer firmware left, with nothing here to drive it: named as
/// simpledrm names it
pub struct FirmwareKms;

pub static BOCHS: BochsKms = BochsKms;
pub static FIRMWARE: FirmwareKms = FirmwareKms;

/// Which of the two this machine has
pub fn kms() -> &'static dyn Kms {
    match chosen() {
        Some(c) if c.bochs => &BOCHS,
        _ => &FIRMWARE,
    }
}

fn one_output(name: &str, connector: Connector, vrr_why: &'static str) -> State {
    let Some(c) = chosen() else { return State { outputs: Vec::new() } };
    let monitor = c.edid.map(|e| Monitor {
        name: String::from(e.name_str()),
        make: String::from(e.manufacturer_str()),
        modes: e.modes[..e.n_modes].to_vec(),
        preferred: e.preferred().and_then(|p| e.modes[..e.n_modes].iter().position(|t| *t == p)),
        refresh_range: (e.v_max_hz > 0).then_some((e.v_min_hz as u32, e.v_max_hz as u32)),
        max_bpc: e.bpc,
    });
    let mode = match (&monitor, c.timing) {
        (Some(m), Some(t)) => m.modes.iter().position(|x| *x == t),
        _ => None,
    };
    State {
        outputs: alloc::vec![Output {
            name: String::from(name),
            connector,
            key: 0,
            connected: true,
            primary: true,
            monitor,
            head: Some(HeadState {
                head: 0,
                size: (c.fb.width as u32, c.fb.height as u32),
                refresh_centihz: c.timing.map_or(0, |t| t.refresh_centihz()),
                mode,
                asleep: false,
                colour: None,
                vrr: Vrr::Unsupported(vrr_why),
            }),
        }],
    }
}

/// Any change these drivers cannot make, refused with the reason for the first
/// thing it would change. `mode_why` is `None` where a new mode can be taken
fn refuse_changes(changes: &[Change], mode_why: Option<&'static str>) -> Result<(), String> {
    for c in changes {
        if c.light {
            return Err(String::from("there is one output and it is already lit"));
        }
        if c.raster {
            if let Some(why) = mode_why {
                return Err(String::from(why));
            }
        }
        if c.sleep.is_some() {
            return Err(String::from("this display cannot be put to sleep"));
        }
        if c.vrr_on_after || c.vrr_off_before {
            if let Some(HeadState { vrr: Vrr::Unsupported(why), .. }) = c.now {
                return Err(alloc::format!("no variable refresh: {}", why));
            }
            return Err(String::from("no variable refresh on this display"));
        }
        if c.recolour {
            return Err(String::from("this display has no colour pipeline to set"));
        }
    }
    Ok(())
}

impl Kms for BochsKms {
    fn name(&self) -> &'static str {
        "bochs (a virtual machine's display)"
    }
    fn state(&self) -> State {
        one_output("Virtual-1", Connector::Virtual, "not on a virtual display")
    }
    fn check(&self, changes: &[Change]) -> Result<(), String> {
        refuse_changes(changes, None)?;
        let c = chosen().ok_or_else(|| String::from("the display was never chosen"))?;
        for ch in changes.iter().filter(|ch| ch.raster) {
            let t = bochs_mode(&c, ch)?;
            if t.h_display as usize > c.boot.0 || t.v_display as usize > c.boot.1 {
                return Err(alloc::format!(
                    "{}x{} is larger than the {}x{} the boot set up, and the console's copy of \
                     the screen and the wallpaper were made for that one",
                    t.h_display, t.v_display, c.boot.0, c.boot.1));
            }
        }
        Ok(())
    }
    fn set_vrr(&self, _c: &Change, _on: bool) -> Result<u32, String> {
        Err(String::from("no variable refresh on a virtual display"))
    }
    fn apply(&self, changes: &[Change]) -> Result<Vec<String>, String> {
        let mut done = Vec::new();
        for ch in changes.iter().filter(|ch| ch.raster) {
            let c = chosen().ok_or_else(|| String::from("the display was never chosen"))?;
            let t = bochs_mode(&c, ch)?;
            let dev = Bochs::find().ok_or_else(|| String::from("the bochs display went away"))?;
            let pitch = dev.set_mode(t.h_display, t.v_display, 32) as usize;
            let (w, h, _) = dev.current();
            let (w, h) = (w as usize, h as usize);
            let pitch = if pitch == 0 { w * 4 } else { pitch };
            // The console onto the panel at its new geometry. It cannot fail
            // for a mode check let through, the frame being no larger than the
            // boot's; if it did, the device is put back
            if !crate::io::console::panel_resized(w, h, pitch) {
                let _ = dev.set_mode(c.fb.width as u32, c.fb.height as u32, 32);
                return Err(String::from("the console would not take the new geometry - the mode is back"));
            }
            *CHOSEN.lock() = Some(Chosen {
                timing: Some(t),
                fb: Framebuffer { width: w, height: h, pitch, ..c.fb },
                ..c
            });
            // The layout's screen, which re-arranges every window and layer
            // surface. The compositor is stopped around this by the layer and
            // reads the new size when it starts
            crate::mayland::server::set_output(w as i32, h as i32, t.refresh_centihz() as i32 * 10);
            let r = t.refresh_centihz();
            done.push(alloc::format!("{}: mode {}: {}x{} @ {}.{:02} Hz",
                ch.output, ch.mode.unwrap_or(0), w, h, r / 100, r % 100));
        }
        if done.is_empty() {
            done.push(String::from("nothing to change"));
        }
        Ok(done)
    }
}

/// The timing a change asks bochs for, out of the EDID the boot read
fn bochs_mode(c: &Chosen, ch: &Change) -> Result<edid::Timing, String> {
    let e = c.edid.as_ref().ok_or_else(|| String::from("the display has no EDID to pick a mode from"))?;
    ch.mode
        .and_then(|i| e.modes[..e.n_modes].get(i).copied())
        .ok_or_else(|| alloc::format!("{} has no such mode", ch.output))
}

impl Kms for FirmwareKms {
    fn name(&self) -> &'static str {
        "the firmware's framebuffer"
    }
    fn state(&self) -> State {
        one_output("Unknown-1", Connector::Unknown, "the firmware's framebuffer has no driver")
    }
    fn check(&self, changes: &[Change]) -> Result<(), String> {
        refuse_changes(changes,
            Some("the firmware's framebuffer has one mode, and nothing here can drive this display"))
    }
    fn set_vrr(&self, _c: &Change, _on: bool) -> Result<u32, String> {
        Err(String::from("the firmware's framebuffer has no driver"))
    }
    fn apply(&self, _changes: &[Change]) -> Result<Vec<String>, String> {
        Ok(alloc::vec![String::from("nothing to change")])
    }
}
