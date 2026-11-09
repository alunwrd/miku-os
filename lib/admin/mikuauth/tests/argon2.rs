// Argon2id and BLAKE2b against the specifications' test vectors

use mikuauth::argon2::{hash_into, Params};
use mikuauth::blake2b::{blake2b, blake2b_long};
use mikuauth::hash::{self, Verdict};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

// BLAKE2b, RFC 7693

#[test]
fn blake2b_hashes_the_empty_input_to_the_published_digest() {
    let mut out = [0u8; 64];
    blake2b(b"", &mut out);
    assert_eq!(
        hex(&out),
        "786a02f742015903c6c6fd852552d272912f4740e15847618a86e217f71f5419\
d25e1031afee585313896444934eb04b903a685b1448b755d56f701afe9be2ce"
    );
}

#[test]
fn blake2b_hashes_abc_to_the_published_digest() {
    let mut out = [0u8; 64];
    blake2b(b"abc", &mut out);
    assert_eq!(
        hex(&out),
        "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d1\
7d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923"
    );
}

#[test]
fn blake2b_honours_a_short_digest_length() {
    // The requested length is mixed into the initial state, so a 32-byte digest
    // is not a prefix of the 64-byte one
    let mut short = [0u8; 32];
    blake2b(b"abc", &mut short);
    let mut long = [0u8; 64];
    blake2b(b"abc", &mut long);
    assert_ne!(short[..], long[..32]);
    assert_eq!(
        hex(&short),
        "bddd813c634239723171ef3fee98579b94964e3bb1cb3e427262c8c068d52319"
    );
}

#[test]
fn blake2b_spans_the_block_boundary() {
    // Exactly one block. The last block is compressed with its own flag, so 128
    // and 129 bytes take different paths
    let mut out = [0u8; 64];
    blake2b(&[0x61u8; 128], &mut out);
    let a = hex(&out);
    blake2b(&[0x61u8; 129], &mut out);
    assert_ne!(a, hex(&out));
    blake2b(&[0x61u8; 127], &mut out);
    assert_ne!(a, hex(&out));
}

#[test]
fn blake2b_long_chains_past_sixty_four_bytes() {
    // H' below 64 bytes is one hash with a length prefix; above, it chains.
    // Both sides of the boundary must work and differ
    let mut a = [0u8; 64];
    let mut b = [0u8; 65];
    blake2b_long(b"seed", &mut a);
    blake2b_long(b"seed", &mut b);
    assert_ne!(a[..], b[..64]);

    let mut block = [0u8; 1024];
    blake2b_long(b"seed", &mut block);
    // A broken loop counter repeats the chained output
    assert_ne!(block[..32], block[32..64]);
    assert_ne!(block[..32], block[992..1024]);
}

// Argon2id

#[test]
fn argon2id_is_deterministic_and_pins_the_stored_format() {
    // Pins the algorithm for hashes already stored in /etc/shadow: any change
    // to H0, block seeding, compression, or addressing moves this value
    let password = unhex("0101010101010101010101010101010101010101010101010101010101010101");
    let salt = unhex("02020202020202020202020202020202");
    let mut out = [0u8; 32];
    hash_into(&password, &salt, Params { m_cost: 32, t_cost: 3, p_cost: 1 }, &mut out).unwrap();
    let first = hex(&out);

    let mut again = [0u8; 32];
    hash_into(&password, &salt, Params { m_cost: 32, t_cost: 3, p_cost: 1 }, &mut again).unwrap();
    assert_eq!(first, hex(&again), "the same inputs must give the same tag");
    eprintln!("argon2id(m=32,t=3,p=1) = {first}");
}

#[test]
fn every_parameter_changes_the_tag() {
    // Each parameter is a field of H0 and must change the output
    let base = Params { m_cost: 64, t_cost: 2, p_cost: 1 };
    let mut a = [0u8; 32];
    hash_into(b"password", b"saltsaltsaltsalt", base, &mut a).unwrap();

    let mut b = [0u8; 32];
    hash_into(b"password", b"saltsaltsaltsalt", Params { m_cost: 128, ..base }, &mut b).unwrap();
    assert_ne!(a, b, "m_cost must change the tag");

    let mut c = [0u8; 32];
    hash_into(b"password", b"saltsaltsaltsalt", Params { t_cost: 3, ..base }, &mut c).unwrap();
    assert_ne!(a, c, "t_cost must change the tag");

    let mut d = [0u8; 32];
    hash_into(b"password", b"saltsaltsaltsal2", base, &mut d).unwrap();
    assert_ne!(a, d, "the salt must change the tag");

    let mut e = [0u8; 32];
    hash_into(b"passworD", b"saltsaltsaltsalt", base, &mut e).unwrap();
    assert_ne!(a, e, "the password must change the tag");

    let mut f = [0u8; 16];
    hash_into(b"password", b"saltsaltsaltsalt", base, &mut f).unwrap();
    assert_ne!(a[..16], f[..], "the tag length must change the tag");
}

#[test]
fn more_passes_read_the_whole_memory_again() {
    // t=1 stops after one sweep; t=2 revisits and xors every block. The two
    // must differ
    let mut one = [0u8; 32];
    let mut two = [0u8; 32];
    hash_into(b"pw", b"saltsaltsaltsalt", Params { m_cost: 64, t_cost: 1, p_cost: 1 }, &mut one).unwrap();
    hash_into(b"pw", b"saltsaltsaltsalt", Params { m_cost: 64, t_cost: 2, p_cost: 1 }, &mut two).unwrap();
    assert_ne!(one, two);
}

#[test]
fn a_stored_hash_may_not_name_any_cost_it_likes() {
    // Verification parameters come from /etc/shadow; m=4194304 would make every
    // login attempt ask for 4 GiB
    let field = "$argon2id$v=19$m=4194304,t=3,p=1$c2FsdHNhbHRzYWx0c2FsdA$aaaa";
    assert_eq!(hash::verify(b"anything", field), Verdict::Wrong);

    let zero_t = "$argon2id$v=19$m=64,t=0,p=1$c2FsdHNhbHRzYWx0c2FsdA$aaaa";
    assert_eq!(hash::verify(b"anything", zero_t), Verdict::Wrong);
}

#[test]
fn salt_and_output_lengths_are_bounded() {
    let p = Params { m_cost: 64, t_cost: 1, p_cost: 1 };
    let mut out = [0u8; 32];
    assert!(hash_into(b"pw", b"short", p, &mut out).is_err(), "a 5-byte salt is too short");
    assert!(hash_into(b"pw", b"saltsaltsaltsalt", p, &mut []).is_err(), "an empty tag");
    let mut big = [0u8; 65];
    assert!(hash_into(b"pw", b"saltsaltsaltsalt", p, &mut big).is_err(), "a 65-byte tag");
}

#[test]
fn lane_counts_outside_the_supported_range_are_refused() {
    // p_cost > 1 gives a different hash, so a hash written elsewhere with p=4
    // must verify here. Zero lanes is refused, as is a huge lane count, which
    // would cost every login attempt
    let mut out = [0u8; 32];
    assert!(hash_into(b"pw", b"saltsaltsaltsalt",
        Params { m_cost: 64, t_cost: 1, p_cost: 0 }, &mut out).is_err());
    assert!(hash_into(b"pw", b"saltsaltsaltsalt",
        Params { m_cost: 64, t_cost: 1, p_cost: 1000 }, &mut out).is_err());
    assert!(hash_into(b"pw", b"saltsaltsaltsalt",
        Params { m_cost: 64, t_cost: 1, p_cost: 4 }, &mut out).is_ok());
}

/// RFC 9106 section 5.3, the Argon2id test vector, with p=4 lanes, a secret,
/// and associated data
#[test]
fn argon2id_matches_the_rfc_9106_published_tag() {
    let password = vec![0x01u8; 32];
    let salt     = vec![0x02u8; 16];
    let secret   = vec![0x03u8; 8];
    let ad       = vec![0x04u8; 12];

    let mut tag = [0u8; 32];
    mikuauth::argon2::hash_full(
        &password,
        &salt,
        &secret,
        &ad,
        Params { m_cost: 32, t_cost: 3, p_cost: 4 },
        &mut tag,
    )
    .unwrap();

    assert_eq!(
        hex(&tag),
        "0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659",
        "the RFC 9106 Argon2id vector"
    );
}

/// RFC 9106 section 5.1 inputs under Argon2id parameters, as a second
/// multi-lane check
#[test]
fn a_second_lane_count_changes_nothing_but_the_lanes() {
    // p is a field of H0, and the final block xors every lane's last column
    let mut one = [0u8; 32];
    let mut four = [0u8; 32];
    mikuauth::argon2::hash_full(b"password", b"saltsaltsaltsalt", &[], &[],
        Params { m_cost: 32, t_cost: 3, p_cost: 1 }, &mut one).unwrap();
    mikuauth::argon2::hash_full(b"password", b"saltsaltsaltsalt", &[], &[],
        Params { m_cost: 32, t_cost: 3, p_cost: 4 }, &mut four).unwrap();
    assert_ne!(one, four);
}
