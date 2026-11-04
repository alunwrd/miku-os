// Window rules: `windowrule = float, class:calculator`
//
// Matches on class and title and applies a property through the protocol:
// `set_window_state`, `set_window_param`, or `move_to_workspace`.

use crate::conn::Conn;
// The requests themselves, shared with the kernel's client half
use crate::config::{Config, Effect, Field, Rule};
use crate::requests::Requests;
use crate::state::State;

/// Applies matching rules once per window (`ruled`), so hand-made changes
/// survive later window lists
pub fn apply(c: &mut Conn, desk: u32, wm: u32, st: &mut State, cfg: &Config, idx: usize) -> bool {
    if idx >= st.n || st.wins[idx].ruled {
        return false;
    }
    // Waits for an app id or title, which a client sets after the toplevel
    // exists
    if st.wins[idx].app_id.len == 0 && st.wins[idx].title.len == 0 {
        return false;
    }
    st.wins[idx].ruled = true;

    let (client, surface) = (st.wins[idx].client, st.wins[idx].surface);
    let mut acted = false;
    // Size as rules leave it, in file order, so `size` then `center` uses the
    // new size
    let (mut w, mut h) = (st.wins[idx].w, st.wins[idx].h);
    let mut refloat_later = false;
    for i in 0..cfg.n_rules {
        let r = cfg.rules[i];
        if !catches(&r, st, idx) {
            continue;
        }
        match r.effect {
            Effect::State { mask, flags } => {
                c.wm_set_window_state(wm, client, surface, mask, flags);
                let on = flags & mask != 0;
                if mask & crate::protocol::wm::state::FLOATING != 0 {
                    st.set_floating(client, surface, on);
                    // Later `floating:` rules see it floating; earlier ones are
                    // redone after the pass
                    refloat_later = true;
                }
                if mask & crate::protocol::wm::state::FULLSCREEN != 0 {
                    st.set_fullscreen(client, surface, on);
                }
                if mask & crate::protocol::wm::state::PINNED != 0 {
                    st.set_pinned(client, surface, on);
                }
            }
            Effect::Param { .. } | Effect::Opacity { .. } => {
                let (sent, n) = params_of(&r.effect, cfg);
                for &(param, value) in &sent[..n] {
                    c.wm_set_window_param(wm, client, surface, param, value, true);
                }
            }
            Effect::Workspace(ws) => {
                if ws >= 1 && ws <= crate::protocol::wm::WORKSPACES {
                    c.desk_move_to_workspace(desk, client, surface, ws);
                    st.set_workspace_of(client, surface, ws);
                }
            }
            // Rectangle rules, sent regardless of state as in Hyprland; the
            // layout restores a tiled window
            Effect::Size(rw, rh) => {
                w = rw.max(1);
                h = rh.max(1);
                c.wm_resize(wm, client, surface, w, h);
            }
            Effect::Move(x, y) => c.wm_move(wm, client, surface, x, y),
            Effect::Center => {
                let (ax, ay, aw, ah) = st.work_area;
                // Skips centering with an unknown size
                if aw > 0 && ah > 0 && w > 0 && h > 0 {
                    c.wm_move(wm, client, surface, ax + (aw - w) / 2, ay + (ah - h) / 2);
                }
            }
        }
        acted = true;
    }
    if refloat_later {
        refloat(c, wm, st, cfg, idx);
    }
    acted
}

/// Whether rule `r` matches window `idx`
fn catches(r: &Rule, st: &State, idx: usize) -> bool {
    let w = &st.wins[idx];
    match r.field {
        Field::AppId => r.pat.matches(w.app_id.as_str()),
        Field::Title => r.pat.matches(w.title.as_str()),
        Field::Floating(f) => w.floating == f,
    }
}

/// Numbers an effect sends through `set_window_param`, at most two. `opacity`
/// is both, each the desktop's times the rule's unless `override` (Hyprland's
/// `SAlphaValue::applyAlpha`)
fn params_of(e: &Effect, cfg: &Config) -> ([(u32, f32); 2], usize) {
    use crate::protocol::wm::param;
    let desk = |p: u32| (0..cfg.n_params)
        .find(|&i| cfg.params[i].0 == p)
        .map_or(1.0, |i| cfg.params[i].1);
    match *e {
        Effect::Param { param, value } => ([(param, value), (0, 0.0)], 1),
        Effect::Opacity { active, inactive, own_active, own_inactive } => {
            let a = if own_active { active } else { active * desk(param::ALPHA) };
            let i = if own_inactive { inactive } else { inactive * desk(param::INACTIVE_ALPHA) };
            ([(param::ALPHA, a), (param::INACTIVE_ALPHA, i)], 2)
        }
        _ => ([(0, 0.0); 2], 0),
    }
}

/// Reruns `floating:` rules after a window's float state changes, as Hyprland
/// re-evaluates dynamic rules. Each param named is recomputed from matching
/// rules in file order, the last winning, and cleared if none match. Numbers
/// only
pub fn refloat(c: &mut Conn, wm: u32, st: &State, cfg: &Config, idx: usize) {
    if idx >= st.n {
        return;
    }
    let (client, surface) = (st.wins[idx].client, st.wins[idx].surface);
    // Params named by `floating:` rules
    let mut touched = [0u32; 8];
    let mut n_touched = 0;
    for i in 0..cfg.n_rules {
        let r = cfg.rules[i];
        if !matches!(r.field, Field::Floating(_)) {
            continue;
        }
        let (sent, n) = params_of(&r.effect, cfg);
        for &(param, _) in &sent[..n] {
            if !touched[..n_touched].contains(&param) && n_touched < touched.len() {
                touched[n_touched] = param;
                n_touched += 1;
            }
        }
    }
    for &param in &touched[..n_touched] {
        let mut value = None;
        for i in 0..cfg.n_rules {
            let r = cfg.rules[i];
            if !catches(&r, st, idx) {
                continue;
            }
            let (sent, n) = params_of(&r.effect, cfg);
            for &(p, v) in &sent[..n] {
                if p == param {
                    value = Some(v);
                }
            }
        }
        match value {
            Some(v) => c.wm_set_window_param(wm, client, surface, param, v, true),
            None => c.wm_set_window_param(wm, client, surface, param, 0.0, false),
        }
    }
}
