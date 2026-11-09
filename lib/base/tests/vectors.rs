//! Published check values, and the two calendar directions against each other

use miku_base::{base64, crc32, date, fnv, hex};

#[test]
fn crc32_check_value() {
    assert_eq!(crc32::crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32::crc32(b""), 0);
    assert_eq!(crc32::update(crc32::crc32(b"1234"), b"56789"), 0xCBF4_3926);
}

#[test]
fn fnv1a_published_values() {
    assert_eq!(fnv::fnv1a_32(b""), 0x811c_9dc5);
    assert_eq!(fnv::fnv1a_32(b"a"), 0xe40c_292c);
    assert_eq!(fnv::fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv::fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
}

#[test]
fn calendar_known_days() {
    assert_eq!(date::days_from_civil(1970, 1, 1), 0);
    assert_eq!(date::days_from_civil(2000, 3, 1), 11017);
    assert_eq!(date::days_from_civil(1969, 12, 31), -1);
    assert_eq!(date::civil_from_days(0), (1970, 1, 1));
    assert_eq!(date::civil_from_days(-1), (1969, 12, 31));
    assert_eq!(date::civil_from_days(11017), (2000, 3, 1));
}

#[test]
fn calendar_round_trips_across_centuries() {
    let mut z = date::days_from_civil(1600, 1, 1);
    let end = date::days_from_civil(2500, 1, 1);
    while z < end {
        let (y, m, d) = date::civil_from_days(z);
        assert_eq!(date::days_from_civil(y, m, d), z);
        z += 37;
    }
}

#[test]
fn leap_years() {
    assert!(date::is_leap(2000));
    assert!(date::is_leap(2024));
    assert!(!date::is_leap(1900));
    assert!(!date::is_leap(2023));
}

// RFC 4648 section 10
const RFC: &[(&[u8], &[u8])] = &[
    (b"", b""),
    (b"f", b"Zg=="),
    (b"fo", b"Zm8="),
    (b"foo", b"Zm9v"),
    (b"foob", b"Zm9vYg=="),
    (b"fooba", b"Zm9vYmE="),
    (b"foobar", b"Zm9vYmFy"),
];

#[test]
fn base64_padded() {
    for (plain, text) in RFC {
        let mut out = [0u8; 16];
        let n = base64::encode(plain, &mut out).unwrap();
        assert_eq!(&out[..n], *text);
        let n = base64::decode(text, &mut out).unwrap();
        assert_eq!(&out[..n], *plain);
    }
    assert!(base64::decode(b"Zg=", &mut [0u8; 8]).is_none());
    assert!(base64::decode(b"Z=g=", &mut [0u8; 8]).is_none());
    assert!(base64::decode(b"Zm9v", &mut [0u8; 2]).is_none());
}

#[test]
fn base64_unpadded() {
    for (plain, text) in RFC {
        let stripped: Vec<u8> = text.iter().copied().filter(|&c| c != b'=').collect();
        let mut out = [0u8; 16];
        let n = base64::encode_unpadded(plain, &mut out).unwrap();
        assert_eq!(&out[..n], &stripped[..]);
        let n = base64::decode_unpadded(&stripped, &mut out).unwrap();
        assert_eq!(&out[..n], *plain);
    }
    assert!(base64::decode_unpadded(b"Z", &mut [0u8; 8]).is_none());
    assert!(base64::decode_unpadded(b"Z!", &mut [0u8; 8]).is_none());
}

#[test]
fn base64_pem_lines() {
    let mut out = Vec::new();
    assert!(base64::decode_lines(b"Zm9v\nYmE=\n", |b| out.push(b)));
    assert_eq!(out, b"fooba");
    assert!(!base64::decode_lines(b"Zm9v!", |_| {}));
    assert!(!base64::decode_lines(b"Zg==Zg", |_| {}));
}

#[test]
fn hex_digits() {
    assert_eq!(hex::value(b'0'), Some(0));
    assert_eq!(hex::value(b'f'), Some(15));
    assert_eq!(hex::value(b'F'), Some(15));
    assert_eq!(hex::value(b'g'), None);
    assert_eq!(hex::digit(10), b'a');
}
