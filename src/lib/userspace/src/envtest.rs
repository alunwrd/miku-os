// End-to-end test for the exec/ELF-loader plumbing added with envp support.
// Communicates through the exit code (visible in the serial log as
// "[syscall] exit pid=N code=..."):
//   0 = all good
//   1 = miku_env_init imported nothing (envp missing/empty on the stack)
//   2 = PATH not found after init
//   3 = PATH has the wrong value (expected "/bin" from DEFAULT_ENV)
//   4 = argc/argv broken
//   5 = fork failed
//   6 = waitpid returned wrong pid or wrong child status
#![no_std]
#![no_main]

#[path = "miku.rs"]
mod miku;

#[no_mangle]
pub extern "C" fn _start_main(
    argc: i32,
    argv: *const *const u8,
    envp: *const *const u8,
) -> ! {
    unsafe {
        if argc < 1 || argv.is_null() || (*argv).is_null() {
            miku::exit(4);
        }

        let imported = miku::miku_env_init(envp);
        if imported == 0 {
            miku::exit(1);
        }

        let path = miku::miku_getenv(b"PATH\0".as_ptr());
        if path.is_null() {
            miku::exit(2);
        }
        let expect = b"/bin\0";
        for (i, &e) in expect.iter().enumerate() {
            if *path.add(i) != e {
                miku::exit(3);
            }
        }

        miku::println("envtest: envp + getenv OK, testing fork/waitpid...");

        let r = miku::miku_fork();
        if r == 0 {
            // child
            miku::exit(7);
        }
        if r < 0 {
            // encode errno: exit 50+errno (51=EPERM, 62=ENOMEM, ...)
            miku::exit(50 - r);
        }
        let mut status: i64 = -1;
        let reaped = miku::miku_waitpid(r as u64, &mut status);
        if reaped != r {
            // 20 = wrong/negative pid from waitpid (21 = ECHILD, ...)
            miku::exit(if reaped < 0 { 20 - reaped } else { 20 });
        }
        if status != 7 {
            // 30 + status: 29 = status never written (stayed -1)
            miku::exit(30 + status);
        }

        miku::println("envtest: all OK");
    }
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(9);
}
