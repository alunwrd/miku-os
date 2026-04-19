extern crate alloc;
use alloc::vec::Vec;
use alloc::string::String;
use core::sync::atomic::Ordering;
use super::CTRL_C;

const CLIENT_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

const FT_DATA:          u8 = 0x0;
const FT_HEADERS:       u8 = 0x1;
const FT_RST_STREAM:    u8 = 0x3;
const FT_SETTINGS:      u8 = 0x4;
const FT_PING:          u8 = 0x6;
const FT_GOAWAY:        u8 = 0x7;
const FT_WINDOW_UPDATE: u8 = 0x8;
const FT_CONTINUATION:  u8 = 0x9;

const FL_END_STREAM:  u8 = 0x1;
const FL_END_HEADERS: u8 = 0x4;
const FL_ACK:         u8 = 0x1;

const SETTINGS_HEADER_TABLE_SIZE:      u16 = 0x1;
const SETTINGS_ENABLE_PUSH:            u16 = 0x2;
const SETTINGS_MAX_CONCURRENT_STREAMS: u16 = 0x3;
const SETTINGS_INITIAL_WINDOW_SIZE:    u16 = 0x4;
const SETTINGS_MAX_FRAME_SIZE:         u16 = 0x5;

const INIT_WINDOW: u32 = 65535;
const MAX_FRAME:   u32 = 16384;

static HPACK_STATIC: &[(&[u8], &[u8])] = &[
    (b":authority",                  b""),
    (b":method",                     b"GET"),
    (b":method",                     b"POST"),
    (b":path",                       b"/"),
    (b":path",                       b"/index.html"),
    (b":scheme",                     b"http"),
    (b":scheme",                     b"https"),
    (b":status",                     b"200"),
    (b":status",                     b"204"),
    (b":status",                     b"206"),
    (b":status",                     b"304"),
    (b":status",                     b"400"),
    (b":status",                     b"404"),
    (b":status",                     b"500"),
    (b"accept-charset",              b""),
    (b"accept-encoding",             b"gzip, deflate"),
    (b"accept-language",             b""),
    (b"accept-ranges",               b""),
    (b"accept",                      b""),
    (b"access-control-allow-origin", b""),
    (b"age",                         b""),
    (b"allow",                       b""),
    (b"authorization",               b""),
    (b"cache-control",               b""),
    (b"content-disposition",         b""),
    (b"content-encoding",            b""),
    (b"content-language",            b""),
    (b"content-length",              b""),
    (b"content-location",            b""),
    (b"content-range",               b""),
    (b"content-type",                b""),
    (b"cookie",                      b""),
    (b"date",                        b""),
    (b"etag",                        b""),
    (b"expect",                      b""),
    (b"expires",                     b""),
    (b"from",                        b""),
    (b"host",                        b""),
    (b"if-match",                    b""),
    (b"if-modified-since",           b""),
    (b"if-none-match",               b""),
    (b"if-range",                    b""),
    (b"if-unmodified-since",         b""),
    (b"last-modified",               b""),
    (b"link",                        b""),
    (b"location",                    b""),
    (b"max-forwards",                b""),
    (b"proxy-authenticate",          b""),
    (b"proxy-authorization",         b""),
    (b"range",                       b""),
    (b"referer",                     b""),
    (b"refresh",                     b""),
    (b"retry-after",                 b""),
    (b"server",                      b""),
    (b"set-cookie",                  b""),
    (b"strict-transport-security",   b""),
    (b"transfer-encoding",           b""),
    (b"user-agent",                  b""),
    (b"vary",                        b""),
    (b"via",                         b""),
    (b"www-authenticate",            b""),
];

fn hpack_static_index(name: &[u8], value: &[u8]) -> Option<usize> {
    HPACK_STATIC.iter().enumerate()
        .find(|(_, &(n, v))| n == name && v == value)
        .map(|(i, _)| i + 1)
}

fn hpack_static_name_index(name: &[u8]) -> Option<usize> {
    HPACK_STATIC.iter().enumerate()
        .find(|(_, &(n, _))| n == name)
        .map(|(i, _)| i + 1)
}

fn hpack_encode_int(prefix_bits: u8, value: usize, buf: &mut Vec<u8>, first_byte_prefix: u8) {
    let max = (1usize << prefix_bits) - 1;
    if value < max {
        buf.push(first_byte_prefix | value as u8);
    } else {
        buf.push(first_byte_prefix | max as u8);
        let mut v = value - max;
        loop {
            if v < 128 { buf.push(v as u8); break; }
            buf.push((v & 0x7F) as u8 | 0x80);
            v >>= 7;
        }
    }
}

fn hpack_encode_literal(buf: &mut Vec<u8>, s: &[u8]) {
    hpack_encode_int(7, s.len(), buf, 0x00);
    buf.extend_from_slice(s);
}

fn hpack_encode_header(buf: &mut Vec<u8>, name: &[u8], value: &[u8]) {
    if let Some(idx) = hpack_static_index(name, value) {
        buf.push(0x80 | idx as u8);
        return;
    }
    if let Some(idx) = hpack_static_name_index(name) {
        hpack_encode_int(4, idx, buf, 0x00);
    } else {
        buf.push(0x00);
        hpack_encode_literal(buf, name);
    }
    hpack_encode_literal(buf, value);
}

pub fn hpack_encode_request(method: &str, scheme: &str, authority: &str, path: &str) -> Vec<u8> {
    let mut out = Vec::new();
    hpack_encode_header(&mut out, b":method",    method.as_bytes());
    hpack_encode_header(&mut out, b":scheme",    scheme.as_bytes());
    hpack_encode_header(&mut out, b":path",      path.as_bytes());
    hpack_encode_header(&mut out, b":authority", authority.as_bytes());
    hpack_encode_header(&mut out, b"user-agent", b"MikuOS/0.1");
    hpack_encode_header(&mut out, b"accept",     b"*/*");
    out
}

fn hpack_decode_int(data: &[u8], pos: &mut usize, prefix_bits: u8) -> usize {
    if *pos >= data.len() { return 0; }
    let mask  = (1u8 << prefix_bits) - 1;
    let first = (data[*pos] & mask) as usize;
    *pos += 1;
    if first < mask as usize { return first; }
    let mut value = first;
    let mut shift = 0usize;
    while *pos < data.len() {
        let b = data[*pos]; *pos += 1;
        value += ((b & 0x7F) as usize) << shift;
        shift += 7;
        if b & 0x80 == 0 { break; }
    }
    value
}

fn hpack_decode_string(data: &[u8], pos: &mut usize) -> Vec<u8> {
    if *pos >= data.len() { return Vec::new(); }
    let huffman = data[*pos] & 0x80 != 0;
    let len     = hpack_decode_int(data, pos, 7);
    if *pos + len > data.len() { return Vec::new(); }
    let s = &data[*pos..*pos + len];
    *pos += len;
    if huffman { huffman_decode(s) } else { Vec::from(s) }
}

fn decode_headers_block(block: &[u8]) -> (u16, Option<String>) {
    let mut pos      = 0usize;
    let mut status   = 0u16;
    let mut location = None;

    while pos < block.len() {
        let b = block[pos];

        let (name, value) = if b & 0x80 != 0 {
            let idx = hpack_decode_int(block, &mut pos, 7);
            if idx == 0 || idx > HPACK_STATIC.len() { continue; }
            (Vec::from(HPACK_STATIC[idx-1].0), Vec::from(HPACK_STATIC[idx-1].1))
        } else if b & 0x40 != 0 {
            let idx = hpack_decode_int(block, &mut pos, 6);
            let n = if idx > 0 && idx <= HPACK_STATIC.len() {
                Vec::from(HPACK_STATIC[idx-1].0)
            } else {
                hpack_decode_string(block, &mut pos)
            };
            let v = hpack_decode_string(block, &mut pos);
            (n, v)
        } else if b & 0x20 != 0 {
            hpack_decode_int(block, &mut pos, 5);
            continue;
        } else {
            let idx = hpack_decode_int(block, &mut pos, 4);
            let n = if idx > 0 && idx <= HPACK_STATIC.len() {
                Vec::from(HPACK_STATIC[idx-1].0)
            } else {
                hpack_decode_string(block, &mut pos)
            };
            let v = hpack_decode_string(block, &mut pos);
            (n, v)
        };

        if name == b":status" {
            status = core::str::from_utf8(&value).ok()
                .and_then(|s| s.parse().ok()).unwrap_or(0);
        } else if name == b"location" {
            if let Ok(s) = core::str::from_utf8(&value) {
                location = Some(String::from(s));
            }
        }
    }
    (status, location)
}

fn make_frame(ft: u8, flags: u8, sid: u32, payload: &[u8], out: &mut Vec<u8>) {
    let len = payload.len();
    out.push((len >> 16) as u8);
    out.push((len >> 8)  as u8);
    out.push(len         as u8);
    out.push(ft);
    out.push(flags);
    out.push(((sid >> 24) & 0x7F) as u8);
    out.push((sid >> 16) as u8);
    out.push((sid >> 8)  as u8);
    out.push(sid         as u8);
    out.extend_from_slice(payload);
}

fn settings_frame(settings: &[(u16, u32)]) -> Vec<u8> {
    let mut payload = Vec::new();
    for &(id, val) in settings {
        payload.push((id  >> 8) as u8); payload.push(id  as u8);
        payload.push((val >> 24) as u8); payload.push((val >> 16) as u8);
        payload.push((val >> 8)  as u8); payload.push(val         as u8);
    }
    let mut out = Vec::new();
    make_frame(FT_SETTINGS, 0, 0, &payload, &mut out);
    out
}

fn settings_ack() -> Vec<u8> {
    let mut out = Vec::new();
    make_frame(FT_SETTINGS, FL_ACK, 0, &[], &mut out);
    out
}

fn window_update(sid: u32, inc: u32) -> Vec<u8> {
    let p = [(( inc >> 24) & 0x7F) as u8, (inc >> 16) as u8, (inc >> 8) as u8, inc as u8];
    let mut out = Vec::new();
    make_frame(FT_WINDOW_UPDATE, 0, sid, &p, &mut out);
    out
}

pub struct H2Response {
    pub status:   u16,
    pub body:     Vec<u8>,
    pub location: Option<String>,
}

pub fn h2_request(
    tls:    &mut super::tls::TlsStream,
    method: &str,
    scheme: &str,
    host:   &str,
    path:   &str,
    body:   Option<&[u8]>,
) -> Option<H2Response> {
    if CTRL_C.load(Ordering::SeqCst) { return None; }

    tls.send(CLIENT_PREFACE);
    tls.send(&settings_frame(&[
        (SETTINGS_HEADER_TABLE_SIZE,      4096),
        (SETTINGS_ENABLE_PUSH,            0),
        (SETTINGS_MAX_CONCURRENT_STREAMS, 100),
        (SETTINGS_INITIAL_WINDOW_SIZE,    INIT_WINDOW),
        (SETTINGS_MAX_FRAME_SIZE,         MAX_FRAME),
    ]));
    tls.send(&window_update(0, 1 << 20));

    let hpack = hpack_encode_request(method, scheme, host, path);
    let es    = if body.is_none() { FL_END_STREAM | FL_END_HEADERS } else { FL_END_HEADERS };
    let mut hf = Vec::new();
    make_frame(FT_HEADERS, es, 1, &hpack, &mut hf);
    tls.send(&hf);

    if let Some(b) = body {
        let mut df = Vec::new();
        make_frame(FT_DATA, FL_END_STREAM, 1, b, &mut df);
        tls.send(&df);
    }

    let mut buf:           Vec<u8>      = Vec::new();
    let mut status         = 0u16;
    let mut body_out:      Vec<u8>      = Vec::new();
    let mut location:      Option<String> = None;
    let mut headers_block: Vec<u8>      = Vec::new();
    let mut got_headers    = false;
    let mut done           = false;
    let mut idle           = 0usize;

    loop {
        if CTRL_C.load(Ordering::SeqCst) { return None; }
        if done { break; }

        let chunk = tls.recv_chunk(500);
        if chunk.is_empty() {
            if tls.is_closed() { break; }
            idle += 1;
            if idle > 200_000 { break; }
            continue;
        }
        idle = 0;
        buf.extend_from_slice(chunk);

        let mut pos = 0usize;
        loop {
            if pos + 9 > buf.len() { break; }
            let len = ((buf[pos] as usize) << 16)
                    | ((buf[pos+1] as usize) << 8)
                    |  (buf[pos+2] as usize);
            if pos + 9 + len > buf.len() { break; }

            let ftype     = buf[pos+3];
            let flags     = buf[pos+4];
            let stream_id = (((buf[pos+5] & 0x7F) as u32) << 24)
                          | ((buf[pos+6] as u32) << 16)
                          | ((buf[pos+7] as u32) << 8)
                          |  (buf[pos+8] as u32);
            let payload   = &buf[pos+9..pos+9+len].to_vec();
            pos          += 9 + len;

            match ftype {
                FT_SETTINGS if flags & FL_ACK == 0 => {
                    tls.send(&settings_ack());
                    crate::log!("h2: SETTINGS ack sent");
                }
                FT_HEADERS if stream_id == 1 => {
                    headers_block.extend_from_slice(payload);
                    if flags & FL_END_HEADERS != 0 {
                        let (s, loc) = decode_headers_block(&headers_block);
                        status       = s;
                        location     = loc;
                        headers_block.clear();
                        got_headers  = true;
                        crate::log!("h2: status={}", status);
                    }
                    if flags & FL_END_STREAM != 0 { done = true; }
                }
                FT_CONTINUATION if stream_id == 1 => {
                    headers_block.extend_from_slice(payload);
                    if flags & FL_END_HEADERS != 0 {
                        let (s, loc) = decode_headers_block(&headers_block);
                        status       = s;
                        location     = loc;
                        headers_block.clear();
                        got_headers  = true;
                    }
                }
                FT_DATA if stream_id == 1 => {
                    if flags & 0x08 != 0 && !payload.is_empty() {
                        let pad = payload[0] as usize;
                        let end = payload.len().saturating_sub(pad);
                        if end > 1 { body_out.extend_from_slice(&payload[1..end]); }
                    } else {
                        body_out.extend_from_slice(payload);
                    }
                    if flags & FL_END_STREAM != 0 { done = true; }
                }
                FT_RST_STREAM => {
                    let code = if payload.len() >= 4 {
                        u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]])
                    } else { 0 };
                    crate::log_err!("h2: RST_STREAM code={}", code);
                    done = true;
                }
                FT_GOAWAY => {
                    let code = if payload.len() >= 8 {
                        u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]])
                    } else { 0 };
                    crate::log_err!("h2: GOAWAY code={}", code);
                    done = true;
                }
                FT_PING if flags & FL_ACK == 0 => {
                    let mut pf = Vec::new();
                    make_frame(FT_PING, FL_ACK, 0, payload, &mut pf);
                    tls.send(&pf);
                }
                FT_WINDOW_UPDATE => {}
                _ => {}
            }
        }

        if pos > 0 {
            buf.drain(..pos);
        }
    }

    if !got_headers { return None; }
    Some(H2Response { status, body: body_out, location })
}

fn huffman_decode(src: &[u8]) -> Vec<u8> {
    static TABLE: &[(u32, u8, u8)] = &[
        (0x1ff8,13,0),(0x7fffd8,23,1),(0xfffffe2,28,2),(0xfffffe3,28,3),
        (0xfffffe4,28,4),(0xfffffe5,28,5),(0xfffffe6,28,6),(0xfffffe7,28,7),
        (0xfffffe8,28,8),(0xffffea,24,9),(0x3ffffffc,30,10),(0xfffffe9,28,11),
        (0xfffffea,28,12),(0x3ffffffd,30,13),(0xfffffeb,28,14),(0xfffffec,28,15),
        (0xfffffed,28,16),(0xfffffee,28,17),(0xfffffef,28,18),(0xffffff0,28,19),
        (0xffffff1,28,20),(0xffffff2,28,21),(0x3ffffffe,30,22),(0xffffff3,28,23),
        (0xffffff4,28,24),(0xffffff5,28,25),(0xffffff6,28,26),(0xffffff7,28,27),
        (0xffffff8,28,28),(0xffffff9,28,29),(0xffffffa,28,30),(0xffffffb,28,31),
        (0x14,6,b' '),(0x3f8,10,b'!'),(0x3f9,10,b'"'),(0xffa,12,b'#'),
        (0x1ff9,13,b'$'),(0x15,6,b'%'),(0xf8,8,b'&'),(0x7fa,11,b'\''),
        (0x3fa,10,b'('),(0x3fb,10,b')'),(0xf9,8,b'*'),(0x7fb,11,b'+'),
        (0xfa,8,b','),(0x16,6,b'-'),(0x17,6,b'.'),(0x18,6,b'/'),
        (0x0,5,b'0'),(0x1,5,b'1'),(0x2,5,b'2'),(0x19,6,b'3'),
        (0x1a,6,b'4'),(0x1b,6,b'5'),(0x1c,6,b'6'),(0x1d,6,b'7'),
        (0x1e,6,b'8'),(0x1f,6,b'9'),(0x5c,7,b':'),(0xfb,8,b';'),
        (0x7ffc,15,b'<'),(0x20,7,b'='),(0xffb,12,b'>'),(0x3fc,10,b'?'),
        (0x1ffa,13,b'@'),(0x21,7,b'A'),(0x5d,7,b'B'),(0x5e,7,b'C'),
        (0x5f,7,b'D'),(0x60,7,b'E'),(0x61,7,b'F'),(0x62,7,b'G'),
        (0x63,7,b'H'),(0x64,7,b'I'),(0x65,7,b'J'),(0x66,7,b'K'),
        (0x67,7,b'L'),(0x68,7,b'M'),(0x69,7,b'N'),(0x6a,7,b'O'),
        (0x6b,7,b'P'),(0x6c,7,b'Q'),(0x6d,7,b'R'),(0x6e,7,b'S'),
        (0x6f,7,b'T'),(0x70,7,b'U'),(0x71,7,b'V'),(0x72,7,b'W'),
        (0xfc,8,b'X'),(0x73,7,b'Y'),(0xfd,8,b'Z'),(0x1ffb,13,b'['),
        (0x7fff0,19,b'\\'),(0x1ffc,13,b']'),(0x3ffc,14,b'^'),(0x22,7,b'_'),
        (0x7ffd,15,b'`'),(0x3,5,b'a'),(0x23,6,b'b'),(0x4,5,b'c'),
        (0x24,6,b'd'),(0x5,5,b'e'),(0x25,6,b'f'),(0x26,6,b'g'),
        (0x27,6,b'h'),(0x6,5,b'i'),(0x74,7,b'j'),(0x75,7,b'k'),
        (0x28,6,b'l'),(0x29,6,b'm'),(0x2a,6,b'n'),(0x7,5,b'o'),
        (0x2b,6,b'p'),(0x76,7,b'q'),(0x2c,6,b'r'),(0x8,5,b's'),
        (0x9,5,b't'),(0x2d,6,b'u'),(0x77,7,b'v'),(0x2e,6,b'w'),
        (0x78,7,b'x'),(0x2f,6,b'y'),(0x79,7,b'z'),(0x3ffd,14,b'{'),
        (0x7fff1,19,b'|'),(0x3ffe,14,b'}'),(0x7fff2,19,b'~'),(0x3fffd,22,127),
        (0x1fffe6,21,128),(0x3fffe9,22,129),(0x3fffea,22,130),(0x3fffeb,22,131),
        (0x1fffee,21,132),(0x3fffec,22,133),(0x3fffed,22,134),(0x3fffee,22,135),
        (0x77ffef,23,136),(0x3fffef,22,137),(0x3ffff0,22,138),(0x3ffff1,22,139),
        (0x3ffff2,22,140),(0x7ffff0,23,141),(0x3ffff3,22,142),(0x7ffff1,23,143),
        (0x7ffff2,23,144),(0x3ffff4,22,145),(0x3ffff5,22,146),(0x7ffff3,23,147),
        (0x3ffff6,22,148),(0x7ffff4,23,149),(0x7ffff5,23,150),(0x7ffff6,23,151),
        (0x7ffff7,23,152),(0x7ffff8,23,153),(0x7ffff9,23,154),(0x7ffffa,23,155),
        (0x7ffffb,23,156),(0x7ffffc,23,157),(0x7ffffd,23,158),(0x3ffff7,22,159),
        (0x7ffffe,23,160),(0x3ffff8,22,161),(0x1fffef,21,162),(0x3ffff9,22,163),
        (0x1ffff0,21,164),(0x3ffffa,22,165),(0x3ffffb,22,166),(0x1ffff1,21,167),
        (0x3ffffc,22,168),(0x3ffffd,22,169),(0x3ffffe,22,170),(0x3fffff,22,171),
        (0x1ffff2,21,172),(0x1ffff3,21,173),(0x1ffff4,21,174),(0x1ffff5,21,175),
        (0x3fffff,22,176),(0x7fffff,23,177),(0xffffff,24,178),(0xfffffc,24,179),
        (0xfffffd,24,180),(0xfffffe,24,181),(0xffffff,24,182),(0x1fffff,25,183),
        (0x3fffff,26,184),(0x7fffff,27,185),(0xffffff,28,186),(0xfffffff,28,187),
        (0x3ffffff,26,188),(0x7fffffc,27,189),(0x7fffffd,27,190),(0xfffffe,28,191),
        (0xffffff0,28,192),(0x1ffffed,25,193),(0xffffff1,28,194),(0xffffff2,28,195),
        (0xffffff3,28,196),(0xffffff4,28,197),(0xffffff5,28,198),(0xffffff6,28,199),
        (0xffffff7,28,200),(0xffffff8,28,201),(0xffffff9,28,202),(0xffffffa,28,203),
        (0xffffffb,28,204),(0xffffffc,28,205),(0xffffffd,28,206),(0xffffffe,28,207),
        (0xfffffff,28,208),(0x7fffffe,27,209),(0x7ffffff,27,210),(0x3ffffff,26,211),
        (0x1ffffff,25,212),(0x3ffffff,26,213),(0x7ffffff,27,214),(0xfffffff,28,215),
        (0x3ffffff,26,216),(0x7ffffff,27,217),(0xfffffff,28,218),(0xfffffff,28,219),
        (0xfffffff,28,220),(0xfffffff,28,221),(0xfffffff,28,222),(0x7ffffff,27,223),
        (0xfffffff,28,224),(0x1ffffff,25,225),(0xfffffff,28,226),(0xfffffff,28,227),
        (0xfffffff,28,228),(0xfffffff,28,229),(0xfffffff,28,230),(0xfffffff,28,231),
        (0xfffffff,28,232),(0xfffffff,28,233),(0xfffffff,28,234),(0xfffffff,28,235),
        (0xfffffff,28,236),(0xfffffff,28,237),(0xfffffff,28,238),(0xfffffff,28,239),
        (0xfffffff,28,240),(0xfffffff,28,241),(0xfffffff,28,242),(0xfffffff,28,243),
        (0xfffffff,28,244),(0xfffffff,28,245),(0xfffffff,28,246),(0xfffffff,28,247),
        (0xfffffff,28,248),(0xfffffff,28,249),(0xfffffff,28,250),(0xfffffff,28,251),
        (0xfffffff,28,252),(0xfffffff,28,253),(0xfffffff,28,254),(0xfffffff,28,255),
    ];

    let mut bits: u64  = 0;
    let mut nbits: u32 = 0;
    let mut out        = Vec::new();

    for &byte in src {
        bits   = (bits << 8) | byte as u64;
        nbits += 8;
        'outer: loop {
            for &(code, len, ch) in TABLE.iter() {
                if nbits >= len as u32 {
                    let shift = nbits - len as u32;
                    if shift >= 64 { continue; }
                    if (bits >> shift) as u32 == code {
                        out.push(ch);
                        bits  = if shift == 0 { 0 } else { bits & ((1u64 << shift) - 1) };
                        nbits -= len as u32;
                        continue 'outer;
                    }
                }
            }
            break;
        }
    }
    out
}
