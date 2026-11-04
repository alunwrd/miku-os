// 'display': the screens, through the display layer (`drivers::gpu::kms`)
//
// One command whoever drives the screen: the NVIDIA card, QEMU's bochs display,
// or the firmware's framebuffer. Outputs are named as Linux names them
// (`DP-3`), and a change is one request the layer checks before anything moves:
// `test` at the end of any of them checks and stops there.
//
// `nvidia disp` keeps what only the NVIDIA driver can answer: display ids, OR
// and SOR, DPCD, the surface, `imp`, `underflow`, `poll`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::drivers::gpu::kms::{
    self, Curve, Depth, Dither, HeadColour, HeadRequest, Range, Request, State, Vrr,
};
use crate::{cprintln, print_error, print_success, println};

pub fn cmd_display(args: &str) {
    let mut words: Vec<&str> = args.split_whitespace().collect();
    // `test` anywhere at the end: check, change nothing
    let test_only = words.last() == Some(&"test");
    if test_only {
        words.pop();
    }
    match words.first().copied().unwrap_or("") {
        "" | "list" | "ls" => list(),
        "mode" => cmd_mode(&words[1..], test_only),
        "on" | "off" => cmd_power(words[0] == "off", &words[1..], test_only),
        "vrr" => cmd_vrr(&words[1..], test_only),
        "color" | "colour" | "gamma" => cmd_colour(&words[1..], test_only),
        "help" | "?" => usage(),
        other => {
            print_error!("  display: unknown '{}'", other);
            usage();
        }
    }
}

fn usage() {
    println!("Usage: display [list]");
    println!("       display mode <output> <n> [test]   - another of the monitor's own modes;");
    println!("                                            n is the number 'display' prints");
    println!("       display on|off <output> [test]     - wake or sleep a monitor (DPMS); 'on' on");
    println!("                                            an output nothing drives lights it");
    println!("       display vrr on|off [<output>] [test] - variable refresh, between the");
    println!("                                            monitor's EDID range");
    println!("       display color [<output>] k=v ... [test] - gamma=10..500 (hundredths),");
    println!("                                            range=full|limited, dither=off|spatial|");
    println!("                                            temporal, depth=8|10, red/green/blue=");
    println!("                                            100..2000 (thousandths), or 'reset'");
    println!("  Outputs are named as Linux names them (DP-3, HDMI-A-1, Virtual-1). 'test'");
    println!("  checks the change and stops: nothing on the screen moves");
}

/// Every output, what is on it, and what drives it
fn list() {
    let k = kms::current();
    let st = k.state();
    cprintln!(57, 197, 187, "  Display - {}", k.name());
    if st.outputs.is_empty() {
        println!("  no outputs - the display is not up");
    }
    for o in &st.outputs {
        let (cr, cg, cb) = if o.connected {
            (230, 240, 240)
        } else {
            (120, 140, 140)
        };
        let Some(m) = o.monitor.as_ref() else {
            cprintln!(
                cr,
                cg,
                cb,
                "    {:<9} {}",
                o.name,
                if o.connected {
                    "connected"
                } else {
                    "nothing plugged in"
                }
            );
            if let Some(h) = &o.head {
                cprintln!(
                    cr,
                    cg,
                    cb,
                    "        {}x{}, head {}",
                    h.size.0,
                    h.size.1,
                    h.head
                );
            }
            continue;
        };
        cprintln!(cr, cg, cb, "    {:<9} \"{}\"", o.name, m.name);
        match &o.head {
            Some(h) => {
                let r = h.refresh_centihz;
                cprintln!(
                    cr,
                    cg,
                    cb,
                    "        head {}, {}x{} @ {}.{:02} Hz{}",
                    h.head,
                    h.size.0,
                    h.size.1,
                    r / 100,
                    r % 100,
                    if h.asleep {
                        "  (asleep - nothing reaches the monitor)"
                    } else {
                        ""
                    }
                );
            }
            None => cprintln!(
                120,
                140,
                140,
                "        not driven - 'display on {}' lights it",
                o.name
            ),
        }
        let running = o.head.as_ref().and_then(|h| h.mode);
        for (n, t) in m.modes.iter().enumerate() {
            let r = t.refresh_centihz();
            cprintln!(
                cr,
                cg,
                cb,
                "        {} {:>2} {}x{}{} @ {}.{:02} Hz",
                if running == Some(n) { "*" } else { " " },
                n,
                t.h_display,
                t.v_display,
                if t.interlace { "i" } else { "" },
                r / 100,
                r % 100
            );
        }
        let range = match m.refresh_range {
            Some((lo, hi)) => alloc::format!("{}-{} Hz", lo, hi),
            None => String::from("no range"),
        };
        match o.head.as_ref().map(|h| h.vrr) {
            Some(Vrr::On { timeout_us }) => cprintln!(
                100,
                220,
                150,
                "        adaptive-sync {}: on, repeats after {} us",
                range,
                timeout_us
            ),
            Some(Vrr::Off) => cprintln!(
                cr,
                cg,
                cb,
                "        adaptive-sync {}: capable, off ('display vrr on {}')",
                range,
                o.name
            ),
            Some(Vrr::Unsupported(why)) => cprintln!(
                120,
                140,
                140,
                "        adaptive-sync {}: no, {}",
                range,
                why
            ),
            None => {}
        }
        if m.max_bpc != 0 {
            cprintln!(cr, cg, cb, "        takes {} bits a component", m.max_bpc);
        }
        if let Some(c) = o.head.as_ref().and_then(|h| h.colour) {
            cprintln!(cr, cg, cb, "        colour: {}", colour_line(&c));
        }
    }

    // The screen the boot picked, where the boot picked it: on a machine with
    // the card, the card drives it
    if crate::drivers::gpu::nvidia::device().is_none() {
        if let Some(c) = crate::drivers::gpu::display::chosen() {
            println!(
                "  screen: {}x{} at {} bpp, {} bytes a line, at {:#x}",
                c.fb.width, c.fb.height, c.fb.bpp, c.fb.pitch, c.fb.phys
            );
            if c.programmed {
                print_success!("  the display was programmed into that mode at boot");
            } else {
                println!("  the mode is the one firmware left; nothing here could change it");
            }
        }
    }
}

/// The output a command names, or the first one a head drives
fn output_named<'a>(st: &'a State, name: Option<&str>) -> Option<&'a kms::Output> {
    match name {
        Some(n) => {
            let o = st.output(n);
            if o.is_none() {
                print_error!(
                    "  display: there is no output called {} - 'display' lists them",
                    n
                );
            }
            o
        }
        None => {
            let o = st.first_lit();
            if o.is_none() {
                print_error!("  display: no output is being driven");
            }
            o
        }
    }
}

/// Check and apply one head's request, and say what happened. The layer's
/// lines name the output they are about, since a request may touch several
fn run(req: HeadRequest, test_only: bool) -> bool {
    let name = req.output.clone();
    match kms::apply(&Request::one(req, test_only)) {
        Ok(lines) => {
            for l in &lines {
                cprintln!(100, 220, 150, "  {}", l);
            }
            true
        }
        Err(e) => {
            print_error!("  {}: {}", name, e);
            false
        }
    }
}

fn cmd_mode(w: &[&str], test_only: bool) {
    let (Some(name), Some(Ok(n))) = (w.first(), w.get(1).map(|s| s.parse::<usize>())) else {
        print_error!("  usage: display mode <output> <n> [test] - 'display' numbers the modes");
        return;
    };
    let st = kms::state();
    let Some(o) = output_named(&st, Some(name)) else {
        return;
    };
    let mut r = HeadRequest::current(o);
    r.lit = true;
    r.mode = Some(n);
    run(r, test_only);
}

fn cmd_power(off: bool, w: &[&str], test_only: bool) {
    let Some(name) = w.first() else {
        print_error!("  usage: display on|off <output> [test]");
        return;
    };
    let st = kms::state();
    let Some(o) = output_named(&st, Some(name)) else {
        return;
    };
    let mut r = HeadRequest::current(o);
    if off {
        if !r.lit {
            print_error!("  {}: nothing drives it - it is already dark", o.name);
            return;
        }
        r.asleep = true;
    } else {
        r.lit = true;
        r.asleep = false;
    }
    run(r, test_only);
}

fn cmd_vrr(w: &[&str], test_only: bool) {
    let on = match w.first().copied() {
        Some("on") => true,
        Some("off") => false,
        _ => {
            print_error!("  usage: display vrr on|off [<output>] [test]");
            return;
        }
    };
    let st = kms::state();
    let Some(o) = output_named(&st, w.get(1).copied()) else {
        return;
    };
    let mut r = HeadRequest::current(o);
    r.vrr = on;
    run(r, test_only);
}

/// `display color [<output>] k=v ...`: the head's colour, as one state
///
/// One call, whole state. A command per knob is a pipeline that can be left
/// half-configured: a limited-range claim on the wire with full-range pixels
/// behind it is a washed-out picture. Each key defaults to what the head is
/// doing, so `display color dither=spatial` changes the dither and nothing
/// else. See `dev/colour-pipeline.md`.
///
/// Gamma is in hundredths because the shell has no floating point: 100 is
/// identity, 220 is what a CRT-shaped panel wants
fn cmd_colour(w: &[&str], test_only: bool) {
    // An output name first, or straight into the settings
    let (name, settings) = match w.first() {
        Some(first) if !first.contains('=') && *first != "reset" => (Some(*first), &w[1..]),
        _ => (None, w),
    };
    let st = kms::state();
    let Some(o) = output_named(&st, name) else {
        return;
    };
    let mut want = o
        .head
        .as_ref()
        .and_then(|h| h.colour)
        .unwrap_or(HeadColour::DEFAULT);

    let mut bad: Option<String> = None;
    for word in settings {
        // `reset` is a word, not a setting: everything else is key=value
        if *word == "reset" {
            want = HeadColour::DEFAULT;
            continue;
        }
        let Some((key, val)) = word.split_once('=') else {
            bad = Some(alloc::format!("'{}' is not key=value", word));
            break;
        };
        match (key, val) {
            ("gamma", v) => match v.parse::<u32>() {
                Ok(g) if (10..=500).contains(&g) => want.curve.gamma_milli = g * 10,
                _ => bad = Some(alloc::format!("gamma {} is not 10..500", v)),
            },
            // The range moves the pixels and the claim together, so it is one
            // key
            ("range", "full") | ("range", "limited") => {
                want.range = if val == "full" {
                    Range::Full
                } else {
                    Range::Limited
                };
                let (b, wpt) = want.range.points();
                want.curve.black = b;
                want.curve.white = wpt;
            }
            // Colour balance, by the output matrix. Thousandths so the shell
            // needs no floating point: 1000 leaves a channel alone, and a cast
            // is corrected by taking a little out of the channel with too much
            ("red", v) | ("green", v) | ("blue", v) => {
                let i = match key {
                    "red" => 0,
                    "green" => 1,
                    _ => 2,
                };
                match v.parse::<u16>() {
                    Ok(g) if (100..=2000).contains(&g) => want.balance[i] = g,
                    _ => bad = Some(alloc::format!("{} {} is not 100..2000", key, v)),
                }
            }
            ("dither", "off") => want.dither = Dither::Off,
            ("dither", "spatial") => want.dither = Dither::Spatial,
            ("dither", "temporal") => want.dither = Dither::Temporal,
            ("depth", "8") => want.depth = Depth::Bpc8,
            // Whether the monitor takes ten is the driver's check: it reads the
            // EDID, and "did not say" is refused rather than taken as eight
            ("depth", "10") => want.depth = Depth::Bpc10,
            _ => bad = Some(alloc::format!("'{}={}' is not a colour setting", key, val)),
        }
        if bad.is_some() {
            break;
        }
    }
    if let Some(why) = bad {
        print_error!("  display color: {}", why);
        println!("    gamma=10..500 (hundredths)  range=full|limited");
        println!("    dither=off|spatial|temporal  depth=8|10  red/green/blue=100..2000  reset");
        return;
    }

    let mut r = HeadRequest::current(o);
    r.colour = Some(want);
    if !run(r, test_only) || test_only {
        return;
    }
    let c: Curve = want.curve;
    cprintln!(
        140,
        220,
        160,
        "  {}{}",
        colour_line(&want),
        if want.is_default() {
            " (the default)"
        } else {
            ""
        }
    );
    // The three points a person can check by looking
    println!(
        "    ramp: 0 -> {}, 511 -> {}, 1023 -> {}",
        c.at(0),
        c.at(511),
        c.at(1023)
    );
    // What the state does to three values, per channel. A monitor plus an eye
    // is a poor instrument here: the console prints in cyan, and cyan with its
    // blue taken down looks green
    for (name, input) in [("white", 1023u32), ("grey", 511), ("black", 0)] {
        let o = want.sample(input);
        println!(
            "    {:<5} {:>4} -> r {:>4}  g {:>4}  b {:>4}",
            name, input, o[0], o[1], o[2]
        );
    }
    // Said rather than refused: a person may want to see a mismatch, the most
    // common way a colour pipeline is wrong while every register is right
    if !want.matching() {
        cprintln!(
            220,
            200,
            100,
            "    note: the pixels land on {}..{} and the monitor is told to expect {}",
            c.black,
            c.white,
            if want.range == Range::Limited {
                "64..940"
            } else {
                "0..1023"
            }
        );
    }
}

/// gamma, range, dither, depth and balance on one line. The balance always, not
/// only when it differs from the default: every key keeps what the head had, so
/// `red=700` then `blue=700` has both
fn colour_line(c: &HeadColour) -> String {
    alloc::format!(
        "gamma {}.{:02}, range {}, dither {}, {} bpc, balance r {} g {} b {}",
        c.curve.gamma_milli / 1000,
        (c.curve.gamma_milli % 1000) / 10,
        if c.range == Range::Limited {
            "limited"
        } else {
            "full"
        },
        match c.dither {
            Dither::Off => "off",
            Dither::Spatial => "spatial",
            Dither::Temporal => "temporal",
        },
        c.depth.bits(),
        c.balance[0],
        c.balance[1],
        c.balance[2],
    )
}
