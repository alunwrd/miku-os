//! CPU-raised signals through `libc`, as `wasmtime` uses them
//!
//! wasmtime maps out-of-bounds access to `SIGSEGV`, `unreachable` to `SIGILL`,
//! and division by zero to `SIGFPE`. Its handler reads RIP from
//! `uc_mcontext.gregs[REG_RIP]`, writes a new RIP and RSP, and returns. Each
//! case does this and checks all fourteen named general registers and a
//! clobbered `xmm`.
//!
//! Also: `si_code` and `si_addr`, `sigaltstack` for an unusable RSP (stack
//! overflow), a fault in a handler, four threads faulting at once, a sent
//! signal's `ucontext_t`, and two children that must die of `SIGSEGV`.

use std::arch::asm;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

// What the handler saw, and where it should send the thread next
static RESUME: AtomicU64 = AtomicU64::new(0);
static FAULT_AT: AtomicU64 = AtomicU64::new(0);
static TARGET: AtomicU64 = AtomicU64::new(0);
static SAVED_RSP: AtomicU64 = AtomicU64::new(0);

static SEEN_SIG: AtomicI32 = AtomicI32::new(0);
static SEEN_CODE: AtomicI32 = AtomicI32::new(0);
static SEEN_ADDR: AtomicU64 = AtomicU64::new(0);
static SEEN_RIP: AtomicU64 = AtomicU64::new(0);
static SEEN_R12: AtomicU64 = AtomicU64::new(0);
static SEEN_FPREGS: AtomicU64 = AtomicU64::new(0);
static SEEN_STACK: AtomicU64 = AtomicU64::new(0);
static SEEN_ALT_FLAGS: AtomicI32 = AtomicI32::new(-1);
static SEEN_ALT_EPERM: AtomicBool = AtomicBool::new(false);

/// Run a `ud2` from inside the next `SIGSEGV` handler
static NEST: AtomicBool = AtomicBool::new(false);
static NESTED_SIG: AtomicI32 = AtomicI32::new(0);
/// Skips the faulting three-byte `mov rax, [rax]` instead of jumping to
/// `RESUME`, for threads
static SKIP: AtomicBool = AtomicBool::new(false);
/// Also restores the RSP `probe_bad_stack` saved, as a stack overflow needs
static SHIM: AtomicBool = AtomicBool::new(false);
static FAULTS: AtomicU64 = AtomicU64::new(0);

static USR1_CODE: AtomicI32 = AtomicI32::new(-1);
static USR1_CTX: AtomicBool = AtomicBool::new(false);

const MARK: [u64; 12] = [
    0x0101_0101_0101_0101,
    0x0202_0202_0202_0202,
    0x0303_0303_0303_0303,
    0x0404_0404_0404_0404,
    0x0808_0808_0808_0808,
    0x0909_0909_0909_0909,
    0x1010_1010_1010_1010,
    0x1111_1111_1111_1111,
    0x1212_1212_1212_1212,
    0x1313_1313_1313_1313,
    0x1414_1414_1414_1414,
    0x1515_1515_1515_1515,
];
const XMM_MARK: u64 = 0x5a5a_5a5a_a5a5_a5a5;

extern "C" fn on_fault(sig: i32, info: *mut libc::siginfo_t, ctx: *mut c_void) {
    let here = 0u8;
    let resume = RESUME.load(Ordering::SeqCst);
    let resume_rsp = if SHIM.load(Ordering::SeqCst) { SAVED_RSP.load(Ordering::SeqCst) } else { 0 };
    let uc = unsafe { &mut *(ctx as *mut libc::ucontext_t) };
    FAULTS.fetch_add(1, Ordering::SeqCst);

    if SKIP.load(Ordering::SeqCst) && sig == libc::SIGSEGV {
        uc.uc_mcontext.gregs[libc::REG_RIP as usize] += 3;
        return;
    }

    SEEN_SIG.store(sig, Ordering::SeqCst);
    SEEN_CODE.store(unsafe { (*info).si_code }, Ordering::SeqCst);
    SEEN_ADDR.store(unsafe { (*info).si_addr() } as u64, Ordering::SeqCst);
    SEEN_RIP.store(uc.uc_mcontext.gregs[libc::REG_RIP as usize] as u64, Ordering::SeqCst);
    SEEN_R12.store(uc.uc_mcontext.gregs[libc::REG_R12 as usize] as u64, Ordering::SeqCst);
    SEEN_FPREGS.store(uc.uc_mcontext.fpregs as u64, Ordering::SeqCst);
    SEEN_STACK.store(std::hint::black_box(&here) as *const u8 as u64, Ordering::SeqCst);

    // `sigaltstack` state in the handler, and that a change is refused
    let mut now: libc::stack_t = unsafe { std::mem::zeroed() };
    if unsafe { libc::sigaltstack(std::ptr::null(), &mut now) } == 0 {
        SEEN_ALT_FLAGS.store(now.ss_flags, Ordering::SeqCst);
        if now.ss_flags & libc::SS_ONSTACK != 0 {
            let r = unsafe { libc::sigaltstack(&now, std::ptr::null_mut()) };
            SEEN_ALT_EPERM.store(
                r == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM),
                Ordering::SeqCst,
            );
        }
    }

    // Clobbers the FPU image, which the stub must restore
    unsafe { asm!("pxor xmm8, xmm8", out("xmm8") _) };

    if sig == libc::SIGSEGV && NEST.swap(false, Ordering::SeqCst) {
        probe_ud2();
        NESTED_SIG.store(SEEN_SIG.load(Ordering::SeqCst), Ordering::SeqCst);
    }

    uc.uc_mcontext.gregs[libc::REG_RIP as usize] = resume as i64;
    if resume_rsp != 0 {
        uc.uc_mcontext.gregs[libc::REG_RSP as usize] = resume_rsp as i64;
    }
}

extern "C" fn on_usr1(_sig: i32, info: *mut libc::siginfo_t, ctx: *mut c_void) {
    USR1_CODE.store(unsafe { (*info).si_code }, Ordering::SeqCst);
    USR1_CTX.store(!ctx.is_null(), Ordering::SeqCst);
}

fn install(sig: i32, handler: usize, flags: i32) -> bool {
    let mut sa: libc::sigaction = unsafe { std::mem::zeroed() };
    sa.sa_sigaction = handler;
    sa.sa_flags = flags;
    unsafe { libc::sigaction(sig, &sa, std::ptr::null_mut()) == 0 }
}

/// A load from `TARGET` with a mark in every named register. Returns them after
/// the resume, then `xmm8`
fn probe_load() -> [u64; 13] {
    let mut v = [0u64; 13];
    unsafe {
        asm!(
            "lea rax, [rip + 3f]",
            "mov qword ptr [rip + {resume}], rax",
            "lea rax, [rip + 2f]",
            "mov qword ptr [rip + {fault_at}], rax",
            "mov rax, {xmark}",
            "movq xmm8, rax",
            "mov rcx, {m0}",
            "mov rdx, {m1}",
            "mov rsi, {m2}",
            "mov rdi, {m3}",
            "mov r8, {m4}",
            "mov r9, {m5}",
            "mov r10, {m6}",
            "mov r11, {m7}",
            "mov r12, {m8}",
            "mov r13, {m9}",
            "mov r14, {m10}",
            "mov r15, {m11}",
            "mov rax, qword ptr [rip + {target}]",
            "2:",
            "mov rax, qword ptr [rax]",
            "3:",
            "movq rax, xmm8",
            resume = sym RESUME,
            fault_at = sym FAULT_AT,
            target = sym TARGET,
            xmark = const XMM_MARK,
            m0 = const MARK[0], m1 = const MARK[1], m2 = const MARK[2], m3 = const MARK[3],
            m4 = const MARK[4], m5 = const MARK[5], m6 = const MARK[6], m7 = const MARK[7],
            m8 = const MARK[8], m9 = const MARK[9], m10 = const MARK[10], m11 = const MARK[11],
            out("rax") v[12],
            out("rcx") v[0], out("rdx") v[1], out("rsi") v[2], out("rdi") v[3],
            out("r8") v[4], out("r9") v[5], out("r10") v[6], out("r11") v[7],
            out("r12") v[8], out("r13") v[9], out("r14") v[10], out("r15") v[11],
            out("xmm8") _,
        );
    }
    v
}

/// A store to `TARGET`
fn probe_store() {
    unsafe {
        asm!(
            "lea rax, [rip + 3f]",
            "mov qword ptr [rip + {resume}], rax",
            "lea rax, [rip + 2f]",
            "mov qword ptr [rip + {fault_at}], rax",
            "mov rax, qword ptr [rip + {target}]",
            "2:",
            "mov qword ptr [rax], rax",
            "3:",
            resume = sym RESUME,
            fault_at = sym FAULT_AT,
            target = sym TARGET,
            out("rax") _,
        );
    }
}

fn probe_ud2() {
    unsafe {
        asm!(
            "lea rax, [rip + 3f]",
            "mov qword ptr [rip + {resume}], rax",
            "lea rax, [rip + 2f]",
            "mov qword ptr [rip + {fault_at}], rax",
            "2:",
            "ud2",
            "3:",
            resume = sym RESUME,
            fault_at = sym FAULT_AT,
            out("rax") _,
        );
    }
}

fn probe_div0() {
    unsafe {
        asm!(
            "lea rax, [rip + 3f]",
            "mov qword ptr [rip + {resume}], rax",
            "lea rax, [rip + 2f]",
            "mov qword ptr [rip + {fault_at}], rax",
            "mov eax, 1",
            "xor edx, edx",
            "xor ecx, ecx",
            "2:",
            "div ecx",
            "3:",
            resume = sym RESUME,
            fault_at = sym FAULT_AT,
            out("rax") _, out("rcx") _, out("rdx") _,
        );
    }
}

/// A push with an invalid RSP: the handler must run on the alternate stack and
/// return a usable RSP with RIP
fn probe_bad_stack() {
    unsafe {
        asm!(
            "lea rax, [rip + 3f]",
            "mov qword ptr [rip + {resume}], rax",
            "mov qword ptr [rip + {saved}], rsp",
            "lea rax, [rip + 2f]",
            "mov qword ptr [rip + {fault_at}], rax",
            "mov rsp, 0x1000",
            "2:",
            "push rax",
            "3:",
            resume = sym RESUME,
            saved = sym SAVED_RSP,
            fault_at = sym FAULT_AT,
            out("rax") _,
        );
    }
}

/// `n` faulting loads, each skipped. Returns the loop count, `n` only if
/// R12-R14 survive
fn probe_many(n: u64) -> u64 {
    let count: u64;
    unsafe {
        asm!(
            "2:",
            "mov rax, r12",
            "mov rax, qword ptr [rax]",
            "inc r13",
            "cmp r13, r14",
            "jne 2b",
            in("r12") 8u64,
            inout("r13") 0u64 => count,
            in("r14") n,
            out("rax") _,
        );
    }
    count
}

fn reset_seen() {
    SEEN_SIG.store(0, Ordering::SeqCst);
    SEEN_CODE.store(0, Ordering::SeqCst);
    SEEN_ADDR.store(0, Ordering::SeqCst);
    SEEN_RIP.store(0, Ordering::SeqCst);
    SEEN_STACK.store(0, Ordering::SeqCst);
    SEEN_ALT_FLAGS.store(-1, Ordering::SeqCst);
    SEEN_ALT_EPERM.store(false, Ordering::SeqCst);
}

fn check(name: &str, ok: bool, detail: String, failed: &mut i32) {
    if ok {
        println!("  ok    {name}");
    } else {
        println!("  FAIL  {name}: {detail}");
        *failed += 1;
    }
}

/// Fork, run `body` in the child, and answer with the signal it died of
fn child_dies_of(body: fn()) -> Option<i32> {
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return None;
    }
    if pid == 0 {
        body();
        unsafe { libc::_exit(0) };
    }
    let mut st = 0;
    if unsafe { libc::waitpid(pid, &mut st, 0) } != pid {
        return None;
    }
    if libc::WIFSIGNALED(st) { Some(libc::WTERMSIG(st)) } else { Some(-libc::WEXITSTATUS(st) - 1000) }
}

fn main() {
    println!("stdsignal: faults as signals, through libc");
    let mut failed = 0;
    let fault_flags = libc::SA_SIGINFO | libc::SA_NODEFER | libc::SA_ONSTACK;

    // Before any handler: a fault crashes
    {
        let got = child_dies_of(|| {
            TARGET.store(8, Ordering::SeqCst);
            probe_load();
        });
        check("no handler: the child dies of SIGSEGV", got == Some(libc::SIGSEGV),
            format!("{got:?}"), &mut failed);
    }

    let ok = install(libc::SIGSEGV, on_fault as usize, fault_flags)
        && install(libc::SIGILL, on_fault as usize, fault_flags)
        && install(libc::SIGFPE, on_fault as usize, fault_flags);
    check("sigaction with SA_SIGINFO|SA_NODEFER|SA_ONSTACK", ok, String::new(), &mut failed);

    // 1. Load from an unmapped page, resumed at a label, with registers
    //    preserved
    {
        reset_seen();
        TARGET.store(8, Ordering::SeqCst);
        let v = probe_load();
        let fault_at = FAULT_AT.load(Ordering::SeqCst);
        check("SIGSEGV from a load: signal and si_code",
            SEEN_SIG.load(Ordering::SeqCst) == libc::SIGSEGV
                && SEEN_CODE.load(Ordering::SeqCst) == libc::SEGV_MAPERR,
            format!("sig {} code {}", SEEN_SIG.load(Ordering::SeqCst), SEEN_CODE.load(Ordering::SeqCst)),
            &mut failed);
        check("si_addr is the address loaded from", SEEN_ADDR.load(Ordering::SeqCst) == 8,
            format!("{:#x}", SEEN_ADDR.load(Ordering::SeqCst)), &mut failed);
        check("gregs[REG_RIP] is the faulting instruction",
            SEEN_RIP.load(Ordering::SeqCst) == fault_at,
            format!("{:#x} vs {:#x}", SEEN_RIP.load(Ordering::SeqCst), fault_at), &mut failed);
        check("gregs[REG_R12] is what R12 held", SEEN_R12.load(Ordering::SeqCst) == MARK[8],
            format!("{:#x}", SEEN_R12.load(Ordering::SeqCst)), &mut failed);
        check("uc_mcontext.fpregs points into the frame", SEEN_FPREGS.load(Ordering::SeqCst) != 0,
            String::new(), &mut failed);
        let regs_ok = v[..12] == MARK;
        check("resumed where the handler said, RCX..R15 intact (iretq, not sysret)", regs_ok,
            format!("{:x?}", &v[..12]), &mut failed);
        check("xmm8 intact though the handler zeroed it", v[12] == XMM_MARK,
            format!("{:#x}", v[12]), &mut failed);
    }

    // 2. Store to a read-only page is an access error at that address
    {
        reset_seen();
        let page = unsafe {
            libc::mmap(std::ptr::null_mut(), 4096, libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0)
        };
        let ok = page != libc::MAP_FAILED
            && unsafe { *(page as *mut u64) = 1; libc::mprotect(page, 4096, libc::PROT_READ) } == 0;
        TARGET.store(page as u64 + 16, Ordering::SeqCst);
        if ok {
            probe_store();
        }
        check("SIGSEGV from a store to a read-only page is SEGV_ACCERR",
            ok && SEEN_SIG.load(Ordering::SeqCst) == libc::SIGSEGV
                && SEEN_CODE.load(Ordering::SeqCst) == libc::SEGV_ACCERR
                && SEEN_ADDR.load(Ordering::SeqCst) == page as u64 + 16,
            format!("mapped {ok} sig {} code {} addr {:#x}", SEEN_SIG.load(Ordering::SeqCst),
                SEEN_CODE.load(Ordering::SeqCst), SEEN_ADDR.load(Ordering::SeqCst)),
            &mut failed);
    }

    // 3. `ud2`: wasmtime's `unreachable`
    {
        reset_seen();
        probe_ud2();
        let at = FAULT_AT.load(Ordering::SeqCst);
        check("SIGILL from ud2, ILL_ILLOPN, at the instruction",
            SEEN_SIG.load(Ordering::SeqCst) == libc::SIGILL
                && SEEN_CODE.load(Ordering::SeqCst) == libc::ILL_ILLOPN
                && SEEN_RIP.load(Ordering::SeqCst) == at
                && SEEN_ADDR.load(Ordering::SeqCst) == at,
            format!("sig {} code {} rip {:#x} vs {at:#x}", SEEN_SIG.load(Ordering::SeqCst),
                SEEN_CODE.load(Ordering::SeqCst), SEEN_RIP.load(Ordering::SeqCst)),
            &mut failed);
    }

    // 4. Integer division by zero: #DE
    {
        reset_seen();
        probe_div0();
        check("SIGFPE from div by zero, FPE_INTDIV",
            SEEN_SIG.load(Ordering::SeqCst) == libc::SIGFPE
                && SEEN_CODE.load(Ordering::SeqCst) == libc::FPE_INTDIV
                && SEEN_RIP.load(Ordering::SeqCst) == FAULT_AT.load(Ordering::SeqCst),
            format!("sig {} code {}", SEEN_SIG.load(Ordering::SeqCst), SEEN_CODE.load(Ordering::SeqCst)),
            &mut failed);
    }

    // 5. A fault in a handler is delivered (SA_NODEFER)
    {
        reset_seen();
        NEST.store(true, Ordering::SeqCst);
        TARGET.store(8, Ordering::SeqCst);
        let _ = probe_load();
        check("a SIGILL raised inside a SIGSEGV handler runs, and both return",
            NESTED_SIG.load(Ordering::SeqCst) == libc::SIGILL,
            format!("nested {}", NESTED_SIG.load(Ordering::SeqCst)), &mut failed);
    }

    // 6. With an invalid RSP, the handler runs on the alternate stack and
    //    restores RSP
    {
        let size = 64 * 1024;
        let mem = unsafe {
            libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0)
        };
        let ss = libc::stack_t { ss_sp: mem, ss_flags: 0, ss_size: size };
        let set = mem != libc::MAP_FAILED
            && unsafe { libc::sigaltstack(&ss, std::ptr::null_mut()) } == 0;
        let mut old: libc::stack_t = unsafe { std::mem::zeroed() };
        let read = unsafe { libc::sigaltstack(std::ptr::null(), &mut old) } == 0;
        check("sigaltstack set and read back",
            set && read && old.ss_sp == mem && old.ss_size == size && old.ss_flags == 0,
            format!("set {set} read {read} sp {:?} size {} flags {}", old.ss_sp, old.ss_size, old.ss_flags),
            &mut failed);

        let tiny = libc::stack_t { ss_sp: mem, ss_flags: 0, ss_size: 1024 };
        let r = unsafe { libc::sigaltstack(&tiny, std::ptr::null_mut()) };
        check("an alternate stack below MINSIGSTKSZ is ENOMEM",
            r == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOMEM),
            format!("{r}"), &mut failed);

        reset_seen();
        SHIM.store(true, Ordering::SeqCst);
        probe_bad_stack();
        SHIM.store(false, Ordering::SeqCst);
        let st = SEEN_STACK.load(Ordering::SeqCst);
        let lo = mem as u64;
        check("a push with RSP=0x1000 is caught, on the alternate stack",
            SEEN_SIG.load(Ordering::SeqCst) == libc::SIGSEGV && st > lo && st <= lo + size as u64,
            format!("sig {} handler stack {st:#x}, alt {lo:#x}+{size:#x}", SEEN_SIG.load(Ordering::SeqCst)),
            &mut failed);
        check("sigaltstack from inside says SS_ONSTACK, and refuses a change with EPERM",
            SEEN_ALT_FLAGS.load(Ordering::SeqCst) == libc::SS_ONSTACK && SEEN_ALT_EPERM.load(Ordering::SeqCst),
            format!("flags {} eperm {}", SEEN_ALT_FLAGS.load(Ordering::SeqCst), SEEN_ALT_EPERM.load(Ordering::SeqCst)),
            &mut failed);

        let off = libc::stack_t { ss_sp: std::ptr::null_mut(), ss_flags: libc::SS_DISABLE, ss_size: 0 };
        unsafe { libc::sigaltstack(&off, std::ptr::null_mut()) };
    }

    // 7. Without an alternate stack, the child dies of the fault
    {
        let got = child_dies_of(|| {
            SHIM.store(true, Ordering::SeqCst);
            probe_bad_stack();
        });
        check("no alternate stack and no usable RSP: the child dies of SIGSEGV",
            got == Some(libc::SIGSEGV), format!("{got:?}"), &mut failed);
    }

    // 8. Four threads faulting at once, each on its own frames
    {
        SKIP.store(true, Ordering::SeqCst);
        FAULTS.store(0, Ordering::SeqCst);
        let n = 500u64;
        let hs: Vec<_> = (0..4).map(|_| std::thread::spawn(move || probe_many(n))).collect();
        let counts: Vec<u64> = hs.into_iter().map(|h| h.join().unwrap_or(0)).collect();
        SKIP.store(false, Ordering::SeqCst);
        check("four threads x 500 faults, every one skipped and counted",
            counts.iter().all(|&c| c == n) && FAULTS.load(Ordering::SeqCst) == 4 * n,
            format!("{counts:?}, {} faults", FAULTS.load(Ordering::SeqCst)), &mut failed);
    }

    // 9. A sent signal gets the same frame, with si_code SI_USER; the
    //    interrupted syscall still returns 0
    {
        let ok = install(libc::SIGUSR1, on_usr1 as usize, libc::SA_SIGINFO);
        let r = unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) };
        check("kill(self, SIGUSR1): handler ran with a ucontext and SI_USER, kill returned 0",
            ok && r == 0 && USR1_CTX.load(Ordering::SeqCst)
                && USR1_CODE.load(Ordering::SeqCst) == libc::SI_USER,
            format!("installed {ok} r {r} ctx {} code {}", USR1_CTX.load(Ordering::SeqCst),
                USR1_CODE.load(Ordering::SeqCst)),
            &mut failed);
    }

    println!("stdsignal: {}", if failed == 0 { "all passed" } else { "FAILURES" });
    std::process::exit(if failed == 0 { 0 } else { 1 });
}

