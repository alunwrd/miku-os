// Logs slow frames once a second, split into layout, paint, and copy, with the
// painted area

use std::time::{Duration, Instant};

use crate::object::value::Oid;
use crate::object::Core;
use crate::scene::paint::Rect;

/// 1.5 frames at 240 Hz
const SLOW: Duration = Duration::from_millis(6);

/// How often the slowest one is said, at most
const EVERY: Duration = Duration::from_secs(1);

/// What one window's frame cost
pub struct Took {
    pub window: Oid,
    pub layout: Duration,
    pub paint: Duration,
    pub copy: Duration,
    /// Painted rectangles
    pub damage: Vec<Rect>,
    /// And by what
    pub by: By,
}

/// What drew a window's frame
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum By {
    /// Nothing had changed
    Nothing,
    Processor,
    /// Drawn on the card; `paint` is listing and recording
    Card,
    /// Card, with the processor for unrecordable rectangles
    Both,
}

impl Took {
    fn total(&self) -> Duration {
        self.layout + self.paint + self.copy
    }
}

#[derive(Default)]
pub struct Slowest {
    /// Worst frame since the first slow one, with its window name
    worst: Option<(Took, String)>,
    over: u32,
    since: Option<Instant>,
}

impl Slowest {
    pub fn note(&mut self, core: &Core, t: Took) {
        if t.total() < SLOW {
            return;
        }
        self.over += 1;
        self.since.get_or_insert_with(Instant::now);
        if self.worst.as_ref().is_none_or(|(w, _)| t.total() > w.total()) {
            let name = core.text(t.window, "WlrLayershell.namespace");
            let name = if name.is_empty() { format!("window {}", t.window) } else { name };
            self.worst = Some((t, name));
        }
    }

    /// Logs the slowest frame since the last line, at most once a second
    pub fn tell(&mut self) {
        if self.since.is_none_or(|s| s.elapsed() < EVERY) {
            return;
        }
        if let Some((t, name)) = self.worst.take() {
            let ms = |d: Duration| d.as_secs_f64() * 1e3;
            let px: i64 = t.damage.iter().map(|d| d.area()).sum();
            let painted = match t.damage.len() {
                0 => "nothing".to_string(),
                1 => format!("{}x{}", t.damage[0].w, t.damage[0].h),
                n => format!("{n} rectangles, {} Kpx", px / 1000),
            };
            let by = match t.by {
                By::Nothing => "",
                By::Processor => " on the processor",
                By::Card => " on the card",
                By::Both => " on the card and the processor",
            };
            crate::say(&format!(
                "{} frame(s) slower than {} ms in the last second; the slowest {:.1} ms, {} \
                 (layout {:.1} ms, paint {:.1} ms over {}{by}, copy {:.1} ms)",
                self.over, SLOW.as_millis(), ms(t.total()), name, ms(t.layout), ms(t.paint), painted, ms(t.copy)));
        }
        self.over = 0;
        self.since = None;
    }
}
