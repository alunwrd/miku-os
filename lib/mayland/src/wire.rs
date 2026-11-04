// Mayland wire format, shared by the kernel and clients
//
// Compiled into both sides: the kernel includes it from kernel/mayland/mod.rs,
// and a client with #[path].
//
// Byte-for-byte the Wayland wire format. A message is
//
//      u32  sender object id
//      u32  (size << 16) | opcode          size counts the 8-byte header
//      ...  arguments, each 4-byte aligned
//
// Arguments (matching 'enum wl_arg_type' in wayland-private.h):
//
//      i   int32
//      u   uint32
//      f   fixed        24.8 signed fixed point, stored as int32
//      o   object       object id, 0 means null
//      n   new_id       object id the sender is creating
//      s   string       u32 length INCLUDING the trailing NUL, then the
//                       bytes, zero-padded to a multiple of 4. Length 0
//                       is a null string
//      a   array        u32 byte count, then the bytes, padded to 4
//
// Wayland's 'h' (file descriptor) argument is absent: shared memory is named by
// a plain u32 handle in the stream.
//
// No allocation: clients have no `alloc`, and a message goes into a caller's
// buffer

#![allow(dead_code)]

/// Every message starts with two words: sender id, then size and opcode
pub const HEADER_LEN: usize = 8;

/// Largest message, header included. Same as libwayland's
/// WL_BUFFER_DEFAULT_MAX_SIZE
pub const MAX_MESSAGE_SIZE: usize = 4096;

/// 24.8 signed fixed point, as Wayland's wl_fixed_t
pub type Fixed = i32;

#[inline]
pub fn fixed_from_int(i: i32) -> Fixed { i << 8 }

#[inline]
pub fn fixed_to_int(f: Fixed) -> i32 { f >> 8 }

/// The same for non-integers, so an opacity of 0.85 in a config is 217 on the
/// wire and 0.848 on read
#[inline]
pub fn fixed_from_f32(v: f32) -> Fixed { (v * 256.0) as i32 }

#[inline]
pub fn fixed_to_f32(f: Fixed) -> f32 { f as f32 / 256.0 }

/// Round 'n' up to the next multiple of 4
#[inline]
pub const fn pad4(n: usize) -> usize { (n + 3) & !3 }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WireError {
    /// The buffer ended in the middle of a message or an argument
    Truncated,
    /// The size field is smaller than the header or larger than the cap
    BadSize,
    /// A string did not end in NUL, or had one in the middle
    BadString,
    /// The message would be longer than MAX_MESSAGE_SIZE once encoded
    TooLarge,
    /// The destination buffer is too small
    NoRoom,
}

// Reading

/// Header of one message, and the length of the whole
#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub sender: u32,
    pub opcode: u16,
    /// Total message length, header included
    pub size: usize,
}

/// Peeks at the header at the front of 'buf'. Ok(None) when 'buf' lacks a whole
/// message, the normal state of a stream
pub fn peek_header(buf: &[u8]) -> Result<Option<Header>, WireError> {
    if buf.len() < HEADER_LEN {
        return Ok(None);
    }
    let sender = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let word1 = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let size = (word1 >> 16) as usize;
    let opcode = (word1 & 0xffff) as u16;

    if size < HEADER_LEN || size > MAX_MESSAGE_SIZE || size % 4 != 0 {
        return Err(WireError::BadSize);
    }
    if buf.len() < size {
        return Ok(None);
    }
    Ok(Some(Header { sender, opcode, size }))
}

/// Pulls typed arguments out of a message body
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// 'body' is the message with its 8-byte header already stripped
    pub fn new(body: &'a [u8]) -> Self {
        Reader { buf: body, pos: 0 }
    }

    #[inline]
    pub fn remaining(&self) -> usize { self.buf.len() - self.pos }

    /// True once every argument is consumed. A handler that decodes its
    /// signature and finds this false has a malformed message
    #[inline]
    pub fn is_empty(&self) -> bool { self.pos >= self.buf.len() }

    fn word(&mut self) -> Result<u32, WireError> {
        if self.remaining() < 4 {
            return Err(WireError::Truncated);
        }
        let p = self.pos;
        self.pos += 4;
        Ok(u32::from_le_bytes([
            self.buf[p], self.buf[p + 1], self.buf[p + 2], self.buf[p + 3],
        ]))
    }

    #[inline]
    pub fn uint(&mut self) -> Result<u32, WireError> { self.word() }

    #[inline]
    pub fn int(&mut self) -> Result<i32, WireError> { self.word().map(|v| v as i32) }

    #[inline]
    pub fn fixed(&mut self) -> Result<Fixed, WireError> { self.word().map(|v| v as i32) }

    /// An object reference. 0 is null; the caller decides if that is allowed
    #[inline]
    pub fn object(&mut self) -> Result<u32, WireError> { self.word() }

    /// An id the sender is creating. The object map checks validity
    #[inline]
    pub fn new_id(&mut self) -> Result<u32, WireError> { self.word() }

    /// A string. None is the null string, length 0. libwayland's checks, in
    /// order: the terminator must be there and be the only one
    pub fn string(&mut self) -> Result<Option<&'a str>, WireError> {
        let len = self.word()? as usize;
        if len == 0 {
            return Ok(None);
        }
        let padded = pad4(len);
        if self.remaining() < padded {
            return Err(WireError::Truncated);
        }
        let bytes = &self.buf[self.pos..self.pos + len];
        self.pos += padded;

        if bytes[len - 1] != 0 {
            return Err(WireError::BadString);
        }
        let text = &bytes[..len - 1];
        let mut i = 0;
        while i < text.len() {
            if text[i] == 0 {
                return Err(WireError::BadString);
            }
            i += 1;
        }
        match core::str::from_utf8(text) {
            Ok(s) => Ok(Some(s)),
            Err(_) => Err(WireError::BadString),
        }
    }

    pub fn array(&mut self) -> Result<&'a [u8], WireError> {
        let len = self.word()? as usize;
        let padded = pad4(len);
        if self.remaining() < padded {
            return Err(WireError::Truncated);
        }
        let bytes = &self.buf[self.pos..self.pos + len];
        self.pos += padded;
        Ok(bytes)
    }
}

// Writing

/// One encoded argument. Fixed lists as a slice keep each send site to one
/// call, readable against the protocol table
#[derive(Clone, Copy, Debug)]
pub enum Arg<'a> {
    Int(i32),
    Uint(u32),
    Fixed(Fixed),
    /// 0 encodes a null object
    Object(u32),
    NewId(u32),
    /// None encodes the null string (length word 0)
    Str(Option<&'a str>),
    Array(&'a [u8]),
}

impl<'a> Arg<'a> {
    /// Encoded length in bytes, padding included
    pub fn encoded_len(&self) -> usize {
        match self {
            Arg::Str(None) => 4,
            Arg::Str(Some(s)) => 4 + pad4(s.len() + 1),
            Arg::Array(a) => 4 + pad4(a.len()),
            _ => 4,
        }
    }
}

/// How long this message will be once encoded, header included
pub fn message_len(args: &[Arg]) -> usize {
    let mut size = HEADER_LEN;
    for a in args {
        size += a.encoded_len();
    }
    size
}

/// Encode one message into 'out', returning how many bytes it took
pub fn encode(out: &mut [u8], sender: u32, opcode: u16, args: &[Arg])
    -> Result<usize, WireError>
{
    let size = message_len(args);
    if size > MAX_MESSAGE_SIZE {
        return Err(WireError::TooLarge);
    }
    if out.len() < size {
        return Err(WireError::NoRoom);
    }

    out[0..4].copy_from_slice(&sender.to_le_bytes());
    let word1 = ((size as u32) << 16) | opcode as u32;
    out[4..8].copy_from_slice(&word1.to_le_bytes());

    let mut p = HEADER_LEN;
    let put_word = |out: &mut [u8], p: &mut usize, v: u32| {
        out[*p..*p + 4].copy_from_slice(&v.to_le_bytes());
        *p += 4;
    };

    for a in args {
        match *a {
            Arg::Int(v)    => put_word(out, &mut p, v as u32),
            Arg::Uint(v)   => put_word(out, &mut p, v),
            Arg::Fixed(v)  => put_word(out, &mut p, v as u32),
            Arg::Object(v) => put_word(out, &mut p, v),
            Arg::NewId(v)  => put_word(out, &mut p, v),
            Arg::Str(None) => put_word(out, &mut p, 0),
            Arg::Str(Some(s)) => {
                let len = s.len() + 1; // the NUL counts
                put_word(out, &mut p, len as u32);
                out[p..p + s.len()].copy_from_slice(s.as_bytes());
                p += s.len();
                // the NUL plus the padding after it
                let pad = pad4(len) - s.len();
                for i in 0..pad {
                    out[p + i] = 0;
                }
                p += pad;
            }
            Arg::Array(a) => {
                put_word(out, &mut p, a.len() as u32);
                out[p..p + a.len()].copy_from_slice(a);
                p += a.len();
                let pad = pad4(a.len()) - a.len();
                for i in 0..pad {
                    out[p + i] = 0;
                }
                p += pad;
            }
        }
    }
    Ok(p)
}
