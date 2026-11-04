// Outputs from the compositor's output manager, and `monitor =` lines applied
// to them
//
// The compositor sends each connector, then `done` with a serial. This keeps
// the last whole state and builds a configuration against that serial
// (`dev/kms-architecture.md`, K3) on:
// - the first `done`;
// - a reload;
// - a newly connected output.
//
// Not on every `done`, which would undo a manual `display mode`. Requests an
// output cannot do are reported and omitted: variable refresh or ten bits it
// lacks.

use crate::config::{Config, Text};
use crate::conn::{Conn, Event};
use crate::monitor::{self, Mode};
use crate::protocol::{output_configuration as oc, output_manager as om};
use crate::requests::Requests;

pub const MAX_HEADS: usize = 8;
pub const MAX_MODES: usize = 32;

/// One connector, as the last whole state had it
#[derive(Clone, Copy)]
struct Head {
    name: Text,
    description: Text,
    flags: u32,
    current: i32,
    preferred: i32,
    adaptive_sync: u32,
    max_bpc: u32,
    bpc: u32,
    modes: [Mode; MAX_MODES],
    n_modes: usize,
}

impl Head {
    const EMPTY: Head = Head {
        name: Text::EMPTY, description: Text::EMPTY, flags: 0, current: -1, preferred: -1,
        adaptive_sync: 0, max_bpc: 0, bpc: 0,
        modes: [Mode { width: 0, height: 0, refresh_mhz: 0 }; MAX_MODES], n_modes: 0,
    };

    fn connected(&self) -> bool {
        self.flags & om::CONNECTED != 0
    }
}

/// Text line with no allocator
struct Line {
    buf: [u8; 160],
    len: usize,
}

impl Line {
    const fn new() -> Line {
        Line { buf: [0; 160], len: 0 }
    }

    fn push(&mut self, s: &str) {
        let take = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
    }

    fn num(&mut self, v: i64) {
        if v < 0 {
            self.push("-");
        }
        let mut b = [0u8; 20];
        let n = miku::fmt_dec(&mut b, v.unsigned_abs());
        self.push(core::str::from_utf8(&b[..n]).unwrap_or("?"));
    }

    /// `1920x1080@200`: refresh in whole hertz, or two decimals
    fn mode(&mut self, m: &Mode) {
        self.num(m.width as i64);
        self.push("x");
        self.num(m.height as i64);
        self.push("@");
        self.num((m.refresh_mhz / 1000) as i64);
        let frac = (m.refresh_mhz % 1000) / 10;
        if frac != 0 {
            self.push(".");
            if frac < 10 {
                self.push("0");
            }
            self.num(frac as i64);
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

fn say(parts: &[&str]) {
    miku::print("  maywm: ");
    for p in parts {
        miku::print(p);
    }
    miku::println("");
}

pub struct Outputs {
    /// `may_output_manager` object; 0 when absent
    pub manager: u32,
    heads: [Head; MAX_HEADS],
    n: usize,
    /// A `done` closed the last burst, so the next head starts a new state
    complete: bool,
    serial: u32,
    /// Connected at the last `done`, to detect arrivals
    was_connected: [Text; MAX_HEADS],
    n_was: usize,
    /// The configuration waiting for its answer, and what it asked for
    pending: u32,
    asked: Line,
    /// Applies rules at the next whole state: the first, a mid-burst reload, or
    /// a cancelled configuration
    again: bool,
}

impl Outputs {
    pub const fn new(manager: u32) -> Outputs {
        Outputs {
            manager,
            heads: [Head::EMPTY; MAX_HEADS],
            n: 0,
            complete: false,
            serial: 0,
            was_connected: [Text::EMPTY; MAX_HEADS],
            n_was: 0,
            pending: 0,
            asked: Line::new(),
            again: true,
        }
    }

    /// One event from the compositor. True when a request went out
    pub fn event(&mut self, c: &mut Conn, ev: &Event, cfg: &Config) -> bool {
        match *ev {
            Event::OutHead { name, name_len, description, description_len, flags,
                             current, preferred, adaptive_sync, max_bpc, bpc } => {
                if self.complete {
                    self.n = 0;
                    self.complete = false;
                }
                if self.n < MAX_HEADS {
                    let text = |b: &[u8]| Text::from(core::str::from_utf8(b).unwrap_or(""));
                    self.heads[self.n] = Head {
                        name: text(&name[..name_len]),
                        description: text(&description[..description_len]),
                        flags, current, preferred, adaptive_sync, max_bpc, bpc,
                        ..Head::EMPTY
                    };
                    self.n += 1;
                }
                false
            }
            Event::OutMode { head, head_len, index, width, height, refresh } => {
                let name = core::str::from_utf8(&head[..head_len]).unwrap_or("");
                let i = index as usize;
                if let Some(h) = self.heads[..self.n].iter_mut().find(|h| h.name.as_str() == name) {
                    if i < MAX_MODES {
                        h.modes[i] = Mode { width, height, refresh_mhz: refresh };
                        h.n_modes = h.n_modes.max(i + 1);
                    }
                }
                false
            }
            Event::OutDone { serial } => {
                self.complete = true;
                self.serial = serial;
                // A newly connected output gets its line
                let mut arrived = false;
                for h in self.heads[..self.n].iter().filter(|h| h.connected()) {
                    if !self.was_connected[..self.n_was].iter().any(|t| t.as_str() == h.name.as_str()) {
                        arrived = true;
                    }
                }
                self.n_was = 0;
                for h in self.heads[..self.n].iter().filter(|h| h.connected()) {
                    self.was_connected[self.n_was] = h.name;
                    self.n_was += 1;
                }
                if self.again || arrived {
                    self.again = false;
                    return self.apply(c, cfg);
                }
                false
            }
            Event::OutConfigSucceeded { config } if config == self.pending => {
                say(&["outputs: ", self.asked.as_str()]);
                c.output_config_destroy(config);
                self.pending = 0;
                true
            }
            Event::OutConfigFailed { config, reason, reason_len } if config == self.pending => {
                say(&["outputs: ", self.asked.as_str(), " - refused: ",
                      core::str::from_utf8(&reason[..reason_len]).unwrap_or("?")]);
                c.output_config_destroy(config);
                self.pending = 0;
                true
            }
            Event::OutConfigCancelled { config } if config == self.pending => {
                // Outputs changed underneath; reconfigures at the next whole
                // state
                c.output_config_destroy(config);
                self.pending = 0;
                self.again = true;
                true
            }
            _ => false,
        }
    }

    /// Config reread: applies now, or at the next whole state if one is
    /// arriving
    pub fn reload(&mut self, c: &mut Conn, cfg: &Config) -> bool {
        if self.complete {
            self.apply(c, cfg)
        } else {
            self.again = true;
            false
        }
    }

    /// Applies the lines as one configuration with every difference. Nothing if
    /// the file names no outputs and no `misc:vrr`
    fn apply(&mut self, c: &mut Conn, cfg: &Config) -> bool {
        if self.manager == 0 || (cfg.n_monitors == 0 && cfg.misc_vrr.is_none()) {
            return false;
        }
        // One at a time; waits for the last answer
        if self.pending != 0 {
            self.again = true;
            return false;
        }
        self.name_lines_that_miss(cfg);

        let mut config = 0;
        self.asked = Line::new();
        for k in 0..self.n {
            let h = self.heads[k];
            if !h.connected() {
                continue;
            }
            let rule = cfg.monitor_rule(h.name.as_str(), h.description.as_str()).copied();
            let mut mode = oc::KEEP;
            let mut vrr = oc::KEEP;
            let mut bpc = 0u32;
            if let Some(r) = rule {
                if r.disabled {
                    say(&["monitor ", h.name.as_str(), ", disable - a head cannot be let go of yet; it stays lit"]);
                    continue;
                }
                let modes = &h.modes[..h.n_modes];
                if let Some(i) = monitor::pick(r.want, modes, (h.preferred >= 0).then_some(h.preferred as usize)) {
                    if !monitor::exact(r.want, &modes[i]) {
                        let mut l = Line::new();
                        l.push("monitor ");
                        l.push(h.name.as_str());
                        l.push(": no mode is exactly what the line asks - the closest is ");
                        l.mode(&modes[i]);
                        say(&[l.as_str()]);
                    }
                    if i as i32 != h.current {
                        mode = i as i32;
                    }
                }
                if let Some(b) = r.bitdepth {
                    if h.bpc == 0 {
                        say(&["monitor ", h.name.as_str(), ": bitdepth - this display has no colour depth to set"]);
                    } else if b == 10 && h.max_bpc < 10 {
                        say(&["monitor ", h.name.as_str(), ": bitdepth, 10 - this monitor does not say it takes ten bits"]);
                    } else if b as u32 != h.bpc {
                        bpc = b as u32;
                    }
                }
            }
            // Its own line's value, else `misc:vrr`. An incapable monitor is
            // reported only if its own line asked
            let asked_here = rule.and_then(|r| r.vrr);
            if let Some(v) = asked_here.or(cfg.misc_vrr) {
                let on = v == 1;
                if on && h.adaptive_sync == om::VRR_UNSUPPORTED {
                    if asked_here.is_some() {
                        say(&["monitor ", h.name.as_str(), ": vrr, 1 - this monitor cannot vary its refresh"]);
                    }
                } else if on != (h.adaptive_sync == om::VRR_ON) && h.adaptive_sync != om::VRR_UNSUPPORTED {
                    vrr = on as i32;
                }
            }
            if mode == oc::KEEP && vrr == oc::KEEP && bpc == 0 {
                continue;
            }
            if config == 0 {
                config = c.outputs_configure(self.manager, self.serial);
            }
            c.output_set(config, h.name.as_str(), mode, vrr, bpc);
            if self.asked.len > 0 {
                self.asked.push(", ");
            }
            self.asked.push(h.name.as_str());
            if mode >= 0 {
                self.asked.push(" ");
                self.asked.mode(&h.modes[mode as usize]);
            }
            if vrr >= 0 {
                self.asked.push(if vrr == 1 { " vrr on" } else { " vrr off" });
            }
            if bpc != 0 {
                self.asked.push(" ");
                self.asked.num(bpc as i64);
                self.asked.push(" bpc");
            }
        }
        if config == 0 {
            return false;
        }
        c.output_apply(config);
        self.pending = config;
        true
    }

    /// Reports lines naming an absent or unplugged output
    fn name_lines_that_miss(&self, cfg: &Config) {
        for r in cfg.monitors[..cfg.n_monitors].iter().filter(|r| r.by == monitor::By::Name) {
            let found = self.heads[..self.n].iter().find(|h| h.name.as_str().eq_ignore_ascii_case(r.name()));
            let mut l = Line::new();
            l.push("monitor ");
            l.push(r.name());
            match found {
                Some(h) if h.connected() => continue,
                Some(_) => l.push(": nothing is plugged into it"),
                None => l.push(": there is no output of that name"),
            }
            let mut first = true;
            for h in self.heads[..self.n].iter().filter(|h| h.connected()) {
                l.push(if first { " - connected: " } else { ", " });
                l.push(h.name.as_str());
                first = false;
            }
            say(&[l.as_str()]);
        }
    }
}
