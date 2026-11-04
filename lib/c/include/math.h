/* The C math library: musl's algorithms through the `libm` crate, exported
   by libc_miku.so (lib/libmiku/cmath.rs). The declarations are that file's
   signatures; the classification macros are the compiler's builtins */
#ifndef _MATH_H
#define _MATH_H

#include <_miku.h>

#define HUGE_VAL __builtin_huge_val()
#define HUGE_VALF __builtin_huge_valf()
#define INFINITY __builtin_inff()
#define NAN __builtin_nanf("")
#define FP_NAN 0
#define FP_INFINITE 1
#define FP_ZERO 2
#define FP_SUBNORMAL 3
#define FP_NORMAL 4
#define fpclassify(x) __builtin_fpclassify(FP_NAN, FP_INFINITE, FP_NORMAL, FP_SUBNORMAL, FP_ZERO, x)
#define isnan(x) __builtin_isnan(x)
#define isinf(x) __builtin_isinf(x)
#define isfinite(x) __builtin_isfinite(x)
#define isnormal(x) __builtin_isnormal(x)
#define signbit(x) __builtin_signbit(x)
#define isgreater(x, y) __builtin_isgreater(x, y)
#define isless(x, y) __builtin_isless(x, y)
#define isunordered(x, y) __builtin_isunordered(x, y)

#define M_E 2.7182818284590452354
#define M_LN2 0.69314718055994530942
#define M_LN10 2.30258509299404568402
#define M_PI 3.14159265358979323846
#define M_PI_2 1.57079632679489661923
#define M_PI_4 0.78539816339744830962
#define M_SQRT2 1.41421356237309504880

__MIKU_BEGIN
double acos(double);
float acosf(float);
double acosh(double);
float acoshf(float);
double asin(double);
float asinf(float);
double asinh(double);
float asinhf(float);
double atan(double);
double atan2(double, double);
float atan2f(float, float);
float atanf(float);
double atanh(double);
float atanhf(float);
double cbrt(double);
float cbrtf(float);
double ceil(double);
float ceilf(float);
double copysign(double, double);
float copysignf(float, float);
double cos(double);
float cosf(float);
double cosh(double);
float coshf(float);
double erf(double);
double erfc(double);
float erfcf(float);
float erff(float);
double exp(double);
double exp10(double);
float exp10f(float);
double exp2(double);
float exp2f(float);
float expf(float);
double expm1(double);
float expm1f(float);
double fabs(double);
float fabsf(float);
double fdim(double, double);
float fdimf(float, float);
double floor(double);
float floorf(float);
double fma(double, double, double);
float fmaf(float, float, float);
double fmax(double, double);
float fmaxf(float, float);
double fmaximum(double, double);
double fmaximum_num(double, double);
float fmaximum_numf(float, float);
float fmaximumf(float, float);
double fmin(double, double);
float fminf(float, float);
double fminimum(double, double);
double fminimum_num(double, double);
float fminimum_numf(float, float);
float fminimumf(float, float);
double fmod(double, double);
float fmodf(float, float);
double hypot(double, double);
float hypotf(float, float);
int ilogb(double);
int ilogbf(float);
double j0(double);
float j0f(float);
double j1(double);
float j1f(float);
double jn(int, double);
float jnf(int, float);
double ldexp(double, int);
float ldexpf(float, int);
double lgamma(double);
float lgammaf(float);
double log(double);
double log10(double);
float log10f(float);
double log1p(double);
float log1pf(float);
double log2(double);
float log2f(float);
float logf(float);
double nextafter(double, double);
float nextafterf(float, float);
double pow(double, double);
float powf(float, float);
double remainder(double, double);
float remainderf(float, float);
double rint(double);
float rintf(float);
long lrint(double);
double round(double);
double roundeven(double);
float roundevenf(float);
float roundf(float);
double scalbn(double, int);
float scalbnf(float, int);
double sin(double);
float sinf(float);
double sinh(double);
float sinhf(float);
double sqrt(double);
float sqrtf(float);
double tan(double);
float tanf(float);
double tanh(double);
float tanhf(float);
double tgamma(double);
float tgammaf(float);
double trunc(double);
float truncf(float);
double y0(double);
float y0f(float);
double y1(double);
float y1f(float);
double yn(int, double);
float ynf(int, float);
double frexp(double, int *);
float frexpf(float, int *);
double lgamma_r(double, int *);
float lgammaf_r(float, int *);
double modf(double, double *);
float modff(float, float *);
double remquo(double, double, int *);
float remquof(float, float, int *);
void sincos(double, double *, double *);
void sincosf(float, float *, float *);
__MIKU_END

#endif
