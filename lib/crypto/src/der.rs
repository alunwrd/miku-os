// Minimal DER reader
//
// X.509 is ASN.1 DER: tag, length, content, definite lengths, fixed order, so a
// reader only walks past unwanted fields.
//
// Accessors return None instead of panicking, since the input is an
// unauthenticated peer's. Each element keeps `full`, its bytes including the
// header, because signatures cover the encoding and not the parsed value.

pub const TAG_BOOLEAN:      u8 = 0x01;
pub const TAG_INTEGER:      u8 = 0x02;
pub const TAG_BITSTRING:    u8 = 0x03;
pub const TAG_OCTETSTRING:  u8 = 0x04;
pub const TAG_NULL:         u8 = 0x05;
pub const TAG_OID:          u8 = 0x06;
pub const TAG_UTF8STRING:   u8 = 0x0C;
pub const TAG_SEQUENCE:     u8 = 0x30;
pub const TAG_SET:          u8 = 0x31;
pub const TAG_PRINTABLE:    u8 = 0x13;
pub const TAG_T61STRING:    u8 = 0x14;
pub const TAG_IA5STRING:    u8 = 0x16;
pub const TAG_UTCTIME:      u8 = 0x17;
pub const TAG_GENTIME:      u8 = 0x18;
pub const TAG_BMPSTRING:    u8 = 0x1E;

/// A context-specific constructed tag, e.g. `ctx(0)` is `[0]` = 0xA0
pub const fn ctx(n: u8) -> u8 { 0xA0 | n }
/// A context-specific primitive tag, e.g. `ctx_prim(2)` is dNSName in a SAN
pub const fn ctx_prim(n: u8) -> u8 { 0x80 | n }

#[derive(Clone, Copy)]
pub struct Tlv<'a> {
    pub tag: u8,
    /// The value octets alone
    pub content: &'a [u8],
    /// Tag, length, and value, as a signature covers them
    pub full: &'a [u8],
}

impl<'a> Tlv<'a> {
    /// Read this element's content as a nested reader
    pub fn reader(&self) -> Der<'a> { Der::new(self.content) }

    /// An INTEGER small enough to be a count or a version number
    pub fn as_u32(&self) -> Option<u32> {
        if self.content.is_empty() || self.content.len() > 5 { return None; }
        let mut s = self.content;
        if s[0] == 0 { s = &s[1..]; }
        if s.len() > 4 { return None; }
        let mut v = 0u32;
        for &b in s { v = (v << 8) | b as u32; }
        Some(v)
    }

    /// An unsigned INTEGER's magnitude, with DER's sign byte stripped
    pub fn as_uint(&self) -> Option<&'a [u8]> {
        if self.content.is_empty() { return None; }
        if self.content[0] == 0x00 { Some(&self.content[1..]) } else { Some(self.content) }
    }

    /// Payload of a BIT STRING with no unused bits. Keys and signatures are
    /// whole octets, so anything else is refused
    pub fn as_bitstring(&self) -> Option<&'a [u8]> {
        if self.tag != TAG_BITSTRING || self.content.is_empty() { return None; }
        if self.content[0] != 0 { return None; }
        Some(&self.content[1..])
    }
}

pub struct Der<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Der<'a> {
    pub fn new(buf: &'a [u8]) -> Self { Self { buf, pos: 0 } }

    pub fn is_empty(&self) -> bool { self.pos >= self.buf.len() }
    pub fn rest(&self) -> &'a [u8] { &self.buf[self.pos.min(self.buf.len())..] }

    /// Next tag without consuming it. ASN.1 marks optional fields by tag
    pub fn peek_tag(&self) -> Option<u8> {
        self.buf.get(self.pos).copied()
    }

    pub fn read(&mut self) -> Option<Tlv<'a>> {
        let start = self.pos;
        let tag = *self.buf.get(self.pos)?;
        self.pos += 1;
        let first = *self.buf.get(self.pos)?;
        self.pos += 1;
        let len = if first & 0x80 == 0 {
            first as usize
        } else {
            let nb = (first & 0x7F) as usize;
            // 0x80 is BER's indefinite length, forbidden in DER; finding its
            // end would mean scanning for a terminator
            if nb == 0 || nb > 4 { return None; }
            let mut l = 0usize;
            for _ in 0..nb {
                l = (l << 8) | *self.buf.get(self.pos)? as usize;
                self.pos += 1;
            }
            l
        };
        if len > self.buf.len() - self.pos { return None; }
        let content = &self.buf[self.pos..self.pos + len];
        self.pos += len;
        Some(Tlv { tag, content, full: &self.buf[start..self.pos] })
    }

    /// Read the next element, requiring it to carry `tag`
    pub fn expect(&mut self, tag: u8) -> Option<Tlv<'a>> {
        if self.peek_tag()? != tag { return None; }
        self.read()
    }

    /// Next element only if it carries `tag`; otherwise the reader does not
    /// move
    pub fn maybe(&mut self, tag: u8) -> Option<Tlv<'a>> {
        if self.peek_tag()? != tag { return None; }
        self.read()
    }

    /// Descend into a constructed element
    pub fn enter(&mut self, tag: u8) -> Option<Der<'a>> {
        Some(self.expect(tag)?.reader())
    }

    pub fn skip(&mut self) -> Option<()> { self.read().map(|_| ()) }
}

/// AlgorithmIdentifier: its OID and parameters, if present. RSA writes an
/// explicit NULL, ECDSA nothing, RSASSA-PSS a nested structure
pub fn algorithm_id<'a>(seq: &Tlv<'a>) -> Option<(&'a [u8], Option<Tlv<'a>>)> {
    if seq.tag != TAG_SEQUENCE { return None; }
    let mut r = seq.reader();
    let oid = r.expect(TAG_OID)?;
    let params = r.read();
    Some((oid.content, params))
}
