// shim for `stdio`:
// forwards to C symbols exported by the library that owns stdio.rs,
// resolved at load time by ld-miku.

mod ffi {
    #[allow(unused_imports)] use super::*;
    #[allow(improper_ctypes)]
    extern "C" {
        pub fn miku_print(s: *const u8) ;
        pub fn miku_println(s: *const u8) ;
        pub fn miku_puts(s: *const u8) -> i32;
        pub fn miku_eprint(s: *const u8) ;
        pub fn miku_eprintln(s: *const u8) ;
        pub fn miku_putchar(c: i32) -> i32;
        pub fn miku_getchar() -> i32;
        pub fn miku_readline(buf: *mut u8, max_len: usize) -> i32;
        pub fn miku_getline() -> *mut u8;
    }
}

#[inline]
pub fn miku_print(s: *const u8)  { unsafe { ffi::miku_print(s) } }
#[inline]
pub fn miku_println(s: *const u8)  { unsafe { ffi::miku_println(s) } }
#[inline]
pub fn miku_puts(s: *const u8) -> i32 { unsafe { ffi::miku_puts(s) } }
#[inline]
pub fn miku_eprint(s: *const u8)  { unsafe { ffi::miku_eprint(s) } }
#[inline]
pub fn miku_eprintln(s: *const u8)  { unsafe { ffi::miku_eprintln(s) } }
#[inline]
pub fn miku_putchar(c: i32) -> i32 { unsafe { ffi::miku_putchar(c) } }
#[inline]
pub fn miku_getchar() -> i32 { unsafe { ffi::miku_getchar() } }
#[inline]
pub fn miku_readline(buf: *mut u8, max_len: usize) -> i32 { unsafe { ffi::miku_readline(buf, max_len) } }
#[inline]
pub fn miku_getline() -> *mut u8 { unsafe { ffi::miku_getline() } }

// ===== manual additions below (preserved by the generator) =====
