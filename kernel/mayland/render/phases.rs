// Where a frame spent its time on the processor
//
// A frame is split into the steps the loop takes, each is timed, and a late
// frame says which step was (`dev/mayland-architecture.md`, "A slow frame says
// where it was slow").
//
// Two lines in `dmesg debug`, both from the frame loop:
//   `a frame took N us on the processor: ...`  past `SLOW_US`, every phase
//   `the screen went N us without a frame ...`  past 2.5 raster periods, and
//                                              which counters moved meanwhile

use spin::Mutex;

/// The steps of a frame on the card's path, in the order the loop takes them
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Phase {
    /// Last frame's copies and compute, waited for (`wait_for_engines`)
    Engines = 0,
    /// Buffers back to clients and their callbacks (`frame_done`): the server
    /// lock
    Handback,
    /// The console and wallpaper pumps
    Pumps,
    /// The animation curves (`layout::tick`)
    Curves,
    /// Clients' own GPU work, held for (`take_fences`, at most a budget each)
    Fences,
    /// The scene, flattened (`presentable`): the server lock again
    Scene,
    /// The windows' copies in video memory (`texture::sync`, `submit`)
    Copies,
    /// Regions the processor blends itself (`compose::compose`), pixel by
    /// pixel: whatever is see-through and not the compute pass's
    Blend,
    /// The frame handed to the engines (`blit_batch`, `compose_frame`)
    Submit,
    /// What is left of `present`: the damage and the frame's lists
    Compose,
    /// The flip, and the fence wait in front of it
    Handover,
}

pub const PHASES: usize = 11;
pub const NAMES: [&str; PHASES] = [
    "engines", "handback", "pumps", "curves", "fences", "scene", "copies", "blend", "submit",
    "compose", "handover",
];

/// A frame on the processor longer than this says where it went. Longer than a
/// 240 Hz period and well short of a 60 Hz one
pub const SLOW_US: u64 = 6_000;

static NOW: Mutex<[u64; PHASES]> = Mutex::new([0; PHASES]);
static WORST: Mutex<[u64; PHASES]> = Mutex::new([0; PHASES]);
/// Pixels `Blend` went through this frame: a slow blend with a megapixel and
/// one with a few thousand are different problems
static BLENDED: Mutex<u64> = Mutex::new(0);

/// A new frame: every phase back to zero
pub fn begin() {
    *NOW.lock() = [0; PHASES];
    *BLENDED.lock() = 0;
}

/// Pixels the processor blended, added to this frame's
pub fn blended(px: u64) {
    *BLENDED.lock() += px;
}

/// Time spent in a phase, added to this frame's
pub fn add(p: Phase, us: u64) {
    NOW.lock()[p as usize] += us;
}

/// What a phase has cost this frame so far
pub fn get(p: Phase) -> u64 {
    NOW.lock()[p as usize]
}

/// The worst each phase has been since the compositor started
pub fn worst() -> [u64; PHASES] {
    *WORST.lock()
}

/// The frame is over: keep the worst of each phase, and report where a slow one
/// went
pub fn end(total_us: u64) {
    let now = *NOW.lock();
    {
        let mut w = WORST.lock();
        for i in 0..PHASES {
            w[i] = w[i].max(now[i]);
        }
    }
    if total_us >= SLOW_US {
        crate::kdebug!(
            "[mayland] a frame took {} us on the processor: engines {}, handback {}, pumps {}, curves {}, \
             fences {}, scene {}, copies {}, blend {} ({} Kpx), submit {}, compose {}, handover {}",
            total_us, now[0], now[1], now[2], now[3], now[4], now[5], now[6], now[7],
            *BLENDED.lock() / 1000, now[8], now[9], now[10]);
    }
}

/// The counters a gap between frames is explained by
#[derive(Clone, Copy, Default)]
pub struct Counters {
    pub stalls: u64,
    pub late: u64,
    pub held: u64,
    pub refused: u64,
}

static SEEN: Mutex<Counters> = Mutex::new(Counters { stalls: 0, late: 0, held: 0, refused: 0 });

/// Between two frames: if the screen went more than two and a half raster
/// periods without one, report how long and what moved
pub fn between(period_us: u64, raster_us: u64, now: Counters) {
    let was = core::mem::replace(&mut *SEEN.lock(), now);
    if raster_us == 0 || period_us * 2 <= raster_us * 5 {
        return;
    }
    crate::kdebug!(
        "[mayland] the screen went {} us without a frame (~{} raster periods): stalls +{}, late +{}, \
         flips held +{}, flips refused +{}",
        period_us, period_us / raster_us,
        now.stalls.saturating_sub(was.stalls), now.late.saturating_sub(was.late),
        now.held.saturating_sub(was.held), now.refused.saturating_sub(was.refused));
}
