use super::tls_bignum::{bn_from_bytes_be, bn_to_bytes_be, bn_powmod_u32, BigNum};

pub struct RsaPublicKey {
    pub n:     BigNum,
    pub e:     u32,
    pub n_len: usize,
}

fn read_len(d: &[u8], p: &mut usize) -> Option<usize> {
    if *p >= d.len() { return None; }
    let b = d[*p]; *p += 1;
    if b & 0x80 == 0 { return Some(b as usize); }
    let nb = (b & 0x7f) as usize;
    if nb > 4 || *p + nb > d.len() { return None; }
    let mut l = 0usize;
    for _ in 0..nb { l = (l << 8) | d[*p] as usize; *p += 1; }
    Some(l)
}

pub fn parse_rsa_public_key(cert: &[u8]) -> Option<RsaPublicKey> {
    let rsa_oid = &[0x2au8, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
    let oid_off = cert.windows(rsa_oid.len()).position(|w| w == rsa_oid)?;

    let mut p = oid_off + rsa_oid.len();
    while p < cert.len() && p < oid_off + 64 {
        if cert[p] == 0x03 { break; }
        p += 1;
    }
    if p >= cert.len() { return None; }
    p += 1;
    read_len(cert, &mut p)?;
    if p >= cert.len() || cert[p] != 0x00 { return None; }
    p += 1;

    if p >= cert.len() || cert[p] != 0x30 { return None; }
    p += 1;
    read_len(cert, &mut p)?;

    if p >= cert.len() || cert[p] != 0x02 { return None; }
    p += 1;
    let n_len = read_len(cert, &mut p)?;
    if p + n_len > cert.len() { return None; }
    let (n_bytes, n_actual) = if cert[p] == 0x00 {
        (&cert[p+1..p+n_len], n_len - 1)
    } else {
        (&cert[p..p+n_len], n_len)
    };
    let n = bn_from_bytes_be(n_bytes);
    p += n_len;

    if p >= cert.len() || cert[p] != 0x02 { return None; }
    p += 1;
    let e_len = read_len(cert, &mut p)?;
    if p + e_len > cert.len() { return None; }
    let mut e = 0u32;
    for i in 0..e_len.min(4) { e = (e << 8) | cert[p + i] as u32; }

    Some(RsaPublicKey { n, e, n_len: n_actual })
}

pub fn rsa_pkcs1_encrypt(key: &RsaPublicKey, data: &[u8], out: &mut [u8; 256]) -> usize {
    let k = key.n_len;
    if k > 256 || data.len() + 11 > k { return 0; }

    let mut em = [0u8; 256];
    em[0] = 0x00;
    em[1] = 0x02;
    let ps_len = k - 3 - data.len();

    let mut i = 0;
    while i < ps_len {
        let r = crate::random::random_u64().to_le_bytes();
        for &b in r.iter() {
            if b != 0 && i < ps_len {
                em[2 + i] = b;
                i += 1;
            }
        }
    }

    em[2 + ps_len] = 0x00;
    em[3 + ps_len..3 + ps_len + data.len()].copy_from_slice(data);

    let m = bn_from_bytes_be(&em[..k]);
    let c = bn_powmod_u32(&m, key.e, &key.n);
    bn_to_bytes_be(&c, &mut out[..k]);
    
    k
}
