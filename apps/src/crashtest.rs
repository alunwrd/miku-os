// Crashes on purpose, for reading the crash report
//
// Expected on the serial log:
//
//   [crash] pid=N 'crashtest' SIGSEGV at 0x...
//   [crash]   write to 0x0 (page not present)
//   [crash]   image /bin/crashtest  bias=0x...  build-id ...
//   [crash]   #0  0x...  level_three+0x...
//   [crash]   #1  0x...  level_two+0x...
//   [crash]   #2  0x...  level_one+0x...
//   [core] /tmp/core.crashtest.N (N bytes)
//
// crashtest null     null dereference (default)
// crashtest exec     jump into data (#PF on an NX page)
// crashtest ud       bad opcode (#UD)
// crashtest deep     null dereference 12 frames down
// crashtest ok       exits 0
#![no_std]
#![no_main]

pub use miku;

/// Must be real named frames
#[inline(never)]
fn level_three(mode: u8) -> u64 {
    match mode {
        // Write through null: "write to 0x0 (page not present)"
        0 => unsafe {
            let p: *mut u64 = core::ptr::null_mut();
            core::ptr::write_volatile(p, 0xDEAD);
            0
        },
        // Call into a mapped non-executable page: an instruction fetch fault
        1 => unsafe {
            static DATA: [u8; 8] = [0x90, 0x90, 0x90, 0x90, 0xC3, 0, 0, 0];
            let f: extern "C" fn() = core::mem::transmute(DATA.as_ptr());
            f();
            0
        },
        // Invalid instruction
        2 => unsafe {
            core::arch::asm!("ud2", options(nomem, nostack));
            0
        },
        _ => 0,
    }
}

#[inline(never)]
fn level_two(mode: u8) -> u64 {
    core::hint::black_box(level_three(mode)) + 1
}

#[inline(never)]
fn level_one(mode: u8) -> u64 {
    core::hint::black_box(level_two(mode)) + 1
}

/// Recursion to a known depth before faulting
#[inline(never)]
fn descend(depth: u8) -> u64 {
    if depth == 0 {
        return level_one(0);
    }
    core::hint::black_box(descend(depth - 1)) + 1
}

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let mode = arg_mode(argc, argv);
    unsafe {
        match mode {
            Mode::Ok => {
                miku::miku_puts(miku::cstr!("crashtest: nothing to do\n"));
                miku::exit(0);
            }
            Mode::Deep => {
                miku::miku_puts(miku::cstr!("crashtest: faulting 12 frames down\n"));
                core::hint::black_box(descend(12));
            }
            Mode::Null => {
                miku::miku_puts(miku::cstr!("crashtest: writing through a null pointer\n"));
                core::hint::black_box(level_one(0));
            }
            Mode::Exec => {
                miku::miku_puts(miku::cstr!("crashtest: calling into data\n"));
                core::hint::black_box(level_one(1));
            }
            Mode::Ud => {
                miku::miku_puts(miku::cstr!("crashtest: executing ud2\n"));
                core::hint::black_box(level_one(2));
            }
        }
        // The fault did not happen, so the protection is off
        miku::miku_puts(miku::cstr!("crashtest: SURVIVED - the fault did not happen\n"));
        miku::exit(1);
    }
}

enum Mode { Null, Exec, Ud, Deep, Ok }

fn arg_mode(argc: i32, argv: *const *const u8) -> Mode {
    if argc < 2 || argv.is_null() {
        return Mode::Null;
    }
    let a = unsafe { *argv.add(1) };
    if a.is_null() {
        return Mode::Null;
    }
    if starts_with(a, b"exec") { Mode::Exec }
    else if starts_with(a, b"ud")   { Mode::Ud }
    else if starts_with(a, b"deep") { Mode::Deep }
    else if starts_with(a, b"ok")   { Mode::Ok }
    else { Mode::Null }
}

fn starts_with(s: *const u8, want: &[u8]) -> bool {
    for (i, w) in want.iter().enumerate() {
        let c = unsafe { *s.add(i) };
        if c != *w {
            return false;
        }
    }
    // Whole argument, not a prefix
    unsafe { *s.add(want.len()) == 0 }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(9);
}
