// A checked request, applied in `nvSetDispModeEvo`'s order
//
// `nvkms-modeset.c:3990`: once the request is valid and committing, the
// satellite channels go idle, variable refresh comes down (`nvDisableVrr`),
// every display is applied with one update
// (`ApplyProposedModeSetHwStateOneDisp`), variable refresh comes back
// (`nvEnableVrr`). "From this point, we should not fail", so every refusal is
// in check.
//
// The layer decides the order. The driver lights, updates and switches variable
// refresh, and writes each head's record when its hardware took the change
// (`heads`).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::check::Change;
use super::Kms;

/// Run `changes` on `k`. A line for each thing that happened; on a failure,
/// the reason and what had already happened before it
pub fn run(k: &dyn Kms, changes: &[Change]) -> Result<Vec<String>, String> {
    if changes.is_empty() {
        return Ok(alloc::vec![String::from("nothing to change")]);
    }

    // 1. The frame path held where a raster changes under the compositor. NVKMS
    //    idles its satellite channels; here the whole compositor stops until
    //    the frame path is per head (K4). `stop_by_operator`, so the watchdog
    //    in `mayland::watch` does not restart it mid-update
    let hold = changes.iter().any(|c| c.hold_frames);
    if hold {
        crate::mayland::render::stop_by_operator();
    }

    let mut done = Vec::new();
    let r = steps(k, changes, &mut done);

    // 5. Back, whatever happened: every failure above leaves a working screen
    if hold {
        if let Err(e) = crate::mayland::render::start() {
            crate::kwarn!("[kms] the compositor did not start again: {}", e);
            done.push(format!("the compositor did not start again: {}", e));
        }
    }
    match r {
        Ok(()) => Ok(done),
        Err(e) if done.is_empty() => Err(e),
        Err(e) => Err(format!("{} (already done: {})", e, done.join("; "))),
    }
}

fn steps(k: &dyn Kms, changes: &[Change], done: &mut Vec<String>) -> Result<(), String> {
    // 2. Variable refresh down on every head whose raster changes, which goes
    //    to sleep, or which was asked to stop
    for c in changes.iter().filter(|c| c.vrr_off_before) {
        k.set_vrr(c, false)?;
        done.push(format!("{}: variable refresh off - fixed rate again", c.output));
    }
    // 3. The driver: new heads lit, then one update for everything else
    done.extend(k.apply(changes)?);
    // 4. Up again where wanted, never onto a sleeping monitor (check did not
    //    set it there)
    for c in changes.iter().filter(|c| c.vrr_on_after) {
        let t = k.set_vrr(c, true)?;
        done.push(format!(
            "{}: variable refresh on - a frame per flip, repeated after {} us if none comes",
            c.output, t));
    }
    Ok(())
}
