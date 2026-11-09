// The heap under program-shaped work. The heap is the process's, so tests take
// turns (`ONE_AT_A_TIME`), and each checks all of it (`heap::check`), every
// slab the stand-in `mmap` gave

use std::sync::{Mutex, MutexGuard};

use crate::heap::{self, miku_calloc, miku_free, miku_malloc, miku_memalign, miku_realloc};
use crate::proc::mapped;

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn turn() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner())
}

/// How many free blocks and how many slabs, once every invariant holds
fn whole() -> (usize, usize) {
    heap::check(&mapped()).unwrap_or_else(|e| panic!("the heap is broken: {e}"))
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// What a live allocation was given, to detect overwrites
struct Live {
    ptr: *mut u8,
    len: usize,
    seed: u8,
    large: bool,
}

fn byte(seed: u8, i: usize) -> u8 {
    seed.wrapping_add((i as u8).wrapping_mul(31))
}

fn fill(p: *mut u8, len: usize, seed: u8) {
    for i in 0..len {
        unsafe { *p.add(i) = byte(seed, i) };
    }
}

fn holds(p: *const u8, len: usize, seed: u8) -> bool {
    (0..len).all(|i| unsafe { *p.add(i) } == byte(seed, i))
}

/// A program's size distribution: mostly small, some middling, a few big enough
/// for their own mapping
fn size(rng: &mut Rng) -> usize {
    match rng.below(100) {
        0..=79 => 1 + rng.below(600),
        80..=95 => 600 + rng.below(8000),
        _ => 32 * 1024 + rng.below(70 * 1024),
    }
}

#[test]
fn a_free_merges_with_both_neighbours() {
    let _t = turn();
    // A slab to cut from, if no test has made one yet
    miku_free(miku_malloc(1));
    let (before, _) = whole();
    // Three of a unique size, cut one after another from one free block
    let p: Vec<*mut u8> = (0..3).map(|_| miku_malloc(1000)).collect();
    miku_free(p[0]);
    miku_free(p[2]);
    let (apart, _) = whole();
    // The middle one joins both, and what they were cut from
    miku_free(p[1]);
    let (after, _) = whole();
    assert_eq!(after, before, "{before} free blocks before, {apart} with two freed, {after} after");
}

#[test]
fn random_work_keeps_the_heap_whole() {
    let _t = turn();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut live: Vec<Live> = Vec::new();
    let mappings_before = mapped().len();
    let (_, slabs_before) = whole();
    for op in 0..60_000 {
        match rng.below(100) {
            r if r < 45 || live.is_empty() => {
                let len = size(&mut rng);
                let ptr = miku_malloc(len);
                assert!(!ptr.is_null() && ptr as usize % 16 == 0, "malloc({len}) gave {ptr:?}");
                let seed = rng.next() as u8;
                fill(ptr, len, seed);
                live.push(Live { ptr, len, seed, large: len >= 32 * 1024 });
            }
            r if r < 80 => {
                let l = live.swap_remove(rng.below(live.len()));
                assert!(holds(l.ptr, l.len, l.seed), "op {op}: a block of {} was written over", l.len);
                miku_free(l.ptr);
            }
            r if r < 92 => {
                let i = rng.below(live.len());
                let len = size(&mut rng);
                let (old, old_len, seed) = (live[i].ptr, live[i].len, live[i].seed);
                let ptr = miku_realloc(old, len);
                assert!(!ptr.is_null(), "realloc to {len}");
                assert!(holds(ptr, old_len.min(len), seed), "op {op}: realloc lost what the block held");
                let seed = rng.next() as u8;
                fill(ptr, len, seed);
                live[i] = Live { ptr, len, seed, large: len >= 32 * 1024 };
            }
            r if r < 96 => {
                let len = 1 + rng.below(2000);
                let ptr = miku_calloc(1, len);
                assert!((0..len).all(|k| unsafe { *ptr.add(k) } == 0), "calloc({len}) is not zero");
                let seed = rng.next() as u8;
                fill(ptr, len, seed);
                live.push(Live { ptr, len, seed, large: false });
            }
            _ => {
                let align = [32, 64, 4096][rng.below(3)];
                let len = 1 + rng.below(3000);
                let ptr = miku_memalign(align, len);
                assert!(!ptr.is_null() && ptr as usize % align == 0, "memalign({align}, {len}) gave {ptr:?}");
                let seed = rng.next() as u8;
                fill(ptr, len, seed);
                live.push(Live { ptr, len, seed, large: false });
            }
        }
        if op % 500 == 0 {
            whole();
            for l in &live {
                assert!(holds(l.ptr, l.len, l.seed), "op {op}: a block of {} was written over", l.len);
            }
        }
    }
    for l in live.drain(..) {
        assert!(holds(l.ptr, l.len, l.seed));
        miku_free(l.ptr);
    }
    // With nothing of this test's left, each slab is one free block again,
    // showing every free merged both ways, and each large block's mapping is
    // gone
    let (free, slabs) = whole();
    assert_eq!(free, slabs, "{free} free blocks in {slabs} slabs");
    assert_eq!(mapped().len() - slabs, mappings_before - slabs_before, "a large block's mapping was kept");
}

#[test]
fn the_free_blocks_stay_few() {
    let _t = turn();
    // Keeps a few thousand things and keeps replacing them, as a shell's layout
    // does per frame
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut live: Vec<(*mut u8, usize)> = (0..5000).map(|_| {
        let n = 16 + rng.below(400);
        (miku_malloc(n), n)
    }).collect();
    let mut worst = 0usize;
    for round in 0..40 {
        let t = std::time::Instant::now();
        for _ in 0..5000 {
            let i = rng.below(live.len());
            miku_free(live[i].0);
            let n = 16 + rng.below(400);
            live[i] = (miku_malloc(n), n);
        }
        let ns = t.elapsed().as_nanos() / 10_000;
        let (free, slabs) = whole();
        worst = worst.max(free.saturating_sub(slabs));
        if round % 10 == 0 {
            eprintln!("round {round}: {free} free blocks for {} live, {ns} ns an allocation and a free", live.len());
        }
    }
    // No two free blocks touch, so there are at most as many as live blocks
    // between them, plus one per slab
    assert!(worst <= live.len(), "{worst} free blocks over the slabs for {} live", live.len());
    for (p, _) in live {
        miku_free(p);
    }
    whole();
}

// the thread caches (`heap.rs`, "The heap's thread caches")

/// Every request lands in a class at least its size, and a block of a class's
/// size returns to that class
#[test]
fn classes_round_trip() {
    use crate::heap::{class_down, class_size, class_up, CLASSES};
    for s in 1..=16384 {
        let (c, cs) = class_up(s).expect("cached size");
        assert!(cs >= s && class_size(c) == cs, "{s}: class {c} of {cs}");
        assert_eq!(class_down(cs), Some(c), "{s}: {cs} back to {c}");
    }
    assert_eq!(class_up(16385), None);
    for u in 16..20000 {
        let c = class_down(u).unwrap();
        assert!(class_size(c) <= u, "{u}: class {c} holds {}", class_size(c));
        if c + 1 < CLASSES {
            assert!(class_size(c + 1) > u, "{u}: class {} would fit too", c + 1);
        }
    }
}

/// A freed block is the next of its size with no lock; freed twice it is kept
/// once
#[test]
fn a_freed_block_comes_back() {
    let _t = turn();
    let p = miku_malloc(4000);
    miku_free(p);
    assert_eq!(heap::cached(), (1, 4096));
    miku_free(p);
    assert_eq!(heap::cached().0, 1, "freed twice, kept twice");
    let q = miku_malloc(4096);
    assert_eq!(p, q, "the cached block, not a new one");
    assert_eq!(heap::cached(), (0, 0));
    miku_free(q);
    whole();
}

/// A class keeps a limited number; the rest go to the heap
#[test]
fn a_cache_is_bounded() {
    let _t = turn();
    let ps: Vec<*mut u8> = (0..100).map(|_| miku_malloc(64)).collect();
    for &p in &ps {
        miku_free(p);
    }
    let (blocks, bytes) = heap::cached();
    assert!(blocks <= 32 && bytes <= 32 * 64, "{blocks} blocks, {bytes} bytes kept");
    whole();
}

/// Allocated on one thread and freed on another, whose end returns everything
#[test]
fn another_thread_frees_and_ends() {
    let _t = turn();
    let ps: Vec<usize> = (0..200).map(|i| miku_malloc(16 + i * 40) as usize).collect();
    std::thread::spawn(move || {
        for p in ps {
            miku_free(p as *mut u8);
        }
        assert!(heap::cached().0 > 0);
        crate::heap::miku_heap_thread_done();
        assert_eq!(heap::cached(), (0, 0));
    })
    .join()
    .unwrap();
    whole();
}
