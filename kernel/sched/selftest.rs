// Selftest for the scheduler's structures (`dev/kernel-core.md`, "The run
// queue and the process table"): the heap every run queue and every CPU's
// sleepers are, the pid bitmap that is the list of live tasks, and the job
// index. Run from the `ipc` suite, beside the time and wait cases it shares a
// note with

extern crate alloc;

use alloc::format;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::kcore::selftest::Runner;

use super::slot_heap::SlotHeap;

pub fn checks(t: &mut Runner) {
    check_heap(t);
    check_pids(t);
    check_live_walk(t);
    check_jobs(t);
}

/// What the heap is filled with here: a slot and the key it went in with
struct Fake {
    slot: AtomicU32,
    key: u64,
    tie: u64,
}

/// Pushed in a scrambled order with many equal keys, a third taken out from
/// wherever they had got to, and the rest popped: in (key, tie) order, none
/// of the removed among them, and every slot naming its own entry until then
fn check_heap(t: &mut Runner) {
    const N: usize = 300;
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut fakes: Vec<Fake> = (0..N)
        .map(|i| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            Fake { slot: AtomicU32::new(u32::MAX), key: (seed >> 40) % 64, tie: i as u64 }
        })
        .collect();
    let base = fakes.as_mut_ptr();
    let at = |i: usize| unsafe { base.add(i) };
    let mut h: SlotHeap<Fake> = SlotHeap::new(|f| &f.slot);
    for i in 0..N {
        unsafe { h.push(at(i), (*at(i)).key, (*at(i)).tie) };
    }

    let mut removed = 0usize;
    for i in (0..N).step_by(3) {
        let slot = unsafe { (*at(i)).slot.load(Ordering::Relaxed) } as usize;
        if h.holds(slot, at(i)) {
            unsafe { h.remove(slot) };
            removed += 1;
        }
    }
    let consistent = (0..N)
        .filter(|i| i % 3 != 0)
        .all(|i| h.holds(unsafe { (*at(i)).slot.load(Ordering::Relaxed) } as usize, at(i)));

    let mut last = (0u64, 0u64);
    let mut ordered = true;
    let mut popped = 0usize;
    while let Some(p) = h.pop() {
        let f = unsafe { &*p };
        if (f.key, f.tie) < last || f.tie % 3 == 0 {
            ordered = false;
        }
        last = (f.key, f.tie);
        popped += 1;
    }
    t.check(
        "sched/heap",
        removed == N.div_ceil(3) && consistent && ordered && popped == N - removed,
        format!("{} in, {} out of the middle, {} popped, in order: {}, slots true: {}",
            N, removed, popped, ordered, consistent),
    );
}

/// 130 pids at once, across two words of the bitmap at least: all different,
/// none a task the walk of the live ones can see, and every one given back
fn check_pids(t: &mut Runner) {
    let mut got: Vec<u64> = Vec::new();
    for _ in 0..130 {
        match super::alloc_pid() {
            Some(p) => got.push(p),
            None => break,
        }
    }
    let n = got.len();
    let mut distinct = got.clone();
    distinct.sort_unstable();
    distinct.dedup();
    let in_range = got.iter().all(|&p| p != 0 && (p as usize) < super::MAX_PROCS);
    let unseen = super::find_task(|p| got.contains(&p.pid)).is_none();
    for &p in &got {
        super::free_pid(p);
    }
    t.check(
        "pid/alloc",
        n == 130 && distinct.len() == n && in_range && unseen,
        format!("{} pids, {} distinct, in range: {}, unregistered: {}", n, distinct.len(), in_range, unseen),
    );
}

/// The walk of the pid bitmap sees what the table holds, within what can start
/// or end while it runs, and finds the task running it
fn check_live_walk(t: &mut Runner) {
    let me = super::current_pid();
    let before = super::thread_count();
    let mut walked = 0usize;
    super::for_each_task(|_| walked += 1);
    let after = super::thread_count();
    let lo = before.min(after).saturating_sub(4);
    let hi = before.max(after) + 4;
    let found = super::find_task(|p| p.pid == me).is_some();
    t.check(
        "pid/live-walk",
        found && (lo..=hi).contains(&walked),
        format!("{} walked, the table held {} then {}, found pid {}: {}", walked, before, after, me, found),
    );
}

/// The running task moved into a job of its own and back: a member of the
/// new one alone, out of the old one meanwhile, and home again after
fn check_jobs(t: &mut Runner) {
    let me = super::current_pid();
    // The boot path has no process to move, and a thread is in no job index
    if me == 0 || super::is_thread(me) {
        return;
    }
    const AWAY: u64 = 999_998;
    let home = super::pgid_of(me);
    let member = |pgid: u64| {
        let mut buf = [0u64; 32];
        let mut from = 0usize;
        let mut all: Vec<u64> = Vec::new();
        loop {
            let (n, next) = super::group_members(pgid, from, &mut buf);
            all.extend_from_slice(&buf[..n]);
            if next >= super::MAX_PROCS { break; }
            from = next;
        }
        all
    };

    let moved = super::set_pgid(me, AWAY);
    let away = member(AWAY);
    let alone = away == [me];
    let after_me = super::group_members(AWAY, me as usize + 1, &mut [0u64; 4]).0 == 0;
    let alive = super::group_alive(AWAY, 0) && !super::group_alive(AWAY, me);
    let left = !member(home).contains(&me);
    let back = super::set_pgid(me, home);
    let empty = member(AWAY).is_empty();
    let returned = member(home).contains(&me) && super::pgid_of(me) == home;
    t.check(
        "job/index",
        moved && alone && after_me && alive && left && back && empty && returned,
        format!("away {:?}, past me empty: {}, alive: {}, left home: {}, empty after: {}, home again: {}",
            away, after_me, alive, left, empty, returned),
    );
}
