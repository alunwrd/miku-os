// gpudemo: GPU use from an unprivileged ring-3 program
//
// Allocates two pools, fills one, has the copy engine move a rectangle into the
// other, waits for the fence, and checks every byte. Then:
// - `gpu_info` reports what can execute a submission;
// - a blit between owned pools is accepted, and the fence passes;
// - a rectangle outside its pool and an unowned handle are refused;
// - a video memory pool (`gpu_alloc`) starts zeroed, the engine copies in and
//   out, BAR1 writes are what the engine reads, mixed directions in one
//   submission are refused, and a compute program writes in place;
// - a window buffer in that memory (`dev/gpu-window-buffers.md`) is drawn by
//   the compute program and shown in place.
#![no_std]
#![no_main]

pub use miku;

use libmay::conn::{gpu_bind_raw, gpu_blits, gpu_dispatch, gpu_errno, gpu_info, gpu_program,
                   gpu_wait, Conn, Event, GpuBlit, GpuParam, Pool};
use libmay::protocol::{self, Interface};
use libmay::requests::Requests;

/// Pitch wider than the line, to expose stride bugs
const W: usize = 64;
const H: usize = 32;
const BPP: usize = 4;
const PITCH: usize = 256;
const POOL: usize = PITCH * (H + 4);

/// `faults <rc> <mmu>` line from `gpu_info`, or "?"
fn faults_line(info: &[u8]) -> &str {
    let Ok(s) = core::str::from_utf8(info) else { return "?" };
    for l in s.lines() {
        if let Some(rest) = l.strip_prefix("faults ") {
            return rest;
        }
    }
    "?"
}

/// The two counts of the `faults` line, `(0, 0)` if there is none
fn fault_counts(info: &[u8]) -> (u64, u64) {
    let mut w = faults_line(info).split(' ').map(|t| t.parse::<u64>().unwrap_or(0));
    (w.next().unwrap_or(0), w.next().unwrap_or(0))
}

/// Does `gpu_info` carry the line `key value`?
fn info_says(info: &[u8], key: &str, value: &str) -> bool {
    let Ok(s) = core::str::from_utf8(info) else { return false };
    s.lines().any(|l| {
        let mut w = l.split(' ');
        w.next() == Some(key) && w.next() == Some(value)
    })
}

/// Finds a program's index in `gpu_info`
fn program_index(info: &[u8], want: &str) -> Option<u32> {
    let s = core::str::from_utf8(info).ok()?;
    for line in s.lines() {
        let mut w = line.split(' ');
        if w.next() != Some("program") {
            continue;
        }
        let idx = w.next()?;
        if w.next() == Some(want) {
            return idx.parse().ok();
        }
    }
    None
}

/// Prints a refusal and its errno
fn report(what: &str, code: i64) {
    miku::print("gpudemo: ");
    miku::print(what);
    miku::print(" - errno ");
    miku::print_int(code);
    miku::print(" (");
    miku::print(gpu_errno(code));
    miku::println(")");
}

fn fail(what: &str) -> ! {
    miku::print("gpudemo: FAIL - ");
    miku::println(what);
    miku::exit(1);
}

/// Loads `/etc/gpu/fill.cubin` through `gpu_program` and runs it. Needs
/// `CAP_GPU`; a refusal is printed and the demo continues
fn user_program(dst: &mut Pool, builtin: u32) {
    // Fixed buffer, so the program needs no heap
    static mut CUBIN: [u8; 64 * 1024] = [0; 64 * 1024];

    const PATH: &str = "/etc/gpu/fill.cubin";
    let Ok(fd) = miku::open(PATH) else {
        miku::print("gpudemo: no ");
        miku::print(PATH);
        miku::println(" - skipping the user-program case");
        return;
    };
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(CUBIN) };
    let got = miku::read(fd, buf);
    miku::close(fd);
    if got <= 0 {
        fail("the cubin would not read");
    }
    let cubin = &buf[..got as usize];

    // `fill` uses 8 registers and no shared memory (`cuobjdump -res-usage`)
    let id = gpu_program(cubin, 8, 0);
    if id < 0 {
        report("gpu_program was refused", id);
        // -EPERM: no `CAP_GPU`
        if id == -1 {
            miku::println("gpudemo: (that is CAP_GPU doing its job)");
            return;
        }
        fail("gpu_program");
    }
    if id as u32 == builtin {
        fail("gpu_program returned the built-in program's index");
    }
    miku::print("gpudemo: ok - a cubin of our own loaded as program ");
    miku::print_int(id);
    miku::println("");

    // Different base from the built-in case, so the earlier result cannot pass
    const N: u32 = 64;
    const BASE: u32 = 0x0200_0000;
    for byte in dst.bytes().iter_mut() {
        *byte = 0;
    }
    let params = [
        GpuParam::sized(dst.handle, 0),
        GpuParam::u32(BASE),
        GpuParam::u32(N),
    ];
    let r = gpu_dispatch(id as u32, (N, 1, 1), (1, 1, 1), &params);
    if r < 0 {
        report("the dispatch of our own program was refused", r);
        fail("gpu_dispatch");
    }
    if !gpu_wait(r as u64, 1_000_000) {
        fail("our own program's fence never passed");
    }
    let b = dst.bytes();
    for i in 0..N as usize {
        let got = u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
        if got != BASE + i as u32 {
            fail("our own program did not write what it should have");
        }
    }
    miku::println("gpudemo: ok - a program this process loaded ran on the GPU");

    // A malformed cubin must be refused by name
    let truncated = &cubin[..cubin.len() / 2];
    let r = gpu_program(truncated, 8, 0);
    if r >= 0 {
        fail("half a cubin was accepted");
    }
    miku::println("gpudemo: ok - a truncated cubin is refused");
}

/// Video memory (`gpu_alloc`): read in place by the GPU, reached through BAR1.
/// Skipped without a BAR1 window (QEMU, `nobar1`)
fn video_memory(src: &mut Pool, fill: Option<u32>) {
    let mut v = match Pool::video(POOL) {
        Ok(p) => p,
        Err(code) => {
            report("no video memory", code);
            if code == -19 {
                miku::println("gpudemo: (no BAR1 window here - the video-memory cases are skipped)");
                return;
            }
            fail("gpu_alloc");
        }
    };
    // Must start zeroed
    if v.bytes().iter().any(|b| *b != 0) {
        fail("video memory came back with something in it");
    }
    miku::println("gpudemo: ok - a pool of video memory, zeroed, mapped through BAR1");

    // System memory to video memory and back, every byte
    let Some(mut back) = Pool::new(POOL) else { fail("no pool to copy back into") };
    for byte in back.bytes().iter_mut() {
        *byte = 0x5a;
    }
    let whole = GpuBlit {
        src_off: 0, dst_off: 0, pitch_in: PITCH as u32, pitch_out: PITCH as u32,
        line_len: (W * BPP) as u32, lines: H as u32,
    };
    let Some(f) = src.blit_to(&v, &whole) else { fail("a copy into video memory was refused") };
    if !gpu_wait(f, 1_000_000) {
        fail("the copy into video memory never finished");
    }
    let Some(f) = v.blit_to(&back, &whole) else { fail("a copy out of video memory was refused") };
    if !gpu_wait(f, 1_000_000) {
        fail("the copy out of video memory never finished");
    }
    {
        let (want, got) = (src.bytes().as_ptr(), back.bytes());
        for y in 0..H {
            for x in 0..W * BPP {
                if got[y * PITCH + x] != unsafe { *want.add(y * PITCH + x) } {
                    fail("system -> video -> system came back different");
                }
            }
        }
    }
    miku::println("gpudemo: ok - 64x32 into video memory and back out, every byte");

    // Writes through BAR1 are what the engine reads
    for y in 0..H {
        for x in 0..W * BPP {
            v.bytes()[y * PITCH + x] = (x as u8) ^ (y as u8).wrapping_mul(7);
        }
    }
    let Some(f) = v.blit_to(&back, &whole) else { fail("a copy out of video memory was refused") };
    if !gpu_wait(f, 1_000_000) {
        fail("the copy out of video memory never finished");
    }
    for y in 0..H {
        for x in 0..W * BPP {
            if back.bytes()[y * PITCH + x] != (x as u8) ^ (y as u8).wrapping_mul(7) {
                fail("what was written through BAR1 is not what the engine read");
            }
        }
    }
    miku::println("gpudemo: ok - what this process wrote through BAR1 is what the copy engine read");

    // Mixed directions in one submission are refused, since memory type is set
    // once per submission
    let mixed = gpu_blits(&[(&*src, &v, &whole), (&v, &back, &whole)]);
    if mixed != -22 {
        if mixed >= 0 {
            let _ = gpu_wait(mixed as u64, 1_000_000);
        }
        report("one submission both into and out of video memory", mixed);
        fail("a submission mixing two pairs of memories was not refused with EINVAL");
    }
    miku::println("gpudemo: ok - one submission both into and out of video memory is refused");

    // And a compute program writing into it in place
    if let Some(fill) = fill {
        let bound = gpu_bind_raw(&v);
        if bound < 0 {
            report("the video-memory pool could not be bound", bound);
            fail("gpu_bind");
        }
        const N: u32 = 64;
        const BASE: u32 = 0x0300_0000;
        let params = [GpuParam::sized(v.handle, 0), GpuParam::u32(BASE), GpuParam::u32(N)];
        let r = gpu_dispatch(fill, (N, 1, 1), (1, 1, 1), &params);
        if r < 0 {
            report("the dispatch over video memory was refused", r);
            fail("gpu_dispatch");
        }
        if !gpu_wait(r as u64, 1_000_000) {
            fail("the dispatch over video memory never finished");
        }
        let b = v.bytes();
        for i in 0..N as usize {
            let got = u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
            if got != BASE + i as u32 {
                fail("the compute program did not write what it should have into video memory");
            }
        }
        miku::println("gpudemo: ok - a compute program wrote 64 elements into video memory, read through BAR1");
    }
    back.free();
    v.free();
}

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    let mut info = [0u8; 1024];
    let n = gpu_info(&mut info);
    // `backend none` is how a machine with no card answers (QEMU)
    if n == 0 || info_says(&info[..n], "backend", "none") {
        miku::println("gpudemo: the kernel reports no GPU that can take a submission");
        miku::exit(0);
    }
    miku::print("gpudemo: ");
    if let Ok(s) = core::str::from_utf8(&info[..n]) {
        for line in s.lines() {
            miku::print(line);
            miku::print("  ");
        }
    }
    miku::println("");

    let Some(mut src) = Pool::new(POOL) else { fail("no source pool") };
    let Some(mut dst) = Pool::new(POOL) else { fail("no destination pool") };

    // Different in every byte of every pixel
    {
        let b = src.bytes();
        for y in 0..H {
            for x in 0..W {
                let o = y * PITCH + x * BPP;
                b[o] = (x as u8).wrapping_mul(3);
                b[o + 1] = (y as u8).wrapping_mul(5);
                b[o + 2] = (x as u8) ^ (y as u8);
                b[o + 3] = 0xff;
            }
        }
    }
    // Clears the destination first
    for byte in dst.bytes().iter_mut() {
        *byte = 0x5a;
    }

    // One row down, to expose an ignored dst_off
    let dst_off = PITCH as u64;
    let blit = GpuBlit {
        src_off: 0,
        dst_off,
        pitch_in: PITCH as u32,
        pitch_out: PITCH as u32,
        line_len: (W * BPP) as u32,
        lines: H as u32,
    };

    let Some(fence) = src.blit_to(&dst, &blit) else { fail("the submission was refused") };
    if !gpu_wait(fence, 1_000_000) {
        fail("the fence never passed");
    }

    // Every byte moved
    {
        let expect = src.bytes().as_ptr();
        let got = dst.bytes();
        for y in 0..H {
            for x in 0..W * BPP {
                let want = unsafe { *expect.add(y * PITCH + x) };
                if got[dst_off as usize + y * PITCH + x] != want {
                    fail("the destination does not match the source");
                }
            }
            // The gap between lines must stay untouched
            for x in W * BPP..PITCH {
                if got[dst_off as usize + y * PITCH + x] != 0x5a {
                    fail("the engine wrote past the end of a line");
                }
            }
        }
    }
    miku::println("gpudemo: ok - the copy engine moved 64x32 and every byte matches");

    // Refusals
    let too_far = GpuBlit {
        src_off: 0,
        dst_off: (POOL - 16) as u64,
        pitch_in: PITCH as u32,
        pitch_out: PITCH as u32,
        line_len: (W * BPP) as u32,
        lines: H as u32,
    };
    if src.blit_to(&dst, &too_far).is_some() {
        fail("a rectangle reaching outside its pool was accepted");
    }
    miku::println("gpudemo: ok - a rectangle past the end of a pool is refused");

    let thin = GpuBlit {
        src_off: 0,
        dst_off: 0,
        pitch_in: 16,
        pitch_out: PITCH as u32,
        line_len: (W * BPP) as u32,
        lines: H as u32,
    };
    if src.blit_to(&dst, &thin).is_some() {
        fail("a pitch narrower than the line was accepted");
    }
    miku::println("gpudemo: ok - a pitch narrower than its line is refused");

    // Compute: `fill` writes `value + i`, launched from ring 3 with the kernel
    // resolving addresses from pool handles
    if let Some(fill) = program_index(&info[..n], "fill") {
        let bound = gpu_bind_raw(&dst);
        if bound < 0 {
            report("the pool could not be bound", bound);
            fail("gpu_bind");
        }

        const N: u32 = 64;
        // Base with high bits set: `BASE + i` means it ran, 0 means nothing was
        // written
        const BASE: u32 = 0x0100_0000;
        for byte in dst.bytes().iter_mut() {
            *byte = 0;
        }
        let params = [
            GpuParam::sized(dst.handle, 0),
            GpuParam::u32(BASE),
            GpuParam::u32(N),
        ];
        let r = gpu_dispatch(fill, (N, 1, 1), (1, 1, 1), &params);
        if r < 0 {
            report("the dispatch was refused", r);
            fail("gpu_dispatch");
        }
        let fence = r as u64;
        if !gpu_wait(fence, 1_000_000) {
            // Fault counters distinguish an engine fault from an engine that
            // never started
            let mut after = [0u8; 1024];
            let m = gpu_info(&mut after);
            miku::print("gpudemo: robust-channel errors and MMU faults, before and after: ");
            miku::print(faults_line(&info[..n]));
            miku::print(" -> ");
            miku::print(faults_line(&after[..m]));
            miku::println("");
            fail("the dispatch fence never passed");
        }
        // Reports how many elements are untouched and the first wrong value
        let b = dst.bytes();
        let at = |i: usize| u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
        let mut zeros = 0usize;
        let mut first_bad: Option<usize> = None;
        for i in 0..N as usize {
            let got = at(i);
            if got == 0 {
                zeros += 1;
            }
            if got != BASE + i as u32 && first_bad.is_none() {
                first_bad = Some(i);
            }
        }
        if let Some(i) = first_bad {
            miku::print("gpudemo: of 64 elements ");
            miku::print_int(zeros as i64);
            miku::print(" are still zero; first wrong is [");
            miku::print_int(i as i64);
            miku::print("] = ");
            miku::print_hex(at(i) as u64);
            miku::print(", wanted ");
            miku::print_hex((BASE + i as u32) as u64);
            miku::println("");
            fail("the compute program did not write what it should have");
        }
        miku::println("gpudemo: ok - a compute program ran on 64 elements and every one matches");

        // An offset past the pool is refused
        let bad = [
            GpuParam::sized(dst.handle, POOL as u64),
            GpuParam::u32(BASE),
            GpuParam::u32(N),
        ];
        if gpu_dispatch(fill, (N, 1, 1), (1, 1, 1), &bad) >= 0 {
            fail("an offset past the end of a pool was accepted");
        }
        miku::println("gpudemo: ok - an offset past the end of a pool is refused");

        // A count past the pool is not refused, but `fill` gets the pool size
        // and stops there
        let (rc0, faults0) = fault_counts(&info[..n]);
        let past = [
            GpuParam::sized(dst.handle, 0),
            GpuParam::u32(BASE),
            GpuParam::u32((POOL / 4 + 4096) as u32),
        ];
        let blocks = (POOL / 4 + 4096).div_ceil(256) as u32;
        let r = gpu_dispatch(fill, (blocks, 1, 1), (256, 1, 1), &past);
        if r < 0 {
            report("the dispatch with a count past the pool was refused", r);
            fail("gpu_dispatch");
        }
        if !gpu_wait(r as u64, 1_000_000) {
            fail("the dispatch with a count past the pool never finished");
        }
        let mut after = [0u8; 1024];
        let m = gpu_info(&mut after);
        let (rc1, faults1) = fault_counts(&after[..m]);
        let b = dst.bytes();
        let last = POOL / 4 - 1;
        let at_last = u32::from_le_bytes([b[last * 4], b[last * 4 + 1], b[last * 4 + 2], b[last * 4 + 3]]);
        if (rc1, faults1) != (rc0, faults0) || at_last != BASE + last as u32 {
            miku::print("gpudemo: robust-channel errors and MMU faults, before and after: ");
            miku::print(faults_line(&info[..n]));
            miku::print(" -> ");
            miku::print(faults_line(&after[..m]));
            miku::println("");
            fail("fill with a count past the pool did not stop at the pool's end");
        }
        miku::println("gpudemo: ok - fill with a count past the pool fills the pool and stops");

        // `CAP_GPU` path: a cubin from a file, parsed by the kernel, uploaded,
        // launched under a new index
        user_program(&mut dst, fill);
    } else if info_says(&info[..n], "compute", "down")
        || info_says(&info[..n], "compute", "unloaded")
    {
        miku::println("gpudemo: the compute channel is not up - skipping the dispatch cases");
    } else {
        miku::println("gpudemo: no compute programs in this build (no CUDA toolkit) - skipping");
    }

    // and video memory
    video_memory(&mut src, program_index(&info[..n], "fill"));

    // and a window in it
    gpu_window(program_index(&info[..n], "fill"));

    // and what the card is doing
    card_numbers();

    miku::exit(0);
}

/// Window size
const WIN_W: i32 = 640;
const WIN_H: i32 = 360;
const WIN_STRIDE: i32 = WIN_W * 4;
const WIN_BYTES: usize = (WIN_STRIDE * WIN_H) as usize;
/// Ten seconds at 240 Hz
const WIN_FRAMES: u32 = 2400;
/// Adding a row's worth of pixels to `value` scrolls the pattern one row per
/// frame
const WIN_STEP: u32 = WIN_W as u32;

/// Window with two video memory buffers, drawn by `fill` and shown in place.
/// Skipped with no program, compositor, or video memory
fn gpu_window(fill: Option<u32>) {
    let Some(fill) = fill else {
        miku::println("gpudemo: no fill program - skipping the window in video memory");
        return;
    };
    let Some(mut c) = Conn::open() else {
        miku::println("gpudemo: no display server - skipping the window in video memory");
        return;
    };
    let (registry, globals) = c.handshake();
    if globals.failed
        || !globals.has(Interface::Shm)
        || !globals.has(Interface::Compositor)
        || !globals.has(Interface::Xdg)
    {
        miku::println("gpudemo: the display server has no windows to offer - skipping the window");
        c.close();
        return;
    }
    let mut v = match Pool::video(WIN_BYTES * 2) {
        Ok(p) => p,
        Err(code) => {
            report("no video memory for the window", code);
            miku::println("gpudemo: (skipping the window in video memory)");
            c.close();
            return;
        }
    };
    let bound = gpu_bind_raw(&v);
    if bound < 0 {
        report("the window's pool could not be bound", bound);
        fail("gpu_bind");
    }

    let shm = c.bind(registry, globals.name(Interface::Shm), Interface::Shm, 1);
    let comp = c.bind(registry, globals.name(Interface::Compositor), Interface::Compositor, 1);
    let xdg = c.bind(registry, globals.name(Interface::Xdg), Interface::Xdg, 1);
    // Video memory pool
    let pool_obj = c.create_pool(shm, v.handle, (WIN_BYTES * 2) as i32);
    let mut buffers = [(0u32, true); 2];
    for (i, b) in buffers.iter_mut().enumerate() {
        b.0 = c.create_buffer(pool_obj, (i * WIN_BYTES) as i32, WIN_W, WIN_H, WIN_STRIDE,
                              protocol::shm::FORMAT_XRGB8888);
    }
    let surface = c.create_surface(comp);
    let xdg_surface = c.get_xdg_surface(xdg, surface);
    let toplevel = c.get_toplevel(xdg_surface);
    c.set_title(toplevel, "gpudemo");
    c.flush();

    let mut evs = [Event::Other { object: 0, opcode: 0 }; 32];
    let mut configured = false;
    while !configured {
        let n = c.read_events(&mut evs);
        if n == 0 {
            fail("no configure arrived for the window");
        }
        for e in &evs[..n] {
            match *e {
                Event::XdgConfigure { serial, .. } => {
                    c.ack_configure(xdg_surface, serial);
                    configured = true;
                }
                Event::Error { msg, msg_len, .. } => protocol_error(&msg[..msg_len]),
                _ => {}
            }
        }
    }
    c.flush();

    // The program draws into a released buffer, and the commit carries its
    // fence for the compositor to wait on
    let blocks = (WIN_W * WIN_H) as u32 / 256;
    let mut frames = 0u32;
    let mut armed = true;
    let mut frame_cb = 0u32;
    let mut closed = false;
    while frames < WIN_FRAMES && !closed {
        if !armed {
            let n = c.read_events(&mut evs);
            if n == 0 {
                miku::print("gpudemo: ");
                miku::println(if c.closed { c.death } else { "no frame callback arrived" });
                fail("the window stopped being drawn");
            }
            for e in &evs[..n] {
                match *e {
                    Event::BufferRelease { id } => {
                        for b in buffers.iter_mut().filter(|b| b.0 == id) {
                            b.1 = true;
                        }
                    }
                    Event::Done { id, .. } if id == frame_cb => armed = true,
                    Event::XdgConfigure { serial, .. } => c.ack_configure(xdg_surface, serial),
                    Event::Close { .. } => closed = true,
                    Event::Error { msg, msg_len, .. } => protocol_error(&msg[..msg_len]),
                    _ => {}
                }
            }
            if !armed || closed {
                continue;
            }
        }
        let Some(slot) = buffers.iter().position(|b| b.1) else {
            armed = false;
            continue;
        };
        let params = [
            GpuParam::sized(v.handle, (slot * WIN_BYTES) as u64),
            GpuParam::u32(frames * WIN_STEP),
            GpuParam::u32((WIN_W * WIN_H) as u32),
        ];
        let fence = gpu_dispatch(fill, (blocks, 1, 1), (256, 1, 1), &params);
        if fence < 0 {
            report("a frame's dispatch was refused", fence);
            fail("gpu_dispatch");
        }
        c.set_fence(surface, fence as u64);
        c.attach(surface, buffers[slot].0, 0, 0);
        c.damage(surface, 0, 0, WIN_W, WIN_H);
        frame_cb = c.frame(surface);
        c.commit(surface);
        c.flush();
        buffers[slot].1 = false;
        armed = false;
        frames += 1;
    }

    c.destroy_surface(surface);
    c.flush();
    v.free();
    c.close();
    if closed {
        miku::print("gpudemo: the compositor closed the window after ");
        miku::print_int(frames as i64);
        miku::println(" frames drawn by the GPU into video memory");
        return;
    }
    miku::print("gpudemo: ok - ");
    miku::print_int(frames as i64);
    miku::println(" frames drawn by the GPU into video memory, shown in place");
}

/// The compositor closed the connection with an error
fn protocol_error(msg: &[u8]) -> ! {
    miku::print("gpudemo: protocol error - ");
    miku::println(core::str::from_utf8(msg).unwrap_or("(unprintable)"));
    fail("the compositor refused the window in video memory");
}

/// `gpu_info` lines for P-state, clocks, load, power, temperatures, and fans
/// (`docs/MikuOS_ABI.md`, 3.2a). The first read starts GSP-RM polling, so a
/// second read follows after 500 ms
fn card_numbers() {
    const KEYS: [&str; 6] = ["perf", "clock", "util", "power", "temp", "fan"];
    for attempt in 0..4 {
        let mut info = [0u8; 1024];
        let n = gpu_info(&mut info);
        let Ok(s) = core::str::from_utf8(&info[..n]) else { return };
        let mut said = false;
        for line in s.lines().filter(|l| KEYS.iter().any(|k| l.split(' ').next() == Some(*k))) {
            miku::print("gpudemo: the card - ");
            miku::println(line);
            said = true;
        }
        if said {
            return;
        }
        if attempt < 3 {
            miku::sleep_ms(500);
        }
    }
    miku::println("gpudemo: the card says nothing about its clocks - no page from GSP-RM, or it is not polling");
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::println("gpudemo: PANIC");
    miku::exit(1);
}
