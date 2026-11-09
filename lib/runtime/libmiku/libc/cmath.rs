//! The C math library, for `libc_miku.so`.
//!
//! `std`'s floating-point methods end in C: `f64::powf` calls `pow`, `sin`
//! calls `sin`, and without an SSE4.1 baseline `floor` and `trunc` are calls
//! too.
//!
//! This is the `libm` crate, the Rust port of musl's libm. Each function is
//! written out, since `lib/buildgen` finds exports by reading source and a
//! macro's output is not in the source.
//!
//! `libm` on x86_64 uses inline assembly for `sqrt` and portable code
//! elsewhere, so nothing lowers back into a libcall of itself.
//!
//! Functions returning two values take an out-pointer in C's order:
//! `frexp(x, &exp)`, `modf(x, &int_part)`, `remquo(x, y, &quo)`,
//! `lgamma_r(x, &sign)`, `sincos(x, &sin, &cos)`.

#[no_mangle]
#[inline(never)]
pub extern "C" fn acos(x: f64) -> f64 {
    libm::acos(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn acosf(x: f32) -> f32 {
    libm::acosf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn acosh(x: f64) -> f64 {
    libm::acosh(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn acoshf(x: f32) -> f32 {
    libm::acoshf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn asin(x: f64) -> f64 {
    libm::asin(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn asinf(x: f32) -> f32 {
    libm::asinf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn asinh(x: f64) -> f64 {
    libm::asinh(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn asinhf(x: f32) -> f32 {
    libm::asinhf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn atan(x: f64) -> f64 {
    libm::atan(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn atan2f(y: f32, x: f32) -> f32 {
    libm::atan2f(y, x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn atanf(x: f32) -> f32 {
    libm::atanf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn atanh(x: f64) -> f64 {
    libm::atanh(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn atanhf(x: f32) -> f32 {
    libm::atanhf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn cbrt(x: f64) -> f64 {
    libm::cbrt(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn cbrtf(x: f32) -> f32 {
    libm::cbrtf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ceil(x: f64) -> f64 {
    libm::ceil(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ceilf(x: f32) -> f32 {
    libm::ceilf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn copysign(x: f64, y: f64) -> f64 {
    libm::copysign(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn copysignf(x: f32, y: f32) -> f32 {
    libm::copysignf(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn cos(x: f64) -> f64 {
    libm::cos(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn cosf(x: f32) -> f32 {
    libm::cosf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn cosh(x: f64) -> f64 {
    libm::cosh(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn coshf(x: f32) -> f32 {
    libm::coshf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn erf(x: f64) -> f64 {
    libm::erf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn erfc(x: f64) -> f64 {
    libm::erfc(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn erfcf(x: f32) -> f32 {
    libm::erfcf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn erff(x: f32) -> f32 {
    libm::erff(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn exp(x: f64) -> f64 {
    libm::exp(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn exp10(x: f64) -> f64 {
    libm::exp10(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn exp10f(x: f32) -> f32 {
    libm::exp10f(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn exp2(x: f64) -> f64 {
    libm::exp2(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn exp2f(x: f32) -> f32 {
    libm::exp2f(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn expf(x: f32) -> f32 {
    libm::expf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn expm1(x: f64) -> f64 {
    libm::expm1(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn expm1f(x: f32) -> f32 {
    libm::expm1f(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fabs(x: f64) -> f64 {
    libm::fabs(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fabsf(x: f32) -> f32 {
    libm::fabsf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fdim(x: f64, y: f64) -> f64 {
    libm::fdim(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fdimf(x: f32, y: f32) -> f32 {
    libm::fdimf(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn floor(x: f64) -> f64 {
    libm::floor(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn floorf(x: f32) -> f32 {
    libm::floorf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fma(x: f64, y: f64, z: f64) -> f64 {
    libm::fma(x, y, z)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fmaf(x: f32, y: f32, z: f32) -> f32 {
    libm::fmaf(x, y, z)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fmax(x: f64, y: f64) -> f64 {
    libm::fmax(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fmaxf(x: f32, y: f32) -> f32 {
    libm::fmaxf(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fmaximum(x: f64, y: f64) -> f64 {
    libm::fmaximum(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fmaximum_num(x: f64, y: f64) -> f64 {
    libm::fmaximum_num(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fmaximum_numf(x: f32, y: f32) -> f32 {
    libm::fmaximum_numf(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fmaximumf(x: f32, y: f32) -> f32 {
    libm::fmaximumf(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fmin(x: f64, y: f64) -> f64 {
    libm::fmin(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fminf(x: f32, y: f32) -> f32 {
    libm::fminf(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fminimum(x: f64, y: f64) -> f64 {
    libm::fminimum(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fminimum_num(x: f64, y: f64) -> f64 {
    libm::fminimum_num(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fminimum_numf(x: f32, y: f32) -> f32 {
    libm::fminimum_numf(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fminimumf(x: f32, y: f32) -> f32 {
    libm::fminimumf(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fmod(x: f64, y: f64) -> f64 {
    libm::fmod(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fmodf(x: f32, y: f32) -> f32 {
    libm::fmodf(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn hypot(x: f64, y: f64) -> f64 {
    libm::hypot(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn hypotf(x: f32, y: f32) -> f32 {
    libm::hypotf(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ilogb(x: f64) -> i32 {
    libm::ilogb(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ilogbf(x: f32) -> i32 {
    libm::ilogbf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn j0(x: f64) -> f64 {
    libm::j0(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn j0f(x: f32) -> f32 {
    libm::j0f(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn j1(x: f64) -> f64 {
    libm::j1(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn j1f(x: f32) -> f32 {
    libm::j1f(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn jn(n: i32, x: f64) -> f64 {
    libm::jn(n, x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn jnf(n: i32, x: f32) -> f32 {
    libm::jnf(n, x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ldexp(x: f64, n: i32) -> f64 {
    libm::ldexp(x, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ldexpf(x: f32, n: i32) -> f32 {
    libm::ldexpf(x, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn lgamma(x: f64) -> f64 {
    libm::lgamma(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn lgammaf(x: f32) -> f32 {
    libm::lgammaf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn log(x: f64) -> f64 {
    libm::log(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn log10(x: f64) -> f64 {
    libm::log10(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn log10f(x: f32) -> f32 {
    libm::log10f(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn log1p(x: f64) -> f64 {
    libm::log1p(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn log1pf(x: f32) -> f32 {
    libm::log1pf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn log2(x: f64) -> f64 {
    libm::log2(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn log2f(x: f32) -> f32 {
    libm::log2f(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn logf(x: f32) -> f32 {
    libm::logf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn nextafter(x: f64, y: f64) -> f64 {
    libm::nextafter(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn nextafterf(x: f32, y: f32) -> f32 {
    libm::nextafterf(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn pow(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn powf(x: f32, y: f32) -> f32 {
    libm::powf(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn remainder(x: f64, y: f64) -> f64 {
    libm::remainder(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn remainderf(x: f32, y: f32) -> f32 {
    libm::remainderf(x, y)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn rint(x: f64) -> f64 {
    libm::rint(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn rintf(x: f32) -> f32 {
    libm::rintf(x)
}

/// `rint` as a `long`: to the nearest integer, ties to even, the only rounding
/// mode here (there is no `fesetround`). Out of range is unspecified in C; this
/// saturates, as the cast does
#[no_mangle]
#[inline(never)]
pub extern "C" fn lrint(x: f64) -> i64 {
    libm::rint(x) as i64
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn round(x: f64) -> f64 {
    libm::round(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn roundeven(x: f64) -> f64 {
    libm::roundeven(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn roundevenf(x: f32) -> f32 {
    libm::roundevenf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn roundf(x: f32) -> f32 {
    libm::roundf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn scalbn(x: f64, n: i32) -> f64 {
    libm::scalbn(x, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn scalbnf(x: f32, n: i32) -> f32 {
    libm::scalbnf(x, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sin(x: f64) -> f64 {
    libm::sin(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sinf(x: f32) -> f32 {
    libm::sinf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sinh(x: f64) -> f64 {
    libm::sinh(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sinhf(x: f32) -> f32 {
    libm::sinhf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sqrt(x: f64) -> f64 {
    libm::sqrt(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sqrtf(x: f32) -> f32 {
    libm::sqrtf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn tan(x: f64) -> f64 {
    libm::tan(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn tanf(x: f32) -> f32 {
    libm::tanf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn tanh(x: f64) -> f64 {
    libm::tanh(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn tanhf(x: f32) -> f32 {
    libm::tanhf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn tgamma(x: f64) -> f64 {
    libm::tgamma(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn tgammaf(x: f32) -> f32 {
    libm::tgammaf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn trunc(x: f64) -> f64 {
    libm::trunc(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn truncf(x: f32) -> f32 {
    libm::truncf(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn y0(x: f64) -> f64 {
    libm::y0(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn y0f(x: f32) -> f32 {
    libm::y0f(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn y1(x: f64) -> f64 {
    libm::y1(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn y1f(x: f32) -> f32 {
    libm::y1f(x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn yn(n: i32, x: f64) -> f64 {
    libm::yn(n, x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn ynf(n: i32, x: f32) -> f32 {
    libm::ynf(n, x)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn frexp(x: f64, exp: *mut i32) -> f64 {
    let (r, o) = libm::frexp(x);
    if !exp.is_null() { unsafe { exp.write(o) }; }
    r
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn frexpf(x: f32, exp: *mut i32) -> f32 {
    let (r, o) = libm::frexpf(x);
    if !exp.is_null() { unsafe { exp.write(o) }; }
    r
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn lgamma_r(x: f64, sign: *mut i32) -> f64 {
    let (r, o) = libm::lgamma_r(x);
    if !sign.is_null() { unsafe { sign.write(o) }; }
    r
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn lgammaf_r(x: f32, sign: *mut i32) -> f32 {
    let (r, o) = libm::lgammaf_r(x);
    if !sign.is_null() { unsafe { sign.write(o) }; }
    r
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn modf(x: f64, iptr: *mut f64) -> f64 {
    let (r, o) = libm::modf(x);
    if !iptr.is_null() { unsafe { iptr.write(o) }; }
    r
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn modff(x: f32, iptr: *mut f32) -> f32 {
    let (r, o) = libm::modff(x);
    if !iptr.is_null() { unsafe { iptr.write(o) }; }
    r
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn remquo(x: f64, y: f64, quo: *mut i32) -> f64 {
    let (r, o) = libm::remquo(x, y);
    if !quo.is_null() { unsafe { quo.write(o) }; }
    r
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn remquof(x: f32, y: f32, quo: *mut i32) -> f32 {
    let (r, o) = libm::remquof(x, y);
    if !quo.is_null() { unsafe { quo.write(o) }; }
    r
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sincos(x: f64, s: *mut f64, c: *mut f64) {
    let (sv, cv) = libm::sincos(x);
    unsafe {
        if !s.is_null() { s.write(sv); }
        if !c.is_null() { c.write(cv); }
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sincosf(x: f32, s: *mut f32, c: *mut f32) {
    let (sv, cv) = libm::sincosf(x);
    unsafe {
        if !s.is_null() { s.write(sv); }
        if !c.is_null() { c.write(cv); }
    }
}
