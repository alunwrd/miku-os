// The GPU, from ring 3
//
//      gpu_info(buf, len)            what this machine's GPU can be asked for
//      gpu_submit(args_ptr)          queue work, get a fence back
//      gpu_wait(fence, timeout_us)   wait for that fence
//      gpu_alloc(size, 0, out)       a pool of video memory (3.7)
//
// Why syscalls, and not a device node
//
// `/dev/dri` would promise a DRM ABI this machine does not have, `dev_read` and
// `dev_write` mean nothing on a GPU, and a devfs node makes the gate a mode bit
// on a file, which `docs/MikuOS_Security.md` is built on not doing.
//
// Why a pool handle, and not a new object
//
// A `may_pool` handle carries what a GPU buffer needs: ownership by pid, a
// per-owner quota, teardown when the process exits, and retire-before-free so
// the allocator never sees a page an engine is reading. It is also a contiguous
// run of frames, which the copy engine can read.
//
// What is not here
//
// No capability is required for this: the kernel picks every address the engine
// touches and refuses any rectangle not inside a pool the caller owns.
// `CAP_GPU` guards arbitrary machine code, `gpu_program`.

use crate::syscall::errno::*;
use crate::syscall::usercopy::{current_cr3, current_pid, user_ptr_mapped, user_ptr_writable};
use crate::mayland::server::shm;

#[inline]
fn e(code: i64) -> u64 { code as u64 }

/// Refuses a call, and says which check refused it.
///
/// A client gets a number meaning "invalid argument" about a call with a dozen
/// arguments, so the reason is reported as a dispatch refusal is: the first few
/// on the console, the rest to the serial port, so a client in a loop cannot
/// push the screen away
fn refuse(code: i64, why: &str) -> u64 {
    use core::sync::atomic::{AtomicU32, Ordering};
    static SEEN: AtomicU32 = AtomicU32::new(0);
    const ON_SCREEN: u32 = 4;

    let n = SEEN.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[gpu] refused ({}): {}", code, why);
    if n < ON_SCREEN {
        crate::println!("  gpu: refused - {}", why);
    } else if n == ON_SCREEN {
        crate::println!("  gpu: further refusals go to the serial port only");
    }
    e(code)
}

/// Operations `gpu_submit` understands
pub const GPU_OP_BLIT: u64 = 1;
pub const GPU_OP_DISPATCH: u64 = 2;
/// Up to `MAX_CHAIN` dispatches, run in order, one fence (`dev/aosh-gpu.md`)
pub const GPU_OP_CHAIN: u64 = 3;

/// The longest chain: the launch slots ring 3 has
const MAX_CHAIN: usize = crate::drivers::gpu::nvidia::gsp_common::compute::CLIENT_SLOTS;

/// Parameter kinds a dispatch may carry.
///
/// A buffer is named by a bound handle, never by an address, which is why a
/// dispatch needs no capability: the kernel resolves the address itself, from a
/// binding it made, for a pool this process owns. A raw GPU address in a
/// constant bank could read the console's shadow buffer, another client's
/// pixels, or the graphics context
pub const PARAM_U32: u32 = 0;
pub const PARAM_U64: u32 = 1;
pub const PARAM_BUFFER: u32 = 2;
/// A buffer and its size: the address, then how many bytes there are from the
/// offset to the end of the pool, so a built-in program taking a buffer can
/// keep every access inside
pub const PARAM_BUFFER_SIZED: u32 = 3;

/// The largest cubin `gpu_program` will read, and the limits on what a caller
/// may declare about the kernel inside it.
///
/// A cubin holding one kernel is kilobytes; a megabyte bounds what a process
/// can make the kernel copy in one call
const MAX_CUBIN: usize = 1 << 20;
/// Registers per thread is an 8-bit field of the launch descriptor
const MAX_REGS: u32 = 255;
/// Shared memory per block on this architecture
const MAX_SHARED: u32 = 228 * 1024;

/// How many parameters one dispatch may carry
const MAX_PARAMS: usize = 16;

/// A pool bound into the GPU's address space: `(pid, handle, slot)`
static BINDS: spin::Mutex<alloc::vec::Vec<(u64, u32, usize)>> =
    spin::Mutex::new(alloc::vec::Vec::new());

/// Rectangles in one submission. The copy engine's own batch limit, so a
/// submission that passes here is one the engine can be given whole
const MAX_RECTS: usize = 48;

/// Submissions a process may have in flight at once.
///
/// One: the copy engine is shared with the compositor, and `submit_batch` waits
/// for the previous fence at the head of the ring, so a client queuing without
/// bound would put the compositor's next frame behind its work. At one, a
/// client waits for its own fence before asking again, which bounds what it can
/// insert between two frames to a single batch.
///
/// A channel of its own is the other answer; a batch is microseconds and a
/// second channel is a graphics context
const MAX_IN_FLIGHT: usize = 1;

/// What a process has outstanding: `(pid, fence)`
static IN_FLIGHT: spin::Mutex<alloc::vec::Vec<(u64, u64)>> =
    spin::Mutex::new(alloc::vec::Vec::new());

/// One rectangle, as `gpu_submit` receives it
#[derive(Clone, Copy)]
struct Rect {
    src_handle: u32,
    dst_handle: u32,
    src_off: u64,
    dst_off: u64,
    pitch_in: u32,
    pitch_out: u32,
    line_len: u32,
    lines: u32,
}

impl Rect {
    /// 48 bytes, and the layout a client writes
    const SIZE: usize = 48;

    fn read(b: &[u8]) -> Self {
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let u64_at = |o: usize| {
            let mut v = [0u8; 8];
            v.copy_from_slice(&b[o..o + 8]);
            u64::from_le_bytes(v)
        };
        Rect {
            src_handle: u32_at(0),
            dst_handle: u32_at(4),
            src_off: u64_at(8),
            dst_off: u64_at(16),
            pitch_in: u32_at(24),
            pitch_out: u32_at(28),
            line_len: u32_at(32),
            lines: u32_at(36),
        }
    }

    /// The last byte this rectangle touches on one side, or `None` if the
    /// arithmetic overflows - which is a rejection and not a large number
    fn extent(off: u64, pitch: u32, line_len: u32, lines: u32) -> Option<u64> {
        if lines == 0 || line_len == 0 {
            return None;
        }
        // The last row starts at (lines - 1) * pitch and is line_len long. A
        // pitch shorter than a line overlaps rows, which the engine will do and
        // nobody means
        if pitch < line_len {
            return None;
        }
        let last_row = (lines as u64 - 1).checked_mul(pitch as u64)?;
        off.checked_add(last_row)?.checked_add(line_len as u64)
    }
}

/// Encodes a fence so one from a rebuilt channel can never read as passed.
///
/// The low 32 bits are the channel's monotonic counter; the next 30 are the
/// channel's epoch, which the teardown moves. Without the epoch, a client
/// holding a fence across a rebuild would compare it against a counter that
/// started again.
///
/// Bit 63 is never set, so the epoch is 30 bits: a syscall's return reaches the
/// caller as an `i64`, where a negative number is an errno
const FENCE_ENGINE_BIT: u32 = 62;
const EPOCH_BITS: u32 = 30;
const EPOCH_MASK: u64 = (1u64 << EPOCH_BITS) - 1;

pub fn encode_fence(epoch: u32, seq: u32) -> u64 {
    ((epoch as u64 & EPOCH_MASK) << 32) | seq as u64
}

pub fn decode_fence(fence: u64) -> (u32, u32) {
    (((fence >> 32) & EPOCH_MASK) as u32, fence as u32)
}

/// gpu_info(buf, len) -> bytes written | errno
///
/// Plain text: a program wanting to know whether it can blit reads this, and a
/// person debugging reads the same
pub fn sys_gpu_info(buf: u64, len: u64) -> u64 {
    // What this process can do: a process with a GPU of its own has its own
    // program list, and the built-in list is shared by everyone else
    let pid = current_pid();
    use core::fmt::Write;

    let cr3 = current_cr3();
    if len == 0 || len > 4096 {
        return e(EINVAL);
    }
    let Some(_ua) = user_ptr_writable(cr3, buf, len) else {
        return e(EFAULT);
    };

    let mut s = alloc::string::String::new();
    match gpu_backend() {
        Some(b) => {
            let _ = writeln!(s, "backend {}", b);
            let _ = writeln!(s, "max_rects {}", MAX_RECTS);
            // What GSP-RM has complained about. A dispatch accepted and never
            // finished has two explanations: the engine faulted, or it never
            // started. These two numbers tell them apart
            let (rc, faults) = crate::drivers::gpu::nvidia::blackwell::boot::trouble_counts();
            let _ = writeln!(s, "faults {} {}", rc, faults);
            let _ = writeln!(s, "max_in_flight {}", MAX_IN_FLIGHT);
            // Video memory for programs (`gpu_alloc`): how much is held of how
            // much, or why there is none: BAR1 not in physical mode is the
            // `nobar1` machine, and its programs keep to system memory
            let (held, limit) = crate::drivers::gpu::nvidia::user_vram::held();
            if crate::drivers::gpu::nvidia::blackwell::bar1::physical().is_some() {
                let _ = writeln!(s, "vram {} of {} MiB for programs, BAR1 physical",
                                 (limit - held) >> 20, limit >> 20);
            } else {
                let _ = writeln!(s, "vram none - BAR1 is not in physical mode");
            }
            // What the card is doing, as GSP-RM writes it into the shared page
            // (`dev/gpu-perf.md`): lines only from fresh numbers. Reading keeps
            // GSP-RM polling (`perf::rusd::touch`), so the first call after a
            // quiet spell has none and the next, half a second on, has them.
            // Nothing here waits on GSP-RM
            if let Some(snap) = crate::drivers::gpu::nvidia::blackwell::boot::perf_snapshot() {
                s.push_str(&crate::drivers::gpu::nvidia::gsp_common::perf::info_lines(&snap));
            }
            // Programs existing is not the same as being able to run one: the
            // compute channel can be torn down or marked dead.
            //
            // The two reasons are reported apart: a build with no CUDA toolkit
            // has no programs, while a machine whose compute channel is down
            // has every program and nowhere to run it
            let built = crate::drivers::gpu::nvidia::blackwell::boot::program_names(pid);
            let up = crate::drivers::gpu::nvidia::blackwell::boot::compute_ready();
            let progs = if up { built.clone() } else { alloc::vec::Vec::new() };
            if progs.is_empty() {
                let _ = writeln!(s, "ops blit");
                // "unbuilt" is about the build, and only `shaders::AVAILABLE`
                // knows that. An empty loaded list means the programs exist and
                // the channel holding them does not
                let _ = writeln!(
                    s, "compute {}",
                    if !crate::drivers::gpu::nvidia::shaders::AVAILABLE {
                        "unbuilt"
                    } else if built.is_empty() {
                        "unloaded"
                    } else {
                        "down"
                    }
                );
            } else {
                let _ = writeln!(s, "ops blit dispatch");
                let _ = writeln!(s, "max_params {}", MAX_PARAMS);
                let _ = writeln!(s, "max_binds {}", MAX_CLIENT_BINDS);
                // How many more programs can be loaded, so a client knows
                // whether `gpu_program` has room before it compiles anything
                let _ = writeln!(
                    s, "program_slots {}",
                    crate::drivers::gpu::nvidia::gsp_common::compute::MAX_PROGRAMS
                        .saturating_sub(progs.len())
                );
                // By name and in index order: the index is what a dispatch
                // carries
                for (i, name) in progs.iter().enumerate() {
                    let _ = writeln!(s, "program {} {}", i, name);
                }
            }
        }
        None => {
            let _ = writeln!(s, "backend none");
        }
    }

    let bytes = s.as_bytes();
    let n = bytes.len().min(len as usize);
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, n);
    }
    n as u64
}

/// The name of whatever can execute a submission, or `None` when nothing can
fn gpu_backend() -> Option<&'static str> {
    crate::drivers::gpu::nvidia::blackwell::boot::copy_engine_ready()
        .then_some("nvidia-ce")
}

/// gpu_submit(args_ptr) -> fence | errno
///
/// `args_ptr` points at six u64s, the shape `mmap_file` and `execve` use when a
/// call needs more than the register convention carries:
///
///     [0] op          GPU_OP_BLIT, GPU_OP_DISPATCH or GPU_OP_CHAIN
///     [1] rects_ptr   an array of `Rect`; a dispatch block; an array of them
///     [2] count       how many rectangles or dispatch blocks
///     [3..6]          zero, and checked to be
pub fn sys_gpu_submit(args_ptr: u64) -> u64 {
    let cr3 = current_cr3();
    let Some(_ua) = user_ptr_mapped(cr3, args_ptr, 48) else {
        return refuse(EFAULT, "the submission block is not readable");
    };
    let args: [u64; 6] = unsafe { core::ptr::read_unaligned(args_ptr as *const [u64; 6]) };
    // Reserved words are checked rather than ignored: a program setting one is
    // written against an ABI this kernel does not have
    if args[3] != 0 || args[4] != 0 || args[5] != 0 {
        return refuse(EINVAL, "a reserved word of the submission is not zero");
    }
    match args[0] {
        GPU_OP_BLIT => blit(cr3, args[1], args[2]),
        GPU_OP_DISPATCH => dispatch(cr3, args[1], 1),
        GPU_OP_CHAIN => dispatch(cr3, args[1], args[2]),
        other => refuse(EINVAL, &alloc::format!("submission op {}", other)),
    }
}

fn blit(cr3: u64, rects_ptr: u64, count: u64) -> u64 {
    use crate::drivers::gpu::nvidia::gsp_common::chan::Copy2d;

    if count == 0 || count as usize > MAX_RECTS {
        return e(EINVAL);
    }
    let bytes = count as usize * Rect::SIZE;
    let Some(_ua) = user_ptr_mapped(cr3, rects_ptr, bytes as u64) else {
        return e(EFAULT);
    };
    if gpu_backend().is_none() {
        return e(ENODEV);
    }
    let pid = current_pid();

    // One in flight. Checked before the work below, so a client over its limit
    // does not pay for the validation
    match in_flight_ok(pid) {
        Ok(()) => {}
        Err(err) => return err,
    }

    let mut raw = alloc::vec![0u8; bytes];
    unsafe {
        core::ptr::copy_nonoverlapping(rects_ptr as *const u8, raw.as_mut_ptr(), bytes);
    }

    // Every address the engine will touch is resolved here, from a handle this
    // process owns, and bounded by that pool's size. Userspace gives a handle
    // and an offset, and the kernel does the arithmetic, so this needs no
    // capability
    let mut copies: alloc::vec::Vec<Copy2d> = alloc::vec::Vec::with_capacity(count as usize);
    let mut pair: Option<(shm::Memory, shm::Memory)> = None;
    for i in 0..count as usize {
        let r = Rect::read(&raw[i * Rect::SIZE..(i + 1) * Rect::SIZE]);

        let Some(src_end) = Rect::extent(r.src_off, r.pitch_in, r.line_len, r.lines) else {
            return e(EINVAL);
        };
        let Some(dst_end) = Rect::extent(r.dst_off, r.pitch_out, r.line_len, r.lines) else {
            return e(EINVAL);
        };
        let (Ok(src_size), Ok(dst_size)) =
            (shm::claim(r.src_handle, pid), shm::claim(r.dst_handle, pid))
        else {
            return e(EACCES);
        };
        if src_end > src_size as u64 || dst_end > dst_size as u64 {
            return e(EINVAL);
        }
        let (Ok((src_base, src_mem)), Ok((dst_base, dst_mem))) =
            (shm::location(r.src_handle), shm::location(r.dst_handle))
        else {
            return e(EINVAL);
        };
        // A batch is one pair of memories: system to video, video to system, or
        // within one kind. That is the ABI (`MikuOS_ABI.md`, `gpu_blit`). The
        // engine takes a mode per copy (3.10), but lifting the rule is an ABI
        // change
        match pair {
            None => pair = Some((src_mem, dst_mem)),
            Some(p) if p == (src_mem, dst_mem) => {}
            Some(_) => return refuse(EINVAL,
                "one submission copies between one pair of memories - send the others separately"),
        }

        copies.push(Copy2d {
            src: src_base + r.src_off,
            dst: dst_base + r.dst_off,
            pitch_in: r.pitch_in,
            pitch_out: r.pitch_out,
            line_len: r.line_len,
            lines: r.lines,
            // Which memory each end is in: a pool of system memory, or one of
            // video memory (`gpu_alloc`). Never a surface: a client does not
            // own one
            src_sysmem: src_mem == shm::Memory::System,
            dst_sysmem: dst_mem == shm::Memory::System,
        });
    }

    let Some((epoch, seq)) = crate::drivers::gpu::nvidia::blackwell::boot::ce_submit_client_batch(
        &copies,
    ) else {
        return e(EIO);
    };
    let fence = encode_fence(epoch, seq);
    IN_FLIGHT.lock().push((pid, fence));
    fence
}

/// Has this process got room for another submission?
///
/// Also where finished ones are forgotten: a client submitting and waiting
/// never accumulates an entry, and one walking away has its entry cleared the
/// next time it asks
fn in_flight_ok(pid: u64) -> Result<(), u64> {
    let mut t = IN_FLIGHT.lock();
    t.retain(|(p, f)| *p != pid || !fence_passed(*f));
    if t.iter().filter(|(p, _)| *p == pid).count() >= MAX_IN_FLIGHT {
        drop(t);
        return Err(refuse(EAGAIN, "this process already has a submission in flight"));
    }
    Ok(())
}

/// Where the engine stands with this fence: done, still coming, or lost with
/// the channel that was to write it
fn fence_state(fence: u64) -> crate::drivers::gpu::nvidia::blackwell::boot::FenceState {
    use crate::drivers::gpu::nvidia::blackwell::boot;
    let (epoch, seq) = decode_fence(fence);
    // The live epoch is masked the same way the fence's was, or a channel built
    // after the counter passed 2^30 would never match its own fences
    let is_epoch = |live: u32| live as u64 & EPOCH_MASK == epoch as u64;
    if fence & COMPUTE_FENCE != 0 {
        // Compute fences come from the kernel's channel or from a process's
        // own, and the epoch says which
        return boot::compute_fence_state(is_epoch, seq);
    }
    boot::ce_fence_state(is_epoch, seq)
}

/// Is there anything left to wait for?
///
/// A fence from a channel that died or has been rebuilt counts: the work it
/// named is not coming. Whether it finished is `fence_state`'s question, which
/// `gpu_wait` asks
fn fence_passed(fence: u64) -> bool {
    fence_state(fence) != crate::drivers::gpu::nvidia::blackwell::boot::FenceState::Pending
}

/// Waits for a fence from inside the kernel, with a budget. True if it passed.
///
/// For the compositor: a client may commit pixels its own GPU work has not
/// finished, and the client should not stall for a frame it could spend drawing
/// the next one
pub fn wait_fence(fence: u64, budget_us: u64) -> bool {
    let watch = crate::kcore::time::Stopwatch::start();
    loop {
        if fence_passed(fence) {
            return true;
        }
        if watch.elapsed_us() >= budget_us {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// gpu_wait(fence, timeout_us) -> 0 | errno
///
/// `EIO` when the channel the work was on died before it finished (GSP-RM
/// killed it, and the driver may have built a new one). The program's buffers
/// hold whatever the engine got to, and it has to be told: OpenRM's error
/// notifier (`krcErrorSetNotifier`), as an errno
pub fn sys_gpu_wait(fence: u64, timeout_us: u64) -> u64 {
    use crate::drivers::gpu::nvidia::blackwell::boot::FenceState;
    // A budget rather than forever: a fence the engine will never write is the
    // likely failure here, and a syscall that cannot return is worse than one
    // that gave up
    let budget = timeout_us.clamp(1, 2_000_000);
    let watch = crate::kcore::time::Stopwatch::start();
    loop {
        match fence_state(fence) {
            FenceState::Passed => {
                IN_FLIGHT.lock().retain(|(_, f)| *f != fence);
                return 0;
            }
            FenceState::Lost => {
                IN_FLIGHT.lock().retain(|(_, f)| *f != fence);
                return e(EIO);
            }
            FenceState::Pending => {}
        }
        if watch.elapsed_us() >= budget {
            return e(ETIMEDOUT);
        }
        // Off the run queue until an engine's fence interrupt or a tick (3.5a);
        // a yield only when fences cannot wake anyone
        if crate::drivers::gpu::nvidia::gsp_common::chan::fences_wake() {
            crate::drivers::gpu::nvidia::gsp_common::chan::sleep_for_fence(1, || {
                !matches!(fence_state(fence), FenceState::Pending)
            });
        } else {
            crate::sched::yield_now();
        }
    }
}

/// Forgets what a process had outstanding, when it exits: nothing will wait on
/// those fences, and the entries would pin a gone pid
pub fn forget_process(pid: u64) {
    IN_FLIGHT.lock().retain(|(p, _)| *p != pid);
    // And the GPU's own translations to its memory. The pools are freed by the
    // shm teardown just before this, so leaving these would be a use-after-free
    unbind_all(pid);

    // And its own GPU, if it had one: an address space, a channel, a graphics
    // context of several megabytes and two carves of video memory. Only a
    // process that loaded its own programs has any of that
    let say = |m: &str| {
        crate::serial_println!("[gpu] {}", m);
    };
    if crate::drivers::gpu::nvidia::blackwell::boot::client_has_own(pid) {
        crate::drivers::gpu::nvidia::blackwell::boot::client_drop(pid, &say);
    } else if let Some(bar0) = crate::drivers::gpu::nvidia::with_gb20x(|d| d.bar0) {
        // Or the programs it loaded into the kernel's set, which would
        // otherwise hold their slots
        crate::drivers::gpu::nvidia::blackwell::boot::client_forget_programs(&bar0, pid, &say);
    }
}

// bind, and dispatch

/// gpu_bind(handle, flags, out_ptr) -> 0 | errno
///
/// Makes a pool this process owns reachable by the GPU's own MMU and writes its
/// address to `out_ptr`. The address is for handing back as a parameter only:
/// it is not mapped in this process, and arithmetic leaving the pool faults the
/// engine.
///
/// Binding twice is not an error and takes no second slot: it answers with the
/// address it has
pub fn sys_gpu_bind(handle: u64, flags: u64, out_ptr: u64) -> u64 {
    if flags != 0 {
        return e(EINVAL);
    }
    let cr3 = current_cr3();
    let Some(_ua) = user_ptr_writable(cr3, out_ptr, 16) else {
        return e(EFAULT);
    };
    let handle = handle as u32;
    let pid = current_pid();

    let Ok(size) = shm::claim(handle, pid) else { return e(EACCES) };
    // Bound in the memory it is in: a pool of video memory is read by the
    // engine in place
    let Ok((phys, memory)) = shm::location(handle) else { return e(EINVAL) };
    let aperture = match memory {
        shm::Memory::System => crate::drivers::gpu::nvidia::gmmu::Aperture::Sysmem,
        shm::Memory::Video => crate::drivers::gpu::nvidia::gmmu::Aperture::Vram,
    };

    let Some(bar0) = crate::drivers::gpu::nvidia::with_gb20x(|d| d.bar0) else {
        return e(ENODEV);
    };

    let slot = {
        let mut t = BINDS.lock();
        match t.iter().find(|(p, h, _)| *p == pid && *h == handle) {
            Some((_, _, slot)) => *slot,
            None => {
                let Some(free) = (0..MAX_CLIENT_BINDS).find(|s| {
                    !t.iter().any(|(_, _, used)| used == s)
                }) else {
                    return e(EMFILE);
                };
                t.push((pid, handle, free));
                free
            }
        }
    };

    // A pool of video memory is bound whole: the block `gpu_alloc` took, padded
    // to its page, so it maps with that page, 64 KiB or 2 MiB (3.8). The rest
    // of the block is the process's own and was zeroed with it. A pool of
    // system memory takes 4 KiB pages either way, and is bound for its size
    let bytes = match memory {
        shm::Memory::Video => crate::drivers::gpu::nvidia::gsp_common::vram::block_size(phys)
            .filter(|whole| *whole >= size as u64)
            .unwrap_or(size as u64),
        shm::Memory::System => size as u64,
    };
    let Some(va) = crate::drivers::gpu::nvidia::blackwell::boot::client_bind(
        &bar0, pid, slot, phys, bytes, aperture,
    ) else {
        BINDS.lock().retain(|(p, h, _)| !(*p == pid && *h == handle));
        return e(EIO);
    };

    unsafe {
        core::ptr::write_unaligned(out_ptr as *mut [u64; 2], [va, size as u64]);
    }
    0
}

/// How many pools may be bound at once, all processes together
const MAX_CLIENT_BINDS: usize =
    crate::drivers::gpu::nvidia::gsp_common::compute::MAX_CLIENT_BINDS;

/// One parameter as a client writes it: 16 bytes, `{kind, handle, value}`
#[derive(Clone, Copy)]
pub struct Param {
    pub kind: u32,
    pub handle: u32,
    pub value: u64,
}

/// `GPU_OP_DISPATCH`'s argument block, at `args[1]`, and each of a
/// `GPU_OP_CHAIN`'s, one after another
///
///     0  u32 program        4  u32 nparams
///     8  u32 grid[3]       20  u32 block[3]
///    32  u64 params_ptr
const DISPATCH_SIZE: usize = 40;

/// One launch, read and checked: its parameters laid out for the program
struct Launch {
    program: usize,
    bank: alloc::vec::Vec<u8>,
    grid: (u32, u32, u32),
    block: (u32, u32, u32),
}

/// `count` dispatch blocks at `ptr`, as one submission
fn dispatch(cr3: u64, ptr: u64, count: u64) -> u64 {
    let pid = current_pid();
    if count == 0 || count > MAX_CHAIN as u64 {
        return refuse(EINVAL, &alloc::format!("a chain is 1 to {} dispatches", MAX_CHAIN));
    }
    let Some(_ua) = user_ptr_mapped(cr3, ptr, count * DISPATCH_SIZE as u64) else {
        return refuse(EFAULT, "the dispatch block is not readable");
    };
    let loaded = crate::drivers::gpu::nvidia::blackwell::boot::program_names(pid).len();
    let mut launches = alloc::vec::Vec::with_capacity(count as usize);
    for i in 0..count {
        match read_launch(cr3, pid, ptr + i * DISPATCH_SIZE as u64, loaded) {
            Ok(l) => launches.push(l),
            Err(err) => return err,
        }
    }
    match in_flight_ok(pid) {
        Ok(()) => {}
        Err(err) => return err,
    }

    let Some(bar0) = crate::drivers::gpu::nvidia::with_gb20x(|d| d.bar0) else {
        return refuse(ENODEV, "there is no GB206 in this machine");
    };
    use crate::drivers::gpu::nvidia::blackwell::boot::{ClientLaunch, DispatchError as D};
    let chain: alloc::vec::Vec<ClientLaunch<'_>> = launches.iter()
        .map(|l| ClientLaunch { program: l.program, params: &l.bank, grid: l.grid, block: l.block })
        .collect();
    let (epoch, seq) = match crate::drivers::gpu::nvidia::blackwell::boot::client_dispatch(&bar0, pid, &chain) {
        Ok(v) => v,
        // Each of the driver's refusals gets its own errno, distinct and odd,
        // so the number a program prints names one check
        Err(d) => {
            return refuse(
                match d {
                    D::NoPrograms | D::NoChannel => ENODEV,
                    D::ChannelDead | D::NoRing => EIO,
                    D::Prepare => ENOEXEC,
                    D::Submit => EBUSY,
                },
                match d {
                    D::NoPrograms => "no GPU programs are loaded",
                    D::NoChannel => "the compute channel is not up",
                    D::ChannelDead => "the compute channel is marked dead",
                    D::NoRing => "the compute channel has no ring attached",
                    D::Prepare => "the launch descriptor could not be built",
                    D::Submit => "the compute channel would not take the work",
                },
            );
        }
    };
    // Tagged as a compute fence, so `gpu_wait` asks the right engine
    let fence = encode_fence(epoch, seq) | COMPUTE_FENCE;
    IN_FLIGHT.lock().push((pid, fence));
    fence
}

/// One dispatch block at `ptr`, which the caller has checked is mapped, and its
/// parameters, refused with the reason as `Err`
fn read_launch(cr3: u64, pid: u64, ptr: u64, loaded: usize) -> Result<Launch, u64> {
    let mut raw = [0u8; DISPATCH_SIZE];
    unsafe { core::ptr::copy_nonoverlapping(ptr as *const u8, raw.as_mut_ptr(), DISPATCH_SIZE) };
    let u32_at = |o: usize| u32::from_le_bytes([raw[o], raw[o + 1], raw[o + 2], raw[o + 3]]);
    let program = u32_at(0) as usize;
    let nparams = u32_at(4) as usize;
    let grid = (u32_at(8), u32_at(12), u32_at(16));
    let block = (u32_at(20), u32_at(24), u32_at(28));
    let params_ptr = u64::from_le_bytes(raw[32..40].try_into().unwrap_or([0; 8]));

    if nparams > MAX_PARAMS {
        return Err(refuse(EINVAL, "more parameters than a dispatch may carry"));
    }
    // A grid or a block with a zero dimension launches nothing and is far more
    // likely to be a bug than a request
    if grid.0 == 0 || grid.1 == 0 || grid.2 == 0 || block.0 == 0 || block.1 == 0 || block.2 == 0 {
        return Err(refuse(EINVAL, "a grid or block dimension is zero, which launches nothing"));
    }
    if loaded <= program {
        return Err(refuse(EINVAL, &alloc::format!("program {} asked for, {} loaded", program, loaded)));
    }

    // Reads the parameters, then resolves them, so a fault on the last one
    // cannot leave half a constant bank written
    let mut params = [Param { kind: 0, handle: 0, value: 0 }; MAX_PARAMS];
    if nparams != 0 {
        let bytes = nparams * 16;
        let Some(_ua) = user_ptr_mapped(cr3, params_ptr, bytes as u64) else {
            return Err(refuse(EFAULT, "the parameter array is not readable"));
        };
        let mut buf = alloc::vec![0u8; bytes];
        unsafe { core::ptr::copy_nonoverlapping(params_ptr as *const u8, buf.as_mut_ptr(), bytes) };
        for (i, p) in params.iter_mut().enumerate().take(nparams) {
            let o = i * 16;
            *p = Param {
                kind: u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]),
                handle: u32::from_le_bytes([buf[o + 4], buf[o + 5], buf[o + 6], buf[o + 7]]),
                value: u64::from_le_bytes(buf[o + 8..o + 16].try_into().unwrap_or([0; 8])),
            };
        }
    }
    // The address comes from the binding, and the offset is checked against the
    // pool it was bound from. Userspace supplies an offset; the kernel supplies
    // every bit of the address
    let resolve = |handle: u32, off: u64| -> Result<(u64, u64), (i64, alloc::string::String)> {
        let Ok(size) = shm::claim(handle, pid) else {
            return Err((EACCES, alloc::format!("pool {} is not this process's", handle)));
        };
        if off >= size as u64 {
            return Err((EINVAL, alloc::format!("offset {:#x} is past the end of pool {} ({:#x} bytes)", off, handle, size)));
        }
        let Some(va) = bound_va(pid, handle) else {
            return Err((EINVAL, alloc::format!("pool {} is not bound - gpu_bind it before a dispatch names it", handle)));
        };
        Ok((va + off, size as u64 - off))
    };
    match lay_bank(&params[..nparams], resolve) {
        Ok(bank) => Ok(Launch { program, bank, grid, block }),
        Err((code, why)) => Err(refuse(code, &why)),
    }
}

/// Lays a dispatch's parameters out as the compiler expects: each value at its
/// own alignment, in order, the C rule the in-kernel callers follow. `resolve`
/// turns a pool handle and an offset into an address and the bytes from it to
/// the pool's end, or says why not. Apart from the reading of user memory so
/// `nvidia_selftest` can decide it
pub fn lay_bank(
    params: &[Param],
    resolve: impl Fn(u32, u64) -> Result<(u64, u64), (i64, alloc::string::String)>,
) -> Result<alloc::vec::Vec<u8>, (i64, alloc::string::String)> {
    let mut bank: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let align8 = |bank: &mut alloc::vec::Vec<u8>| {
        while bank.len() % 8 != 0 {
            bank.push(0);
        }
    };
    for p in params {
        match p.kind {
            PARAM_U32 => bank.extend_from_slice(&(p.value as u32).to_le_bytes()),
            PARAM_U64 => {
                align8(&mut bank);
                bank.extend_from_slice(&p.value.to_le_bytes());
            }
            PARAM_BUFFER | PARAM_BUFFER_SIZED => {
                let (va, bytes) = resolve(p.handle, p.value)?;
                align8(&mut bank);
                bank.extend_from_slice(&va.to_le_bytes());
                if p.kind == PARAM_BUFFER_SIZED {
                    bank.extend_from_slice(&bytes.to_le_bytes());
                }
            }
            other => return Err((EINVAL, alloc::format!("parameter kind {}", other))),
        }
    }
    Ok(bank)
}

/// Which engine a fence belongs to. The copy engine and the compute engine
/// count independently.
///
/// Bit 62: see `encode_fence` for why it is not 63
pub const COMPUTE_FENCE: u64 = 1 << FENCE_ENGINE_BIT;

/// Where a pool this process bound was mapped
fn bound_va(pid: u64, handle: u32) -> Option<u64> {
    use crate::drivers::gpu::nvidia::gsp_common::compute;
    BINDS
        .lock()
        .iter()
        .find(|(p, h, _)| *p == pid && *h == handle)
        .map(|(_, _, slot)| compute::client_va(*slot))
}

/// `gpu_program(args_ptr) -> program id`
///
/// The only call in this range behind a capability. Everything else runs
/// programs the kernel compiled into itself, over addresses the kernel chose.
/// This one takes machine code.
///
/// Until every process has its own GPU address space, a loaded program runs in
/// the shared one and can reach anything mapped there: the console's shadow
/// buffer, another client's pool, the compositor's surfaces. `CAP_GPU` is the
/// protection.
///
/// The arguments are a block, as for `gpu_submit`:
///
/// ```text
///    0  u64 code_ptr     the cubin, as `nvcc -cubin` wrote it
///    8  u64 code_len
///   16  u32 regs         registers per thread, from `cuobjdump -res-usage`
///   20  u32 shared       bytes of shared memory, likewise
/// ```
///
/// `regs` and `shared` are the caller's declaration because nothing in the
/// kernel can derive them. They are checked here, so a wrong declaration costs
/// the caller its own program and not the machine
pub fn sys_gpu_program(args_ptr: u64) -> u64 {
    use crate::drivers::gpu::nvidia::cubin;

    if !crate::sched::current_cred().has(crate::process::cred::CAP_GPU) {
        return refuse(EPERM, "loading a GPU program needs CAP_GPU");
    }
    let cr3 = current_cr3();
    let pid = current_pid();

    let mut raw = [0u8; 24];
    let Some(_ua) = user_ptr_mapped(cr3, args_ptr, raw.len() as u64) else {
        return refuse(EFAULT, "the argument block is not readable");
    };
    unsafe { core::ptr::copy_nonoverlapping(args_ptr as *const u8, raw.as_mut_ptr(), raw.len()) };
    let code_ptr = u64::from_le_bytes(raw[0..8].try_into().unwrap_or([0; 8]));
    let code_len = u64::from_le_bytes(raw[8..16].try_into().unwrap_or([0; 8]));
    let regs = u32::from_le_bytes(raw[16..20].try_into().unwrap_or([0; 4]));
    let shared = u32::from_le_bytes(raw[20..24].try_into().unwrap_or([0; 4]));

    if code_len == 0 || code_len > MAX_CUBIN as u64 {
        return refuse(EINVAL, &alloc::format!(
            "a cubin is between 1 and {} bytes", MAX_CUBIN));
    }
    // Registers per thread is an 8-bit field of the launch descriptor and
    // shared memory is bounded by the hardware; a declaration outside either is
    // refused rather than truncated
    if regs == 0 || regs > MAX_REGS {
        return refuse(EINVAL, &alloc::format!("registers per thread must be 1..{}", MAX_REGS));
    }
    if shared > MAX_SHARED {
        return refuse(EINVAL, &alloc::format!("shared memory must be at most {}", MAX_SHARED));
    }

    let Some(_ub) = user_ptr_mapped(cr3, code_ptr, code_len) else {
        return refuse(EFAULT, "the cubin is not readable");
    };
    let mut code = alloc::vec![0u8; code_len as usize];
    unsafe {
        core::ptr::copy_nonoverlapping(code_ptr as *const u8, code.as_mut_ptr(), code.len())
    };

    // Read with the parser the build uses on `nvcc`'s output, so a cubin that
    // loads here would have built in
    let kernels = match cubin::kernels(&code) {
        Ok(k) => k,
        Err(e) => return refuse(ENOEXEC, e.why()),
    };
    if kernels.len() != 1 {
        return refuse(EINVAL, "a cubin must hold exactly one kernel");
    }
    let k = &kernels[0];

    let Some(bar0) = crate::drivers::gpu::nvidia::with_gb20x(|d| d.bar0) else {
        return refuse(ENODEV, "there is no GB206 in this machine");
    };

    // Loading machine code is where a process stops sharing. Until this call it
    // runs the kernel's programs over addresses the kernel resolved, on the
    // kernel's channel; from here it gets an address space of its own, a
    // channel in it and a graphics context of its own, so its instructions
    // reach only memory it asked for.
    //
    // Built on the first load and kept until the process exits. A graphics
    // context is megabytes, so nothing else in this range triggers it
    let say = |m: &str| {
        crate::serial_println!("[gpu] {}", m);
    };
    // A private space is off by default and its absence is not an error: the
    // program then loads into the shared one, which is what `CAP_GPU` guards
    if let Err(why) = crate::drivers::gpu::nvidia::blackwell::boot::client_own_gpu(pid, &say) {
        crate::serial_println!("[gpu] shared address space: {}", why);
    }

    match crate::drivers::gpu::nvidia::blackwell::boot::client_load_program(
        &bar0, pid, &k.name, &k.code, k.const0, regs, shared,
    ) {
        Ok(id) => id as u64,
        Err(why) => refuse(ENOEXEC, why),
    }
}

/// A pool is being given back: its binding goes with it, slot and mapping,
/// before the memory does (`dev/gpu-memory-compute.md`, 3.7)
pub fn forget_pool(pid: u64, handle: u32) {
    let slot = {
        let mut t = BINDS.lock();
        let slot = t.iter().find(|(p, h, _)| *p == pid && *h == handle).map(|(_, _, s)| *s);
        t.retain(|(p, h, _)| !(*p == pid && *h == handle));
        slot
    };
    if let (Some(slot), Some(bar0)) = (slot, crate::drivers::gpu::nvidia::with_gb20x(|d| d.bar0)) {
        crate::drivers::gpu::nvidia::blackwell::boot::client_unbind(&bar0, pid, slot);
    }
}

/// `gpu_alloc(size, flags, out_ptr)`: a pool of video memory, mapped here.
///
/// `out_ptr` receives `{handle, addr}` as `may_pool`'s does, and the handle is
/// a pool handle: `gpu_bind` binds it, `gpu_submit` copies to and from it,
/// `may_unpool` gives it back. The memory is the top of what BAR1 reaches,
/// zeroed, mapped write-combining: stores are fast and loads cross the bus one
/// at a time, so a program fills it and lets the GPU read it.
///
/// No capability: the kernel picks the memory, and a process gets its own,
/// cleared. The limits are the pool table's and a ceiling for all programs
/// together (`nvidia::user_vram`)
pub fn sys_gpu_alloc(size: u64, flags: u64, out_ptr: u64) -> u64 {
    use crate::drivers::gpu::nvidia::user_vram::{self, Refused};
    if flags != 0 {
        return refuse(EINVAL, "gpu_alloc takes no flags yet");
    }
    if size == 0 || size > shm::MAX_POOL_BYTES as u64 {
        return refuse(EINVAL, "a video-memory pool is between one byte and 64 MiB");
    }
    let cr3 = current_cr3();
    let Some(_ua) = user_ptr_writable(cr3, out_ptr, 16) else {
        return e(EFAULT);
    };
    let pid = current_pid();
    let (vram, cpu_phys, len) = match user_vram::alloc(size) {
        Ok(b) => b,
        Err(why) => return refuse(match why {
            Refused::NoCard | Refused::NoWindow => ENODEV,
            Refused::Quota => EDQUOT,
            Refused::NoMemory => ENOMEM,
        }, why.as_str()),
    };
    let handle = match shm::adopt_video(pid, vram, cpu_phys, len as usize, size as usize) {
        Ok(h) => h,
        Err(_) => {
            user_vram::retire(vram, len);
            return refuse(EMFILE, "this process holds as many pools as it may");
        }
    };
    let addr = match shm::map_into(handle, pid, cr3) {
        Ok(a) => a,
        Err(_) => {
            let _ = shm::destroy(handle);
            return refuse(ENOMEM, "no room in this address space for the window");
        }
    };
    unsafe {
        core::ptr::write_unaligned(out_ptr as *mut [u64; 2], [handle as u64, addr]);
    }
    0
}

/// Drop every binding this process had, and take the mappings out of the GPU's
/// address space. Called when it exits
fn unbind_all(pid: u64) {
    let slots: alloc::vec::Vec<usize> = {
        let mut t = BINDS.lock();
        let mine = t.iter().filter(|(p, _, _)| *p == pid).map(|(_, _, s)| *s).collect();
        t.retain(|(p, _, _)| *p != pid);
        mine
    };
    if slots.is_empty() {
        return;
    }
    if let Some(bar0) = crate::drivers::gpu::nvidia::with_gb20x(|d| d.bar0) {
        for slot in slots {
            crate::drivers::gpu::nvidia::blackwell::boot::client_unbind(&bar0, pid, slot);
        }
    }
}
