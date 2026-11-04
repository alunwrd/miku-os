use crate::class::{class_of, natural_align, CLASSES, MAX_SIZE, NCLASSES};
use crate::magazine::{CpuMags, Depot, MAG};
use crate::slab::{Cache, HDR, MIN_EMPTY, PAGE, POISON};
use crate::{caches, HostPages, Live, Rng};

/// Every size and alignment the heap can be asked for: the class it picks
/// holds the size, keeps the alignment, and is the smallest that does
#[test]
fn every_layout_gets_the_smallest_class_that_fits() {
    let mut align = 1;
    while align <= 8192 {
        for size in 1..=MAX_SIZE + 64 {
            let fits = |c: usize| CLASSES[c].0 >= size && natural_align(CLASSES[c].0) >= align;
            let want = (0..NCLASSES).find(|&c| fits(c));
            assert_eq!(class_of(size, align), want, "size {size} align {align}");
        }
        align *= 2;
    }
}

/// A slab holds its objects and its header, and wastes under an eighth
#[test]
fn every_class_fills_its_slabs() {
    for &(size, pages) in &CLASSES {
        let c = Cache::new("geometry", size, pages);
        let bytes = pages * PAGE;
        assert!(c.per_slab * size + HDR <= bytes, "{size}");
        let waste = bytes - c.per_slab * size;
        assert!(waste * 8 < bytes, "{size}-byte objects in {pages} pages waste {waste}");
        assert!(pages.is_power_of_two() && pages <= 8);
    }
}

/// Alloc and free at random across every class against a model: objects
/// are aligned, never overlap, keep what was written to them, and every
/// page comes back
#[test]
fn caches_hold_up_against_a_model() {
    let mut src = HostPages::default();
    let mut cs = caches();
    let mut live = Live::default();
    let mut of: std::collections::HashMap<usize, usize> = Default::default();
    let mut r = Rng(0x2545_f491_4f6c_dd1d);
    for i in 0..200_000u32 {
        let x = r.next();
        if live.len() < 3000 || x % 3 != 0 {
            let c = (x >> 8) as usize % NCLASSES;
            let size = CLASSES[c].0;
            let p = unsafe { cs[c].alloc(&mut src) };
            assert!(!p.is_null());
            assert_eq!(p as usize % natural_align(size), 0, "{size}-byte object at {p:p}");
            live.add(p, size, (x >> 32) as u8 | 1);
            of.insert(p as usize, c);
        } else {
            let p = live.any((x >> 16) as usize).unwrap();
            live.take(p);
            let c = of.remove(&(p as usize)).unwrap();
            unsafe { cs[c].free(p, &mut src) };
        }
        if i % 50_000 == 0 {
            for c in &cs {
                assert!(c.empty_slabs() <= MIN_EMPTY, "{} keeps {} empty slabs", c.size, c.empty_slabs());
            }
        }
    }
    while let Some(p) = live.any(0) {
        live.take(p);
        let c = of.remove(&(p as usize)).unwrap();
        unsafe { cs[c].free(p, &mut src) };
    }
    for c in cs.iter_mut() {
        assert_eq!(c.out, 0);
        unsafe { c.shrink(&mut src) };
        assert_eq!(c.slabs(), 0);
    }
    assert_eq!(src.outstanding(), 0, "every page back");
}

/// A burst, all freed: only `MIN_EMPTY` slabs stay, and `shrink` gives those
#[test]
fn empty_slabs_go_back() {
    let mut src = HostPages::default();
    let mut c = Cache::new("burst", 256, 1);
    let objs: Vec<_> = (0..c.per_slab * 40).map(|_| unsafe { c.alloc(&mut src) }).collect();
    assert_eq!(c.slabs(), 40);
    for p in objs {
        unsafe { c.free(p, &mut src) };
    }
    assert_eq!(c.slabs(), MIN_EMPTY);
    assert_eq!(src.outstanding(), MIN_EMPTY);
    assert_eq!(unsafe { c.shrink(&mut src) }, MIN_EMPTY);
    assert_eq!(src.outstanding(), 0);
}

/// Four CPUs, each with its magazines, sharing a cache and its depot; an
/// object freed on another CPU than the one it came from. Nothing is handed
/// out twice, and every object comes back
#[test]
fn magazines_hand_out_each_object_once() {
    let mut src = HostPages::default();
    let mut cache = Cache::new("mags", 64, 1);
    let mut depot = Depot::new();
    let mut cpus: Vec<CpuMags> = (0..4).map(|_| CpuMags::new()).collect();
    let mut live = Live::default();
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..300_000u32 {
        let x = r.next();
        let cpu = (x % 4) as usize;
        if live.len() < 2000 || x % 3 != 0 {
            let p = match cpus[cpu].pop() {
                Some(p) => p,
                None => {
                    assert!(unsafe { cpus[cpu].refill(&mut depot, &mut cache, &mut src) });
                    cpus[cpu].pop().unwrap()
                }
            };
            live.add(p, 64, (x >> 40) as u8 | 1);
        } else {
            let p = live.any((x >> 16) as usize).unwrap();
            live.take(p);
            if !cpus[cpu].push(p) {
                unsafe { cpus[cpu].spill(&mut depot, &mut cache, &mut src) };
                assert!(cpus[cpu].push(p));
            }
        }
        let held: usize = cpus.iter().map(|c| c.held()).sum::<usize>() + depot.held();
        assert_eq!(cache.out, live.len() + held, "every object is with a caller or in a magazine");
    }
    while let Some(p) = live.any(0) {
        live.take(p);
        if !cpus[0].push(p) {
            unsafe { cpus[0].spill(&mut depot, &mut cache, &mut src) };
            assert!(cpus[0].push(p));
        }
    }
    for c in cpus.iter_mut() {
        unsafe { c.drain(&mut cache, &mut src) };
    }
    unsafe { depot.drain(&mut cache, &mut src) };
    assert_eq!(cache.out, 0);
    unsafe { cache.shrink(&mut src) };
    assert_eq!(src.outstanding(), 0);
}

/// The depot gives back what sat unused through a whole interval, and keeps
/// what was taken from it meanwhile
#[test]
fn the_depot_reaps_its_unused_magazines() {
    let mut src = HostPages::default();
    let mut cache = Cache::new("reap", 128, 1);
    let mut depot = Depot::new();
    let mut cpu = CpuMags::new();
    // Six full magazines into the depot
    let objs: Vec<_> = (0..MAG * 8).map(|_| unsafe { cache.alloc(&mut src) }).collect();
    for p in objs {
        if !cpu.push(p) {
            unsafe { cpu.spill(&mut depot, &mut cache, &mut src) };
            assert!(cpu.push(p));
        }
    }
    assert_eq!(depot.held(), 6 * MAG);
    // The first reap starts the interval; nothing was below six since
    unsafe { depot.reap(&mut cache, &mut src) };
    // Two taken during the interval: four sat unused the whole time
    let mut other = CpuMags::new();
    for _ in 0..2 {
        while other.pop().is_some() {}
        assert!(unsafe { other.refill(&mut depot, &mut cache, &mut src) });
    }
    let back = unsafe { depot.reap(&mut cache, &mut src) };
    assert_eq!(back, 4 * MAG);
    assert_eq!(depot.held(), 0);
}

/// A stray write into a free object breaks its link, and the next
/// allocation that reaches it stops rather than handing out the address
#[test]
#[should_panic(expected = "free list is broken")]
fn a_broken_free_list_is_caught() {
    let mut src = HostPages::default();
    let mut c = Cache::new("hardened", 64, 1);
    let a = unsafe { c.alloc(&mut src) };
    let b = unsafe { c.alloc(&mut src) };
    unsafe { c.free(a, &mut src) };
    unsafe { c.free(b, &mut src) };
    // Use after free: the first word of the freed object
    unsafe { (b as *mut usize).write(0x4141_4141_4141_4141) };
    let _ = unsafe { c.alloc(&mut src) };
    let _ = unsafe { c.alloc(&mut src) };
}

/// A free through the wrong layout lands in the wrong cache, which says so
#[test]
#[should_panic(expected = "is not an object of")]
fn a_free_into_the_wrong_cache_is_caught() {
    let mut src = HostPages::default();
    let mut small = Cache::new("small", 64, 1);
    let mut big = Cache::new("big", 128, 1);
    let p = unsafe { small.alloc(&mut src) };
    unsafe { big.free(p, &mut src) };
}

/// `is_free` sees an object already on its free list: a second free
#[test]
fn a_double_free_is_seen() {
    let mut src = HostPages::default();
    let mut c = Cache::new("double", 96, 1);
    let a = unsafe { c.alloc(&mut src) };
    let b = unsafe { c.alloc(&mut src) };
    assert!(!unsafe { c.is_free(a) });
    unsafe { c.free(a, &mut src) };
    assert!(unsafe { c.is_free(a) });
    assert!(!unsafe { c.is_free(b) });
}

/// Under `slabdebug` a free object is poison, and a write into it after its
/// free is found at the next allocation of it
#[test]
fn a_write_after_free_is_found() {
    let mut src = HostPages::default();
    let mut c = Cache::new("poisoned", 128, 1);
    c.poison = true;
    let a = unsafe { c.alloc(&mut src) };
    assert_eq!(c.breach, 0, "a fresh object is poison too");
    unsafe { c.free(a, &mut src) };
    assert!(unsafe { std::slice::from_raw_parts(a.add(8), 120) }.iter().all(|&b| b == POISON));
    // Written after its free
    unsafe { a.add(64).write(7) };
    let again = unsafe { c.alloc(&mut src) };
    assert_eq!(again, a);
    assert_eq!(c.breach, a as usize);
}

/// The object freed last is the next one handed out, from whichever slab
#[test]
fn the_last_freed_comes_out_first() {
    let mut src = HostPages::default();
    let mut c = Cache::new("lifo", 64, 1);
    let objs: Vec<_> = (0..c.per_slab * 3).map(|_| unsafe { c.alloc(&mut src) }).collect();
    for &k in &[5usize, c.per_slab + 7, 2 * c.per_slab + 1, 9] {
        unsafe { c.free(objs[k], &mut src) };
        assert_eq!(unsafe { c.alloc(&mut src) }, objs[k]);
    }
}
