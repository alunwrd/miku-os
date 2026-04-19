use crate::sync::SpinLock;
use core::sync::atomic::{AtomicBool, Ordering};

struct RngState {
    state: u64,
}

impl RngState {
    const fn new() -> Self {
        Self { state: 0 }
    }

    fn next(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }
}

static RNG: SpinLock<RngState> = SpinLock::new(RngState::new());
static SEEDED: AtomicBool = AtomicBool::new(false);

fn ensure_seeded() {
    if !SEEDED.load(Ordering::Relaxed) {
        let mut rng = RNG.lock();
        if rng.state == 0 {
            let tick = crate::time::miku_uptime();
            let pid = crate::proc::miku_getpid();
            rng.state = tick.wrapping_mul(6364136223846793005).wrapping_add(pid).wrapping_add(1);
        }
        SEEDED.store(true, Ordering::Relaxed);
    }
}

#[no_mangle]
pub extern "C" fn miku_srand(seed: u64) {
    let mut rng = RNG.lock();
    rng.state = if seed == 0 { 1 } else { seed };
    SEEDED.store(true, Ordering::Relaxed);
}

#[no_mangle]
pub extern "C" fn miku_rand() -> u64 {
    ensure_seeded();
    RNG.lock().next()
}

#[no_mangle]
pub extern "C" fn miku_rand_range(lo: u64, hi: u64) -> u64 {
    if hi <= lo { return lo; }
    lo + miku_rand() % (hi - lo)
}

#[no_mangle]
pub extern "C" fn miku_rand_u32() -> u32 {
    (miku_rand() & 0xFFFF_FFFF) as u32
}

// fill buffer with random bytes
#[no_mangle]
pub extern "C" fn miku_rand_bytes(buf: *mut u8, len: usize) {
    if buf.is_null() || len == 0 { return; }
    ensure_seeded();
    let mut rng = RNG.lock();
    let mut i = 0usize;
    while i + 8 <= len {
        let val = rng.next();
        unsafe { (buf.add(i) as *mut u64).write_unaligned(val); }
        i += 8;
    }
    if i < len {
        let val = rng.next();
        let bytes = val.to_ne_bytes();
        for j in 0..(len - i) {
            unsafe { *buf.add(i + j) = bytes[j]; }
        }
    }
}

// random boolean
#[no_mangle]
pub extern "C" fn miku_rand_bool() -> bool {
    miku_rand() & 1 != 0
}

// random f64-like integer scaled to range [0, scale)
// useful when you need uniform distribution in a range without modulo bias
#[no_mangle]
pub extern "C" fn miku_rand_uniform(bound: u64) -> u64 {
    if bound <= 1 { return 0; }
    // rejection sampling to avoid modulo bias
    let limit = u64::MAX - (u64::MAX % bound);
    loop {
        let r = miku_rand();
        if r < limit {
            return r % bound;
        }
    }
}

// random signed i64 in range [lo, hi)
#[no_mangle]
pub extern "C" fn miku_rand_i64(lo: i64, hi: i64) -> i64 {
    if hi <= lo { return lo; }
    let range = (hi - lo) as u64;
    lo + miku_rand_uniform(range) as i64
}

// random float-like: returns value in [0, 1000000) / 1000000
// since no_std has no floats, returns integer thousandths [0..999999]
#[no_mangle]
pub extern "C" fn miku_rand_frac_million() -> u64 {
    miku_rand_uniform(1_000_000)
}

// dice roll: returns 1..sides
#[no_mangle]
pub extern "C" fn miku_rand_dice(sides: u32) -> u32 {
    if sides == 0 { return 0; }
    (miku_rand_uniform(sides as u64) + 1) as u32
}

// random sample: pick k unique indices from [0, n) into out array
// returns actual count sampled (min(k, n))
#[no_mangle]
pub extern "C" fn miku_rand_sample(n: usize, k: usize, out: *mut usize) -> usize {
    if out.is_null() || n == 0 || k == 0 { return 0; }
    let count = if k < n { k } else { n };

    // reservoir sampling for k < n
    for i in 0..count {
        unsafe { *out.add(i) = i; }
    }
    for i in count..n {
        let j = miku_rand_range(0, (i + 1) as u64) as usize;
        if j < count {
            unsafe { *out.add(j) = i; }
        }
    }
    count
}

// weighted random selection: given weights[0..n), returns index
// weights are non-negative integers
#[no_mangle]
pub extern "C" fn miku_rand_weighted(weights: *const u64, n: usize) -> usize {
    if weights.is_null() || n == 0 { return 0; }
    let mut total: u64 = 0;
    for i in 0..n {
        total += unsafe { *weights.add(i) };
    }
    if total == 0 { return 0; }

    let mut r = miku_rand_uniform(total);
    for i in 0..n {
        let w = unsafe { *weights.add(i) };
        if r < w { return i; }
        r -= w;
    }
    n - 1
}

// random permutation: fill out[0..n) with shuffled 0..n-1
#[no_mangle]
pub extern "C" fn miku_rand_perm(n: usize, out: *mut usize) {
    if out.is_null() || n == 0 { return; }
    for i in 0..n {
        unsafe { *out.add(i) = i; }
    }
    for i in (1..n).rev() {
        let j = miku_rand_range(0, (i + 1) as u64) as usize;
        if i != j {
            unsafe {
                let tmp = *out.add(i);
                *out.add(i) = *out.add(j);
                *out.add(j) = tmp;
            }
        }
    }
}

// shuffle an array in-place (Fisher-Yates)
#[no_mangle]
pub extern "C" fn miku_rand_shuffle(data: *mut u8, count: usize, elem_size: usize) {
    if data.is_null() || count < 2 || elem_size == 0 { return; }
    for i in (1..count).rev() {
        let j = miku_rand_range(0, (i + 1) as u64) as usize;
        if i != j {
            // swap elements i and j
            unsafe {
                let a = data.add(i * elem_size);
                let b = data.add(j * elem_size);
                for k in 0..elem_size {
                    let tmp = *a.add(k);
                    *a.add(k) = *b.add(k);
                    *b.add(k) = tmp;
                }
            }
        }
    }
}
