// Local mirror of what the compositor has reported, avoiding a round trip per
// key press. Fixed arrays, since there is no allocator

use crate::conf::Text;
use crate::protocol::wm;

pub const MAX_WINDOWS: usize = 32;

#[derive(Clone, Copy)]
pub struct Win {
    pub client: u32,
    pub surface: u32,
    pub workspace: u32,
    pub sticky: bool,
    pub backdrop: bool,
    pub floating: bool,
    pub fullscreen: bool,
    pub pinned: bool,
    /// Last reported position and size
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// App id, matched by rules
    pub app_id: Text,
    pub title: Text,
    /// Whether rules ran for this window
    pub ruled: bool,
}

impl Win {
    pub const EMPTY: Win = Win {
        client: 0, surface: 0, workspace: 0,
        x: 0, y: 0, w: 0, h: 0,
        sticky: false, backdrop: false,
        floating: false, fullscreen: false, pinned: false,
        app_id: Text::EMPTY, title: Text::EMPTY,
        ruled: false,
    };
}

pub struct State {
    pub wins: [Win; MAX_WINDOWS],
    pub n: usize,
    pub focus: (u32, u32),
    /// Previous keyboard holder, for `focuscurrentorlast`
    pub prev_focus: (u32, u32),
    pub workspace: u32,
    /// Previous workspace, for `workspace, previous`
    pub prev_workspace: u32,
    /// Bound key map. 0 is the default; otherwise an index into the config's
    /// names (`binds::apply_submap`)
    pub submap: u8,
    pub layout: u32,
    /// Last split ratio, so `splitratio` can be relative
    pub split: f32,
    /// Rectangle layouts may use
    pub work_area: (i32, i32, i32, i32),
}

impl State {
    pub const fn new() -> State {
        State {
            wins: [Win::EMPTY; MAX_WINDOWS],
            n: 0,
            focus: (0, 0),
            prev_focus: (0, 0),
            workspace: 1,
            prev_workspace: 0,
            submap: 0,
            layout: wm::LAYOUT_MASTER,
            split: 1.0,
            work_area: (0, 0, 0, 0),
        }
    }

    pub fn find(&self, client: u32, surface: u32) -> Option<usize> {
        (0..self.n).find(|&i| self.wins[i].client == client
                              && self.wins[i].surface == surface)
    }

    /// Adds or updates a window. `opened` carries no flags, so the following
    /// list fills them in
    pub fn add(&mut self, w: Win) {
        if let Some(i) = self.find(w.client, w.surface) {
            // All except `ruled`, which is local
            let ruled = self.wins[i].ruled;
            self.wins[i] = w;
            self.wins[i].ruled = ruled;
            return;
        }
        if self.n == MAX_WINDOWS {
            return;
        }
        self.wins[self.n] = w;
        self.n += 1;
    }

    pub fn remove(&mut self, client: u32, surface: u32) {
        let Some(i) = self.find(client, surface) else { return };
        // Closes the gap, keeping open order, which the layout uses
        for k in i..self.n - 1 {
            self.wins[k] = self.wins[k + 1];
        }
        self.n -= 1;
        // Clears focus, since chords act on `focus` before the next `focused`
        // event
        if self.focus == (client, surface) {
            self.focus = (0, 0);
        }
        // Clears it as a previous focus
        if self.prev_focus == (client, surface) {
            self.prev_focus = (0, 0);
        }
    }

    /// Windows the keyboard can go to, in layout order, excluding the backdrop
    pub fn visible(&self, out: &mut [usize]) -> usize {
        let mut n = 0;
        for i in 0..self.n {
            if self.wins[i].backdrop {
                continue;
            }
            if self.wins[i].sticky || self.wins[i].pinned
                || self.wins[i].workspace == self.workspace
            {
                if n < out.len() {
                    out[n] = i;
                    n += 1;
                }
            }
        }
        n
    }

    /// The window `step` places along from the focused one, wrapping
    pub fn neighbour(&self, step: i32) -> Option<(u32, u32)> {
        let mut vis = [0usize; MAX_WINDOWS];
        let v = self.visible(&mut vis);
        if v == 0 {
            return None;
        }
        let here = (0..v).find(|&k| {
            let w = self.wins[vis[k]];
            (w.client, w.surface) == self.focus
        });
        let next = match here {
            Some(k) => ((k as i32 + step).rem_euclid(v as i32)) as usize,
            None => 0,
        };
        let w = self.wins[vis[next]];
        Some((w.client, w.surface))
    }

    /// Focused window's rectangle. `None` when it has no size
    pub fn focus_box(&self) -> Option<(u32, u32, i32, i32, i32, i32)> {
        let (c, s) = self.focus;
        let i = self.find(c, s)?;
        let w = self.wins[i];
        (w.w > 0 && w.h > 0).then_some((c, s, w.x, w.y, w.w, w.h))
    }

    pub fn is_sticky(&self, c: u32, s: u32) -> bool {
        // Unknown windows count as sticky, refusing close and move
        self.find(c, s).map(|i| self.wins[i].sticky).unwrap_or(true)
    }

    pub fn is_floating(&self, c: u32, s: u32) -> bool {
        self.find(c, s).map(|i| self.wins[i].floating).unwrap_or(false)
    }

    pub fn is_fullscreen(&self, c: u32, s: u32) -> bool {
        self.find(c, s).map(|i| self.wins[i].fullscreen).unwrap_or(false)
    }

    pub fn is_pinned(&self, c: u32, s: u32) -> bool {
        self.find(c, s).map(|i| self.wins[i].pinned).unwrap_or(false)
    }

    pub fn set_floating(&mut self, c: u32, s: u32, on: bool) {
        if let Some(i) = self.find(c, s) { self.wins[i].floating = on; }
    }

    pub fn set_fullscreen(&mut self, c: u32, s: u32, on: bool) {
        if let Some(i) = self.find(c, s) { self.wins[i].fullscreen = on; }
    }

    pub fn set_pinned(&mut self, c: u32, s: u32, on: bool) {
        if let Some(i) = self.find(c, s) { self.wins[i].pinned = on; }
    }

    pub fn set_workspace_of(&mut self, c: u32, s: u32, ws: u32) {
        if let Some(i) = self.find(c, s) { self.wins[i].workspace = ws; }
    }

    /// Records the focus change and the previous holder
    pub fn set_focus(&mut self, to: (u32, u32)) {
        if self.focus == to {
            return;
        }
        // Only a real window is kept as previous
        if self.focus != (0, 0) {
            self.prev_focus = self.focus;
        }
        self.focus = to;
    }

    /// The workspace has changed under us
    pub fn set_workspace(&mut self, to: u32) {
        if self.workspace == to {
            return;
        }
        self.prev_workspace = self.workspace;
        self.workspace = to;
    }

    /// Whether a user window is on that workspace, excluding the backdrop and
    /// sticky windows
    pub fn workspace_used(&self, ws: u32) -> bool {
        (0..self.n).any(|i| {
            let w = self.wins[i];
            !w.backdrop && !w.sticky && w.workspace == ws
        })
    }

    /// Window a `class:` or `title:` match names, among focusable windows, with
    /// the rule matcher. Bare text is the class
    pub fn match_window(&self, spec: &str) -> Option<(u32, u32)> {
        let spec = spec.trim();
        let (title, pat) = match spec.split_once(':') {
            Some(("title", p)) => (true, p.trim()),
            Some(("class", p)) => (false, p.trim()),
            _ => (false, spec),
        };
        if pat.is_empty() {
            return None;
        }
        let needle = Text::from(pat);
        let mut vis = [0usize; MAX_WINDOWS];
        let v = self.visible(&mut vis);
        (0..v).map(|k| self.wins[vis[k]])
            .find(|w| needle.matches(
                if title { w.title.as_str() } else { w.app_id.as_str() }))
            .map(|w| (w.client, w.surface))
    }
}
