//! Montgomery arithmetic against values computed with Python's `pow`

use mikucrypto::bignum::*;

/// 2048-bit modulus, an RSA-2048 public modulus
const N_HEX: &str = "\
8b2f100f0590f81ce21bc9b3c80458d1b1d58d489333cbfadacd61eee832e160\
09b7653ef21b5a33d9bb0d2cac90b0e2e8f59b17562faa3a6d24519e78d6f67c\
23a56db24fe7d584bb8290b87f6daaee8f5bebec9f2e0520feab919b1a06f6b4\
eb0608017162d263174b0bf2faf28318b5f298c6a5631014c3a495aff766133a\
2c3835c8412b1df2765cd0aa28077aa3e23c5ddec36b13e9db5a9e7339a2a893\
adb85977f3ec2834cfd566d658c79d5a0d3ab3d21858b5e351a57b31f8da4a8f\
a98969144753e99e31747f470139aa5adf509d1913751029d5db91206d891ebb\
d2767b475655ba37cfdf3e662ee27a06cfd44aeaa33b9ad54585404711f46351";

/// Computed with Python:
///
///     random.seed(20260825)
///     n = random.getrandbits(2048) | (1 << 2047) | 1
///     pow(base, e, n)

fn unhex(s: &str) -> Vec<u8> {
    let mut clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    // The byte encoding needs an even number of nibbles
    if clean.len() % 2 == 1 { clean.insert(0, '0'); }
    (0..clean.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).unwrap())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::new();
    let mut leading = true;
    for b in bytes {
        if leading && *b == 0 {
            continue;
        }
        leading = false;
        s.push_str(&format!("{:02x}", b));
    }
    if s.is_empty() { s.push('0'); }
    // Python prints 0x400, not 0x0400. Drops one leading nibble so expectations
    // can be copied from a session
    if s.starts_with('0') && s.len() > 1 { s.remove(0); }
    s
}

#[test]
fn round_trips_through_the_byte_encoding() {
    let bytes = unhex(N_HEX);
    let n: BigNum = bn_from_bytes_be(&bytes).expect("fits in 4096 bits");
    let mut back = [0u8; 512];
    bn_to_bytes_be(&n, &mut back);
    // Fixed-width encoding, so the value sits at the end
    assert_eq!(&back[512 - bytes.len()..], &bytes[..]);
    assert_eq!(bn_bits(&n), 2048);
}

#[test]
fn small_powers_match_python_pow() {
    // pow(base, e, m) for a modulus small enough to write down
    let m = unhex("f000000000000000000000000000000000000000000000000000000000000007");
    let ctx = MontCtx::new(&bn_from_bytes_be(&m).unwrap()).expect("odd modulus");

    for (base_hex, e, want) in [
        ("02", 10u32, "400"),                      // 2^10 = 1024
        ("03", 5u32, "f3"),                        // 3^5 = 243
        ("10", 4u32, "10000"),                     // 16^4
        ("ff", 3u32, "fd02ff"),                    // 255^3 = 16581375
        ("0abcdef", 65537u32,
         "583a29cf329a33514b911e74be272c41cfd7a3a42ec4f6e6266a23d2c8de6614"),
    ] {
        let b = bn_from_bytes_be(&unhex(base_hex)).unwrap();
        let r = ctx.pow_u32(&b, e);
        let mut out = [0u8; 512];
        bn_to_bytes_be(&r, &mut out);
        assert_eq!(hex(&out), want, "base {} ^ {}", base_hex, e);
    }
}

#[test]
fn the_rsa_exponent_works_on_a_real_modulus() {
    // 65537: the exponent of every certificate in the trust store
    let m = bn_from_bytes_be(&unhex(N_HEX)).unwrap();
    let ctx = MontCtx::new(&m).expect("odd modulus");

    let base = bn_from_bytes_be(&unhex("0abcdef0123456789")).unwrap();

    // pow(0x0abcdef0123456789, 3, N), small enough that no reduction happens
    let cubed = ctx.pow_u32(&base, 3);
    let mut out = [0u8; 512];
    bn_to_bytes_be(&cubed, &mut out);
    assert_eq!(hex(&out), "4d610656642a5a5d4515cf1b8fd136d66fd100c3fb900159");

    // Then with 65537, the longest square-and-multiply chain this runs
    let r = ctx.pow_u32(&base, 65537);
    bn_to_bytes_be(&r, &mut out);
    assert_eq!(hex(&out), "\
649339767aa26140ac928932d4e7cea9e66a1f91d8e9813903bbfdd40991b872\
052fe57e11c1e3600f71bfc8089ae0f2f179ef6d12b3238fb04ff139e84917ee\
ba61ab6233d4b5a2db3a7680c008a2d037f2b9388ae878e329d9d7fcf6061aec\
2f4c97c2a4397619266b9f97a482e29fccaa7568dd2264db7e740585196beb9c\
18ae3210b4a66ae8bb3e3dc274e8750cc10be9f67854866b2fa3a5e188c8438b\
a1b1d1137b12de41c6bd7c0eebc4c84a15c6c98b2a14de3d2f5b701bb05ce8cc\
465d4f392ceaea4482abc876d5c963bd844ea8b736d6dc559bef90ea217ae7e8\
b7e39f8abbaebca331a57e692532083e43b4e743e58f897cd3d6fd26e27ea3c0");

    // Same answer twice: the context caches R^2 and must not mutate
    let again = ctx.pow_u32(&base, 65537);
    assert_eq!(r, again);
}

#[test]
fn the_range_check_is_strict() {
    // RSA's rule: above zero and below n. The verifier checks this before
    // exponentiating
    let m = bn_from_bytes_be(&unhex("f000000000000000000000000000000000000000000000000000000000000007")).unwrap();
    let ctx = MontCtx::new(&m).expect("odd modulus");

    assert!(!ctx.in_range(&m), "x == n must be refused");

    let bigger = bn_from_bytes_be(&unhex("f000000000000000000000000000000000000000000000000000000000000009")).unwrap();
    assert!(!ctx.in_range(&bigger), "x > n must be refused");

    assert!(!ctx.in_range(&zero::<{ MAX_LIMBS }>()), "zero must be refused");
    assert!(ctx.in_range(&one::<{ MAX_LIMBS }>()), "one is in range");
}

#[test]
fn an_even_modulus_has_no_montgomery_form() {
    // -n^-1 mod 2^32 does not exist for even n
    let even = bn_from_bytes_be(&unhex("f0000000000000000000000000000000000000000000000000000000000000f0")).unwrap();
    assert!(MontCtx::new(&even).is_none());
}

#[test]
fn comparison_orders_by_value_not_by_limb() {
    let a: BigNum = bn_from_bytes_be(&unhex("0100")).unwrap();
    let b: BigNum = bn_from_bytes_be(&unhex("ff")).unwrap();
    assert_eq!(bn_cmp(&a, &b), core::cmp::Ordering::Greater);
    assert_eq!(bn_cmp(&b, &a), core::cmp::Ordering::Less);
    assert_eq!(bn_cmp(&a, &a), core::cmp::Ordering::Equal);
}
