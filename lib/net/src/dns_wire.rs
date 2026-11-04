// DNS query and answer as bytes
//
// No socket, allocation, or crate. `mikunet::dns` uses it, and `libc_miku.so`
// includes this file by `#[path]` for `getaddrinfo`, used by `std`'s
// `lookup_host` and C programs. Runs under `cargo test` on the build host,
// unlike `dns.rs`.

pub fn build_query(hostname: &str, id: u16, out: &mut [u8; 512]) -> Option<usize> {
    out[0] = (id >> 8) as u8;
    out[1] = id as u8;
    out[2] = 0x01;
    out[3] = 0x00;
    out[4] = 0x00;
    out[5] = 0x01;
    out[6] = 0x00; out[7] = 0x00;
    out[8] = 0x00; out[9] = 0x00;
    out[10] = 0x00; out[11] = 0x00;

    let mut pos = 12usize;

    for label in hostname.split('.') {
        if label.is_empty() { continue; }
        let lb = label.as_bytes();
        if pos + 1 + lb.len() >= 512 { return None; }
        out[pos] = lb.len() as u8;
        pos += 1;
        out[pos..pos + lb.len()].copy_from_slice(lb);
        pos += lb.len();
    }

    if pos + 5 >= 512 { return None; }
    out[pos] = 0x00;
    pos += 1;
    out[pos] = 0x00; out[pos+1] = 0x01;
    pos += 2;
    out[pos] = 0x00; out[pos+1] = 0x01;
    pos += 2;

    Some(pos)
}

/// The address and its TTL, or None if this is not an answer to our query. The
/// transaction id rejects a late reply to an earlier query
pub fn parse_response(buf: &[u8], id: u16) -> Option<([u8; 4], u32)> {
    if buf.len() < 12 { return None; }

    if u16::from_be_bytes([buf[0], buf[1]]) != id { return None; }

    let flags = u16::from_be_bytes([buf[2], buf[3]]);
    if flags & 0x8000 == 0 { return None; }
    if flags & 0x000F != 0 { return None; }

    let qdcount = u16::from_be_bytes([buf[4], buf[5]]) as usize;
    let ancount = u16::from_be_bytes([buf[6], buf[7]]) as usize;

    if ancount == 0 { return None; }

    let mut pos = 12usize;

    for _ in 0..qdcount {
        pos = skip_name(buf, pos)?;
        pos += 4;
        if pos > buf.len() { return None; }
    }

    for _ in 0..ancount {
        pos = skip_name(buf, pos)?;
        if pos + 10 > buf.len() { return None; }

        let rtype = u16::from_be_bytes([buf[pos], buf[pos+1]]);
        let _rclass = u16::from_be_bytes([buf[pos+2], buf[pos+3]]);
        let ttl = u32::from_be_bytes([buf[pos+4], buf[pos+5], buf[pos+6], buf[pos+7]]);
        let rdlen = u16::from_be_bytes([buf[pos+8], buf[pos+9]]) as usize;
        pos += 10;

        if pos + rdlen > buf.len() { return None; }

        if rtype == 1 && rdlen == 4 {
            let mut ip = [0u8; 4];
            ip.copy_from_slice(&buf[pos..pos+4]);
            return Some((ip, ttl));
        }

        pos += rdlen;
    }

    None
}

fn skip_name(buf: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        if pos >= buf.len() { return None; }
        let b = buf[pos];
        if b == 0 {
            return Some(pos + 1);
        }
        if b & 0xC0 == 0xC0 {
            return Some(pos + 2);
        }
        pos += 1 + b as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_is_one_question_for_an_a_record() {
        let mut q = [0u8; 512];
        let n = build_query("zed.dev", 0x1234, &mut q).unwrap();
        assert_eq!(&q[..4], &[0x12, 0x34, 0x01, 0x00]);
        assert_eq!(&q[4..6], &[0, 1]);
        assert_eq!(&q[12..n], b"\x03zed\x03dev\x00\x00\x01\x00\x01");
    }

    /// A CNAME before the A record with a compressed name, as resolvers send it
    fn answer(id: u16, rcode: u8) -> ([u8; 512], usize) {
        let mut b = [0u8; 512];
        let mut q = [0u8; 512];
        let qn = build_query("www.zed.dev", id, &mut q).unwrap();
        b[..qn].copy_from_slice(&q[..qn]);
        b[2] = 0x81;
        b[3] = 0x80 | rcode;
        b[7] = 2; // ancount
        let mut p = qn;
        // www.zed.dev CNAME zed.dev
        b[p..p + 12].copy_from_slice(&[0xC0, 12, 0, 5, 0, 1, 0, 0, 0, 60, 0, 2]);
        p += 12;
        b[p..p + 2].copy_from_slice(&[0xC0, 16]);
        p += 2;
        // zed.dev A 1.2.3.4, ttl 300
        b[p..p + 16].copy_from_slice(&[0xC0, 16, 0, 1, 0, 1, 0, 0, 1, 44, 0, 4, 1, 2, 3, 4]);
        p += 16;
        (b, p)
    }

    #[test]
    fn a_record_after_a_cname() {
        let (b, n) = answer(7, 0);
        assert_eq!(parse_response(&b[..n], 7), Some(([1, 2, 3, 4], 300)));
    }

    #[test]
    fn wrong_id_is_not_an_answer() {
        let (b, n) = answer(7, 0);
        assert_eq!(parse_response(&b[..n], 8), None);
    }

    #[test]
    fn an_error_rcode_is_not_an_answer() {
        let (b, n) = answer(7, 3);
        assert_eq!(parse_response(&b[..n], 7), None);
    }

    #[test]
    fn truncated_answers_are_refused_not_read_past() {
        let (b, n) = answer(7, 0);
        for cut in 0..n {
            assert_eq!(parse_response(&b[..cut], 7), None, "cut at {cut}");
        }
    }
}
