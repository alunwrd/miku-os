// Hyprland's `monitor =` line and which of an output's modes it means
//
//     monitor = NAME, MODE, POSITION, SCALE[, vrr, N][, bitdepth, 10]
//     monitor = NAME, disable
//
// Follows `handleMonitor`
// (refsrc/hyprland/Hyprland/src/config/legacy/ConfigManager.cpp),
// `CMonitorRuleParser` (src/config/shared/monitor/Parser.cpp), and
// `CMonitor::applyMonitorRule` (src/output/Monitor.cpp). A rule gives
// `preferred`, the fastest (`highrr`), the largest (`highres`), the widest
// (`maxwidth`), or the mode closest to `WxH@R`: width first, then height, then
// refresh. A `WxH` with no `@` means 60 Hz (`m_refreshRate = 60`).
//
// NAME is a connector (`DP-3`), `desc:` and the start of the description, or
// empty for every output no other rule names.
//
// Refused by name: `transform`, `mirror`, `cm`, `sdrbrightness`,
// `sdrsaturation`, `icc`, `addreserved`, a scale other than 1, a custom
// modeline. The window manager prints the reason with the line.
//
// No allocator or I/O. The window manager parses with it, and the kernel's
// selftest checks it against the panel's mode list.

use crate::conf;

/// The longest name or description prefix a rule carries
pub const NAME_MAX: usize = 32;

/// Which mode a rule asks for
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Want {
    Preferred,
    /// The highest refresh, then the largest
    HighRr,
    /// The largest, then the highest refresh
    HighRes,
    /// The widest, then the highest refresh
    MaxWidth,
    /// The closest to this
    Size { width: i32, height: i32, refresh_mhz: i32 },
}

/// What a rule's name matches
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum By {
    /// A connector, `DP-3`
    Name,
    /// The start of the monitor's description, `desc:VIE HTMS270F240`
    Description,
    /// Every output no other rule names: the empty name
    Any,
}

/// One `monitor =` line
#[derive(Clone, Copy)]
pub struct Rule {
    name: [u8; NAME_MAX],
    name_len: usize,
    pub by: By,
    pub disabled: bool,
    pub want: Want,
    /// `XxY`, or `None` for `auto`. Unused: the compositor draws on one output
    pub position: Option<(i32, i32)>,
    /// `vrr, N`: 0 off, 1 on
    pub vrr: Option<u8>,
    /// `bitdepth, N`: 8 or 10
    pub bitdepth: Option<u8>,
}

impl Rule {
    pub const EMPTY: Rule = Rule {
        name: [0; NAME_MAX], name_len: 0, by: By::Any, disabled: false,
        want: Want::Preferred, position: None, vrr: None, bitdepth: None,
    };

    /// The connector or description prefix, as written
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
    }

    /// Whether this rule is about the output called `name`, described as
    /// `description`
    pub fn matches(&self, name: &str, description: &str) -> bool {
        match self.by {
            By::Name => self.name().eq_ignore_ascii_case(name),
            By::Description => description.trim().starts_with(self.name()),
            By::Any => true,
        }
    }
}

/// Why a line was not taken, and the part that was the reason
pub type Refusal<'a> = (&'static str, &'a str);

/// Read the value of a `monitor =` line
pub fn parse(value: &str) -> Result<Rule, Refusal<'_>> {
    let mut f = [""; 12];
    let mut n = 0;
    for part in value.split(',') {
        if n == f.len() {
            return Err(("too many fields on one monitor line", value));
        }
        f[n] = part.trim();
        n += 1;
    }
    let mut r = Rule::EMPTY;
    let name = f[0];
    let (by, text) = match name.strip_prefix("desc:") {
        Some(d) => (By::Description, d.trim()),
        None if name.is_empty() => (By::Any, ""),
        None => (By::Name, name),
    };
    if text.len() > NAME_MAX {
        return Err(("that output name is too long to carry", text));
    }
    r.by = by;
    r.name[..text.len()].copy_from_slice(text.as_bytes());
    r.name_len = text.len();

    match f[1] {
        "disable" | "disabled" => {
            r.disabled = true;
            return Ok(r);
        }
        "transform" => return Err(("transform - the picture is not rotated here", f[1])),
        "addreserved" => return Err(("addreserved - reserve the space with a layer surface", f[1])),
        _ => {}
    }
    r.want = parse_mode(f[1])?;
    r.position = parse_position(f[2])?;
    parse_scale(f[3])?;

    let mut i = 4;
    while i < n {
        let (key, val) = (f[i], if i + 1 < n { f[i + 1] } else { "" });
        match key {
            "" => {}
            "vrr" => match conf::number(val) {
                Some(v) if v == 0.0 || v == 1.0 => r.vrr = Some(v as u8),
                Some(v) if v == 2.0 || v == 3.0 =>
                    return Err(("vrr 2 and 3 are fullscreen-only, which means nothing here yet - use 0 or 1", val)),
                _ => return Err(("vrr takes 0 or 1", val)),
            },
            "bitdepth" => match val {
                "8" => r.bitdepth = Some(8),
                "10" => r.bitdepth = Some(10),
                _ => return Err(("bitdepth takes 8 or 10", val)),
            },
            "transform" => return Err(("transform - the picture is not rotated here", key)),
            "mirror" => return Err(("mirror - there is one output to draw on", key)),
            "cm" | "sdrbrightness" | "sdrsaturation" | "icc" =>
                return Err(("colour management is not here - 'display color' sets the head's colour", key)),
            _ => return Err(("not something a monitor line takes", key)),
        }
        i += 2;
    }
    Ok(r)
}

fn parse_mode(s: &str) -> Result<Want, Refusal<'_>> {
    if s.is_empty() || s.starts_with("pref") {
        return Ok(Want::Preferred);
    }
    if s.starts_with("highrr") {
        return Ok(Want::HighRr);
    }
    if s.starts_with("highres") {
        return Ok(Want::HighRes);
    }
    if s.starts_with("maxwidth") {
        return Ok(Want::MaxWidth);
    }
    if s.starts_with("modeline") {
        return Err(("a custom modeline - only the monitor's own modes are driven", s));
    }
    let (size, refresh) = match conf::split_once(s, '@') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    let (w, h) = conf::split_once(size, 'x').ok_or(("not a resolution", s))?;
    let num = |t: &str| conf::number(t).filter(|v| *v > 0.0 && *v == (*v as i32) as f32).map(|v| v as i32);
    let (Some(width), Some(height)) = (num(w), num(h)) else {
        return Err(("not a resolution", s));
    };
    let refresh_mhz = match refresh {
        // What Hyprland's rule holds when the line names no refresh
        None => 60_000,
        Some(r) => match conf::number(r.trim_end_matches("Hz")) {
            Some(v) if v > 0.0 => (v * 1000.0 + 0.5) as i32,
            _ => return Err(("not a refresh rate", r)),
        },
    };
    Ok(Want::Size { width, height, refresh_mhz })
}

fn parse_position(s: &str) -> Result<Option<(i32, i32)>, Refusal<'_>> {
    if s.is_empty() || s.starts_with("auto") {
        return Ok(None);
    }
    let (x, y) = conf::split_once(s, 'x').ok_or(("not a position", s))?;
    match (conf::number(x), conf::number(y)) {
        (Some(x), Some(y)) => Ok(Some((x as i32, y as i32))),
        _ => Err(("not a position", s)),
    }
}

fn parse_scale(s: &str) -> Result<(), Refusal<'_>> {
    if s.is_empty() || s.starts_with("auto") {
        return Ok(());
    }
    match conf::number(s) {
        Some(v) if v == 1.0 => Ok(()),
        Some(_) => Err(("a scale other than 1 - nothing here scales a picture", s)),
        None => Err(("not a scale", s)),
    }
}

/// One mode of an output, as the output manager sends it
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Mode {
    pub width: i32,
    pub height: i32,
    pub refresh_mhz: i32,
}

impl Mode {
    /// Refresh in whole hertz, rounded, as Hyprland compares
    fn hz(&self) -> i32 {
        (self.refresh_mhz + 500) / 1000
    }

    fn area(&self) -> i64 {
        self.width as i64 * self.height as i64
    }
}

/// Which of `modes` a rule means, in `applyMonitorRule`'s order. `preferred` is
/// the monitor's own, or the first mode when it named none. A tie goes to the
/// earlier mode, as after a stable sort
pub fn pick(want: Want, modes: &[Mode], preferred: Option<usize>) -> Option<usize> {
    if modes.is_empty() {
        return None;
    }
    let best_by = |key: &dyn Fn(&Mode) -> (i64, i64, i64)| {
        let mut best = 0;
        for i in 1..modes.len() {
            if key(&modes[i]) > key(&modes[best]) {
                best = i;
            }
        }
        Some(best)
    };
    match want {
        Want::Preferred => Some(preferred.filter(|&p| p < modes.len()).unwrap_or(0)),
        Want::HighRr => best_by(&|m| (m.hz() as i64, m.area(), 0)),
        Want::HighRes => best_by(&|m| (m.area(), m.hz() as i64, 0)),
        Want::MaxWidth => best_by(&|m| (m.width as i64, m.hz() as i64, 0)),
        // Closest: the smallest differences, so the largest negations
        Want::Size { width, height, refresh_mhz } => best_by(&|m| (
            -((m.width - width).abs() as i64),
            -((m.height - height).abs() as i64),
            -((m.refresh_mhz - refresh_mhz).abs() as i64),
        )),
    }
}

/// Whether `mode` is what was asked for, within a hertz, as Hyprland decides
/// between its best match and a custom mode
pub fn exact(want: Want, mode: &Mode) -> bool {
    match want {
        Want::Size { width, height, refresh_mhz } =>
            mode.width == width && mode.height == height && (mode.refresh_mhz - refresh_mhz).abs() < 1000,
        _ => true,
    }
}
