// What gets printed when something dies.
//
// Which process, which signal, which access, which function, and how it got
// there. The pieces come from elsewhere (`process::image` keeps the bias and
// the path, `process::symbols` turns an unbiased address into a name,
// `kcore::backtrace` walks the frame chain); this module puts them in one order
// and prints them. A raw rip is a link-time offset plus an ASLR bias, and two
// identical crashes would print two different numbers.
//
// Everything here runs inside a fault handler, so nothing may panic, block for
// ever, or allocate more than it must. Symbol tables are read once, used and
// dropped.

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

use crate::kcore::backtrace::{self, Frame, MAX_FRAMES};
use crate::process::symbols::SymbolTable;

/// Why we are here. Carries the detail worth printing for each kind
pub enum Cause {
    PageFault { addr: u64, write: bool, present: bool, exec: bool },
    GeneralProtection { error_code: u64 },
    InvalidOpcode,
    /// `div` or `idiv` by zero, or a quotient too big for its register
    DivideError,
    /// A fatal signal with nowhere to go: no handler installed, or the frame
    /// could not be written
    Signal { sig: u32 },
}

impl Cause {
    fn name(&self) -> &'static str {
        match self {
            Cause::PageFault { .. } => "SIGSEGV",
            Cause::GeneralProtection { .. } => "SIGSEGV (#GP)",
            Cause::InvalidOpcode => "SIGILL",
            Cause::DivideError => "SIGFPE",
            Cause::Signal { sig } => match sig {
                2  => "SIGINT",
                4  => "SIGILL",
                6  => "SIGABRT",
                8  => "SIGFPE",
                11 => "SIGSEGV",
                _  => "signal",
            },
        }
    }
}

/// The register state a report can get at. `x86-interrupt` handlers see only
/// the interrupt frame, so the general registers are what the caller recovered,
/// which for RBP is the frame-pointer chain
pub struct Regs {
    pub rip:    u64,
    pub rsp:    u64,
    pub rbp:    u64,
    pub rflags: u64,
    pub cs:     u64,
}

/// The RBP of whatever this handler interrupted, when it can be had safely.
///
/// An interrupt handler's own frame starts with the interrupted context's RBP
/// at [rbp], so one dereference reaches it. But `force-frame-pointers=yes` does
/// not give `page_fault_handler` a `mov rsp, rbp` prologue, and the compiler
/// may keep the fault's error code in RBP. Reading through that would fault in
/// the page fault handler. #PF is on its own IST stack, so every nested entry
/// resets RSP to the same top: the guard page is never touched, no double fault
/// is raised, and the processor takes page faults for ever. On the processor
/// that runs `advance_tick` that stops the clock for the whole machine.
///
/// So RBP is read but not trusted: a value that is not a mapped kernel address
/// is not a frame pointer
#[inline(always)]
pub fn interrupted_rbp() -> u64 {
    let rbp: u64;
    unsafe { core::arch::asm!("mov {}, rbp", out(reg) rbp, options(nomem, nostack)) };
    if rbp == 0 || rbp & 7 != 0 {
        return 0;
    }
    // A frame pointer belonging to a handler frame is a kernel stack address. A
    // user address here is the interrupted ring-3 context's RBP or rubbish, and
    // reading it from ring 0 with AC clear is a SMAP violation: a fault in the
    // fault handler again
    if rbp < 0xFFFF_8000_0000_0000 {
        return 0;
    }
    // Eight bytes, eight-aligned, so the read cannot straddle a page
    let cr3 = x86_64::registers::control::Cr3::read()
        .0
        .start_address()
        .as_u64();
    let mapped = crate::mm::vmm::read_pte_raw(cr3, rbp & !0xFFF)
        .is_some_and(|p| p & crate::mm::vmm::PTE_PRESENT != 0);
    if !mapped {
        return 0;
    }
    unsafe { (rbp as *const u64).read_volatile() }
}

/// Print a full report for a user process that is about to be killed.
pub fn report_user(pid: u64, cause: Cause, regs: &Regs) {
    let cr3 = crate::syscall::usercopy::current_cr3();
    let name = crate::sched::name_of(pid).unwrap_or(crate::process::Name::new("?"));

    crate::serial_println!("[crash] pid={} '{}' {} at {:#018x}", pid, name, cause.name(), regs.rip);
    print_cause(&cause);

    // The image record makes an address meaningful. Without it the raw numbers
    // are printed, with the reason. Recorded under the process, and the task
    // that faulted may be any of its threads
    let tgid = crate::sched::tgid_of(pid);
    let image = crate::process::image::path_and_bias(tgid);
    let (path, bias) = match image {
        Some(v) => v,
        None => {
            crate::serial_println!("[crash]   no image record for this pid - addresses are raw");
            print_frames(&[Frame { pc: regs.rip, guessed: false }], None, 0);
            print_regs(regs);
            return;
        }
    };

    crate::process::image::with(tgid, |i| {
        match i.build_id() {
            Some(id) => crate::serial_println!(
                "[crash]   image {} bias={:#x} build-id {}", i.path, i.load_bias, HexShort(id),
            ),
            None => crate::serial_println!(
                "[crash]   image {} bias={:#x}", i.path, i.load_bias,
            ),
        }
        if i.interp_base != 0 {
            crate::serial_println!("[crash]   interp base={:#x}", i.interp_base);
        }
    });

    // Frame chain first, symbols second: the walk needs the user stack, and
    // reading the binary takes the VFS lock
    let (stack_lo, stack_hi) = user_stack_bounds(cr3, regs.rsp);
    let mut frames = alloc::vec![Frame { pc: regs.rip, guessed: false }];
    frames.extend(backtrace::user_frames(cr3, regs.rbp, regs.rsp, stack_lo, stack_hi));
    frames.truncate(MAX_FRAMES);

    let table = load_symbols(&path);
    print_frames(&frames, table.as_ref(), bias);
    print_regs(regs);
    // Around the faulting address when there is one: the maps that explain a
    // page fault are its neighbours, and the first of a big program's maps
    // are always its libraries' code
    let near = match &cause {
        Cause::PageFault { addr, .. } => *addr,
        _ => regs.rip,
    };
    print_maps(cr3, near);
}

/// The same, for a fault the kernel took itself. There is no image record
/// and no user stack; symbols come from the table built at boot
pub fn report_kernel(cause: Cause, regs: &Regs) {
    crate::serial_println!("[crash] kernel {} at {:#018x}", cause.name(), regs.rip);
    print_cause(&cause);

    let mut frames = alloc::vec![Frame { pc: regs.rip, guessed: false }];
    frames.extend(backtrace::kernel_frames_from(regs.rbp));
    frames.truncate(MAX_FRAMES);

    for (i, f) in frames.iter().enumerate() {
        match crate::process::symbols::resolve_kernel(f.pc) {
            Some((name, off)) => crate::serial_println!(
                "[crash]   #{:<2} {:#018x}  {}+{:#x}{}",
                i, f.pc, name, off, if f.guessed { "  ?" } else { "" },
            ),
            None => crate::serial_println!(
                "[crash]   #{:<2} {:#018x}{}",
                i, f.pc, if f.guessed { "  ?" } else { "" },
            ),
        }
    }
    print_regs(regs);
}

fn print_cause(cause: &Cause) {
    match cause {
        Cause::PageFault { addr, write, present, exec } => {
            let what = if *exec { "execute" } else if *write { "write to" } else { "read from" };
            let why = if *present { "protection violation" } else { "page not present" };
            crate::serial_println!("[crash]   {} {:#x} ({})", what, addr, why);
        }
        Cause::GeneralProtection { error_code } => {
            if *error_code != 0 {
                crate::serial_println!(
                    "[crash]   segment selector {:#x}", error_code,
                );
            }
        }
        Cause::InvalidOpcode => {
            crate::serial_println!("[crash]   not an instruction");
        }
        Cause::DivideError => {
            crate::serial_println!("[crash]   integer division by zero, or overflow");
        }
        Cause::Signal { sig } => {
            crate::serial_println!("[crash]   signal {} with no handler", sig);
        }
    }
}

/// Reads the binary back and indexes its symbols. Only reached from a fault the
/// process itself took, so the process is not holding the VFS lock: it was in
/// ring 3 when it died
fn load_symbols(path: &str) -> Option<SymbolTable> {
    // By range: read whole, a binary over the 64 MiB read cap would have no
    // names
    let file = crate::process::elf_source::FileSource::open(path)?;
    SymbolTable::from_file(&file)
}

fn print_frames(frames: &[Frame], table: Option<&SymbolTable>, bias: u64) {
    for (i, f) in frames.iter().enumerate() {
        let mark = if f.guessed { "  ?" } else { "" };
        // The table holds link-time addresses; the frame holds a runtime one,
        // less the bias
        let unbiased = f.pc.checked_sub(bias);
        match (table, unbiased) {
            (Some(t), Some(a)) => match t.resolve(a) {
                Some((name, off)) => crate::serial_println!(
                    "[crash]   #{:<2} {:#018x}  {}+{:#x}{}", i, a, name, off, mark,
                ),
                None => crate::serial_println!(
                    "[crash]   #{:<2} {:#018x}  (no symbol){}", i, a, mark,
                ),
            },
            _ => crate::serial_println!("[crash]   #{:<2} {:#018x}{}", i, f.pc, mark),
        }
    }
}

fn print_regs(r: &Regs) {
    crate::serial_println!(
        "[crash]   rip={:#018x} rsp={:#018x} rbp={:#018x}", r.rip, r.rsp, r.rbp,
    );
    crate::serial_println!("[crash]   rflags={:#x} cs={:#x}", r.rflags, r.cs);
}

/// The maps, as many as `MAPS_SHOWN`, centred on `near`
fn print_maps(cr3: u64, near: u64) {
    const MAPS_SHOWN: usize = 24;
    let vmas = crate::mm::mmap::vma_snapshot(cr3);
    if vmas.is_empty() {
        return;
    }
    let at = vmas.iter().position(|v| v.end > near).unwrap_or(vmas.len());
    let first = at.saturating_sub(MAPS_SHOWN / 2).min(vmas.len().saturating_sub(MAPS_SHOWN));
    crate::serial_println!("[crash]   maps near {:#x} ({} of {}):", near, MAPS_SHOWN.min(vmas.len()), vmas.len());
    for v in vmas.iter().skip(first).take(MAPS_SHOWN) {
        crate::serial_println!(
            "[crash]     {:#014x}-{:#014x} {}{}{}",
            v.start, v.end,
            if v.prot & crate::mm::mmap::PROT_READ  != 0 { "r" } else { "-" },
            if v.prot & crate::mm::mmap::PROT_WRITE != 0 { "w" } else { "-" },
            if v.prot & crate::mm::mmap::PROT_EXEC  != 0 { "x" } else { "-" },
        );
    }
}

/// Bounds for the user stack walk. The VMA the stack pointer sits in is the
/// answer; when there is none (the stack pointer itself is nonsense) the page
/// it points at, so the walk is bounded rather than skipped
fn user_stack_bounds(cr3: u64, rsp: u64) -> (u64, u64) {
    match crate::mm::mmap::vma_lookup(cr3, rsp) {
        Some(v) => (v.start, v.end),
        None => (rsp & !0xFFF, (rsp & !0xFFF).saturating_add(0x1000)),
    }
}

/// First eight bytes of a build-id, which is enough to tell two builds
/// apart without filling the line
struct HexShort<'a>(&'a [u8]);

impl core::fmt::Display for HexShort<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for b in self.0.iter().take(8) {
            write!(f, "{:02x}", b)?;
        }
        if self.0.len() > 8 {
            write!(f, "...")?;
        }
        Ok(())
    }
}

/// Symbolize one address of a named binary. Backs `elf addr`
pub fn symbolize_file(path: &str, addr: u64) -> Option<(String, u64)> {
    let t = load_symbols(path)?;
    t.resolve(addr).map(|(n, off)| (String::from(n), off))
}

/// Frames as text, for anything that wants them without printing, such as the
/// core-dump writer
pub fn frames_of_user(cr3: u64, rbp: u64, rsp: u64) -> Vec<Frame> {
    let (lo, hi) = user_stack_bounds(cr3, rsp);
    backtrace::user_frames(cr3, rbp, rsp, lo, hi)
}
