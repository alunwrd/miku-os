// Integer math library - no floating point, suitable for kernel/embedded

#[no_mangle]
pub extern "C" fn miku_abs(x: i64) -> i64 {
    x.saturating_abs()
}

#[no_mangle]
pub extern "C" fn miku_min(a: i64, b: i64) -> i64 {
    if a < b { a } else { b }
}

#[no_mangle]
pub extern "C" fn miku_max(a: i64, b: i64) -> i64 {
    if a > b { a } else { b }
}

#[no_mangle]
pub extern "C" fn miku_clamp(val: i64, lo: i64, hi: i64) -> i64 {
    if val < lo { lo } else if val > hi { hi } else { val }
}

#[no_mangle]
pub extern "C" fn miku_swap(a: *mut u64, b: *mut u64) {
    if a.is_null() || b.is_null() { return; }
    unsafe { let tmp = *a; *a = *b; *b = tmp; }
}

#[no_mangle]
pub extern "C" fn miku_umin(a: u64, b: u64) -> u64 {
    if a < b { a } else { b }
}

#[no_mangle]
pub extern "C" fn miku_umax(a: u64, b: u64) -> u64 {
    if a > b { a } else { b }
}

// Greatest common divisor (Euclidean algorithm)
#[no_mangle]
pub extern "C" fn miku_gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

// Least common multiple
#[no_mangle]
pub extern "C" fn miku_lcm(a: u64, b: u64) -> u64 {
    if a == 0 || b == 0 { return 0; }
    a / miku_gcd(a, b) * b
}

// Integer power: base^exp
#[no_mangle]
pub extern "C" fn miku_pow(mut base: i64, mut exp: u32) -> i64 {
    if exp == 0 { return 1; }
    let mut result: i64 = 1;
    while exp > 0 {
        if exp & 1 != 0 {
            result = result.wrapping_mul(base);
        }
        base = base.wrapping_mul(base);
        exp >>= 1;
    }
    result
}

// Unsigned power
#[no_mangle]
pub extern "C" fn miku_upow(mut base: u64, mut exp: u32) -> u64 {
    if exp == 0 { return 1; }
    let mut result: u64 = 1;
    while exp > 0 {
        if exp & 1 != 0 {
            result = result.wrapping_mul(base);
        }
        base = base.wrapping_mul(base);
        exp >>= 1;
    }
    result
}

// Integer square root (Newton's method)
#[no_mangle]
pub extern "C" fn miku_isqrt(n: u64) -> u64 {
    if n < 2 { return n; }
    let bits = (64 - n.leading_zeros() + 1) / 2;
    let mut x = 1u64 << bits;
    loop {
        let q = n / x;
        if q >= x { return x; }
        x = (x + q) / 2;
    }
}

// Integer cube root
#[no_mangle]
pub extern "C" fn miku_icbrt(n: u64) -> u64 {
    if n < 2 { return n; }
    let mut x = n;
    let mut y = (2 * x + n / (x * x)) / 3;
    while y < x {
        x = y;
        if x == 0 { return 0; }
        y = (2 * x + n / (x * x)) / 3;
    }
    x
}

// Integer log base 2 (floor)
#[no_mangle]
pub extern "C" fn miku_ilog2(n: u64) -> u32 {
    if n == 0 { return 0; }
    63 - (n.leading_zeros())
}

// Integer log base 10 (floor)
#[no_mangle]
pub extern "C" fn miku_ilog10(mut n: u64) -> u32 {
    if n == 0 { return 0; }
    let mut count = 0u32;
    while n >= 10 {
        n /= 10;
        count += 1;
    }
    count
}

// Sign function: returns -1, 0, or 1
#[no_mangle]
pub extern "C" fn miku_sign(x: i64) -> i32 {
    if x > 0 { 1 } else if x < 0 { -1 } else { 0 }
}

// Map value from one range to another (integer)
// map(val, in_lo, in_hi, out_lo, out_hi)
#[no_mangle]
pub extern "C" fn miku_map(val: i64, in_lo: i64, in_hi: i64, out_lo: i64, out_hi: i64) -> i64 {
    if in_hi == in_lo { return out_lo; }
    out_lo + (val - in_lo) * (out_hi - out_lo) / (in_hi - in_lo)
}

// Linear interpolation: lerp(a, b, t) where t is 0..1000 (permille)
#[no_mangle]
pub extern "C" fn miku_lerp(a: i64, b: i64, t_permille: u32) -> i64 {
    a + (b - a) * t_permille as i64 / 1000
}

// Saturating add/sub for i64
#[no_mangle]
pub extern "C" fn miku_sadd(a: i64, b: i64) -> i64 {
    a.saturating_add(b)
}

#[no_mangle]
pub extern "C" fn miku_ssub(a: i64, b: i64) -> i64 {
    a.saturating_sub(b)
}

#[no_mangle]
pub extern "C" fn miku_smul(a: i64, b: i64) -> i64 {
    a.saturating_mul(b)
}

// Unsigned saturating arithmetic
#[no_mangle]
pub extern "C" fn miku_usadd(a: u64, b: u64) -> u64 {
    a.saturating_add(b)
}

#[no_mangle]
pub extern "C" fn miku_ussub(a: u64, b: u64) -> u64 {
    a.saturating_sub(b)
}

// Division with rounding up
#[no_mangle]
pub extern "C" fn miku_div_ceil(a: u64, b: u64) -> u64 {
    if b == 0 { return 0; }
    a / b + if a % b != 0 { 1 } else { 0 }
}

// Modular exponentiation: (base^exp) % modulus
// Useful for cryptographic operations
#[no_mangle]
pub extern "C" fn miku_modpow(mut base: u64, mut exp: u64, modulus: u64) -> u64 {
    if modulus <= 1 { return 0; }
    let mut result: u64 = 1;
    base %= modulus;
    while exp > 0 {
        if exp & 1 != 0 {
            result = mul_mod(result, base, modulus);
        }
        exp >>= 1;
        base = mul_mod(base, base, modulus);
    }
    result
}

// Multiplication with modulus (handles overflow via 128-bit intermediate)
fn mul_mod(a: u64, b: u64, m: u64) -> u64 {
    ((a as u128 * b as u128) % m as u128) as u64
}

// Check if n is prime (trial division, sufficient for moderate values)
#[no_mangle]
pub extern "C" fn miku_is_prime(n: u64) -> i32 {
    if n < 2 { return 0; }
    if n < 4 { return 1; }
    if n % 2 == 0 || n % 3 == 0 { return 0; }
    let sqrt_n = miku_isqrt(n);
    let mut i = 5u64;
    while i <= sqrt_n {
        if n % i == 0 || n % (i + 2) == 0 { return 0; }
        i += 6;
    }
    1
}

// Fibonacci number (iterative)
#[no_mangle]
pub extern "C" fn miku_fib(n: u32) -> u64 {
    if n == 0 { return 0; }
    let (mut a, mut b) = (0u64, 1u64);
    for _ in 1..n {
        let tmp = b;
        b = a.wrapping_add(b);
        a = tmp;
    }
    b
}

// Factorial (wrapping on overflow)
#[no_mangle]
pub extern "C" fn miku_factorial(n: u32) -> u64 {
    let mut result: u64 = 1;
    for i in 2..=n as u64 {
        result = result.wrapping_mul(i);
    }
    result
}

// Binomial coefficient C(n, k)
#[no_mangle]
pub extern "C" fn miku_binomial(n: u64, k: u64) -> u64 {
    if k > n { return 0; }
    let k = if k > n - k { n - k } else { k };
    let mut result: u64 = 1;
    for i in 0..k {
        result = result / (i + 1) * (n - i) + result % (i + 1) * (n - i) / (i + 1);
    }
    result
}
