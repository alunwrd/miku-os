// Shadow hash string and verification
//
//     $argon2id$v=19$m=65536,t=3,p=1$<salt-b64>$<tag-b64>
//
// Parameters are in the string, so cost can rise without invalidating stored
// hashes.
//
// Other field values:
// ""    no password: logs in without asking; useradd does not write it
// "!"   locked; no password matches
// "*"   the same

use crate::argon2::{self, Argon2Error, Params};
use crate::b64;
use crate::ct;

pub const MAX_HASH_LEN: usize = 256;
pub const TAG_LEN: usize = 32;
pub const SALT_LEN: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashError {
    /// Not in the format
    Malformed,
    /// A recognized format with an unimplemented algorithm
    UnknownAlgorithm,
    /// Parameters outside the limits (argon2::MAX_M_COST)
    BadParams,
    /// Buffer too small
    NoSpace,
    Argon2(Argon2Error),
}

impl From<Argon2Error> for HashError {
    fn from(e: Argon2Error) -> Self { HashError::Argon2(e) }
}

/// What a parsed shadow field turned out to be
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The password matched
    Ok,
    /// Did not match
    Wrong,
    /// No password set. `login` lets them in; mikudod refuses it as a target
    NoPassword,
    /// The account is locked. No password matches
    Locked,
}

/// A parsed `$argon2id$...` string, borrowed from the line it came from
#[derive(Debug, Clone, Copy)]
pub struct Encoded<'a> {
    pub params: Params,
    pub salt_b64: &'a str,
    pub tag_b64: &'a str,
}

/// Parses a stored hash strictly
pub fn parse(s: &str) -> Result<Encoded<'_>, HashError> {
    // Leading empty field from the opening '$', then id, v, params, salt, tag
    let mut it = s.split('$');
    if it.next() != Some("") {
        return Err(HashError::Malformed);
    }
    match it.next() {
        Some("argon2id") => {}
        Some(_) => return Err(HashError::UnknownAlgorithm),
        None => return Err(HashError::Malformed),
    }
    let v = it.next().ok_or(HashError::Malformed)?;
    if v != "v=19" {
        return Err(HashError::UnknownAlgorithm);
    }

    let pfield = it.next().ok_or(HashError::Malformed)?;
    let mut m_cost = None;
    let mut t_cost = None;
    let mut p_cost = None;
    for kv in pfield.split(',') {
        let (k, val) = kv.split_once('=').ok_or(HashError::Malformed)?;
        let n: u32 = parse_u32(val).ok_or(HashError::Malformed)?;
        match k {
            "m" => m_cost = Some(n),
            "t" => t_cost = Some(n),
            "p" => p_cost = Some(n),
            // Unknown parameter is refused, since applying fewer gives a
            // different hash
            _ => return Err(HashError::Malformed),
        }
    }
    let params = Params {
        m_cost: m_cost.ok_or(HashError::Malformed)?,
        t_cost: t_cost.ok_or(HashError::Malformed)?,
        p_cost: p_cost.ok_or(HashError::Malformed)?,
    };
    params.validate().map_err(|_| HashError::BadParams)?;

    let salt_b64 = it.next().ok_or(HashError::Malformed)?;
    let tag_b64  = it.next().ok_or(HashError::Malformed)?;
    if it.next().is_some() {
        return Err(HashError::Malformed);
    }
    if salt_b64.is_empty() || tag_b64.is_empty() {
        return Err(HashError::Malformed);
    }
    Ok(Encoded { params, salt_b64, tag_b64 })
}

fn parse_u32(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 10 {
        return None;
    }
    let mut n: u32 = 0;
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(n)
}

/// Hash no password produces, verified against for unknown accounts so they
/// cost the same. Parameters match `useradd` and `passwd`. Zero tag; the salt
/// names its purpose
pub const DUMMY: &str =
    "$argon2id$v=19$m=19456,t=3,p=1$bm90aGluZ2hlcmVhdGFsbA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// Checks `password` against a stored field. A wrong password and a malformed
/// hash take the same Argon2 path where possible, with a constant-time compare
pub fn verify(password: &[u8], field: &str) -> Verdict {
    if field.is_empty() {
        return Verdict::NoPassword;
    }
    if field == "!" || field == "*" || field.starts_with('!') {
        return Verdict::Locked;
    }
    let enc = match parse(field) {
        Ok(e) => e,
        Err(_) => return Verdict::Wrong,
    };

    let mut salt = [0u8; b64::MAX_DECODED];
    let salt_len = match b64::decode(enc.salt_b64.as_bytes(), &mut salt) {
        Some(n) => n,
        None    => return Verdict::Wrong,
    };
    let mut want = [0u8; b64::MAX_DECODED];
    let want_len = match b64::decode(enc.tag_b64.as_bytes(), &mut want) {
        Some(n) => n,
        None    => return Verdict::Wrong,
    };
    if want_len < 4 || want_len > 64 {
        return Verdict::Wrong;
    }

    let mut got = [0u8; 64];
    if argon2::hash_into(password, &salt[..salt_len], enc.params, &mut got[..want_len]).is_err() {
        return Verdict::Wrong;
    }
    let ok = ct::eq(&got[..want_len], &want[..want_len]);
    ct::wipe(&mut got);
    if ok { Verdict::Ok } else { Verdict::Wrong }
}

/// Produces a stored field. The caller provides the salt from `getrandom`
/// (syscall 78), since this crate has no I/O
pub fn encode_into<'a>(
    password: &[u8],
    salt: &[u8],
    params: Params,
    out: &'a mut [u8],
) -> Result<&'a str, HashError> {
    params.validate().map_err(|_| HashError::BadParams)?;
    let mut tag = [0u8; TAG_LEN];
    argon2::hash_into(password, salt, params, &mut tag)?;

    let mut salt_b64 = [0u8; 128];
    let sn = b64::encode(salt, &mut salt_b64).ok_or(HashError::NoSpace)?;
    let mut tag_b64 = [0u8; 128];
    let tn = b64::encode(&tag, &mut tag_b64).ok_or(HashError::NoSpace)?;

    let mut pos = 0usize;
    let put = |bytes: &[u8], out: &mut [u8], pos: &mut usize| -> Result<(), HashError> {
        if *pos + bytes.len() > out.len() {
            return Err(HashError::NoSpace);
        }
        out[*pos..*pos + bytes.len()].copy_from_slice(bytes);
        *pos += bytes.len();
        Ok(())
    };
    put(b"$argon2id$v=19$m=", out, &mut pos)?;
    let mut num = [0u8; 12];
    put(fmt_u32(params.m_cost, &mut num), out, &mut pos)?;
    put(b",t=", out, &mut pos)?;
    put(fmt_u32(params.t_cost, &mut num), out, &mut pos)?;
    put(b",p=", out, &mut pos)?;
    put(fmt_u32(params.p_cost, &mut num), out, &mut pos)?;
    put(b"$", out, &mut pos)?;
    put(&salt_b64[..sn], out, &mut pos)?;
    put(b"$", out, &mut pos)?;
    put(&tag_b64[..tn], out, &mut pos)?;

    // Every byte written above is ASCII by construction
    Ok(core::str::from_utf8(&out[..pos]).map_err(|_| HashError::Malformed)?)
}

fn fmt_u32(mut v: u32, buf: &mut [u8; 12]) -> &[u8] {
    if v == 0 {
        buf[0] = b'0';
        return &buf[..1];
    }
    let mut i = buf.len();
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    let len = buf.len() - i;
    buf.copy_within(i.., 0);
    &buf[..len]
}
