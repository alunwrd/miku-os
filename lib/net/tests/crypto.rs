//! Primitives against published vectors

use mikunet::crypto::*;

fn hex(bytes: &[u8]) -> String {
    let mut s = String::new();
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[test]
fn sha256_vectors() {
    // FIPS 180-4 examples
    assert_eq!(
        hex(&sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex(&sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hex(&sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
}

#[test]
fn sha256_crosses_its_block_boundary() {
    // A million 'a': the length counter and padding of a message that is not a
    // whole number of blocks
    let data = vec![b'a'; 1_000_000];
    assert_eq!(
        hex(&sha256(&data)),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

#[test]
fn sha384_and_sha512_vectors() {
    assert_eq!(
        hex(&sha384(b"abc")),
        "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed\
         8086072ba1e7cc2358baeca134c825a7"
    );
    assert_eq!(
        hex(&sha512(b"abc")),
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
         2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    );
    assert_eq!(
        hex(&sha512(b"")),
        "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
         47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
    );
}

#[test]
fn sha1_vectors() {
    assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
}

#[test]
fn hmac_sha256_vectors() {
    // RFC 4231 test case 1
    let key = [0x0bu8; 20];
    assert_eq!(
        hex(&hmac_sha256(&key, b"Hi There")),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    // Case 2: key shorter than the block, text not
    assert_eq!(
        hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
}

#[test]
fn hmac_sha256_hashes_an_oversized_key() {
    // A key longer than the block is hashed first
    let key = [0xaau8; 131];
    assert_eq!(
        hex(&hmac_sha256(&key, b"Test Using Larger Than Block-Size Key - Hash Key First")),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
}

#[test]
fn aes128_ecb_block() {
    // FIPS 197 appendix C.1
    let key = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
        0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    ];
    let mut block = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77,
        0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
    ];
    let aes = Aes128::new(&key);
    aes.encrypt_block(&mut block);
    assert_eq!(hex(&block), "69c4e0d86a7b0430d8cdb78070b4c55a");
}

#[test]
fn cbc_round_trips() {
    let key = [0x2bu8; 16];
    let iv = [0x11u8; 16];
    let plain = b"the quick brown fox jumps over!!"; // exactly two blocks

    let mut ct = [0u8; 64];
    let n = cbc_encrypt(&key, &iv, plain, &mut ct);
    assert_eq!(n, 32);

    let mut back = [0u8; 64];
    let m = cbc_decrypt(&key, &iv, &ct[..n], &mut back);
    assert_eq!(&back[..m], plain);
}

#[test]
fn tls_padding_round_trips_and_rejects_a_lie() {
    for len in 0..40usize {
        let data = vec![0x5au8; len];
        let mut padded = [0u8; 80];
        let n = tls_pad(&data, &mut padded);
        assert_eq!(n % 16, 0, "padded length must be a whole number of blocks");
        let back = tls_unpad(&padded[..n]).expect("well-formed padding");
        assert_eq!(back, &data[..]);
    }

    // A padding byte that claims more than the record holds
    let mut bad = [0u8; 16];
    bad[15] = 200;
    assert!(tls_unpad(&bad).is_none());
}
