// NVKV, the key/value encoding GSP-RM's newer interfaces carry their data in
//
// From 615 on, what the driver tells GSP-RM about the machine and what GSP-RM
// answers about the board are lists of keys (`dev/gsp-615.md`, section 4;
// `nvkv.h`, `nvkv.c` in OpenRM 615). A list is u64 words, each an instruction:
//
//   63           32 31  28 27     16 15      0
//   | count / imm32 | opcode | index  |   key   |
//
//   IMM32    one u32 value, in the instruction itself
//   SEQ32    `count` u32 values for the keys key, key+1, ... packed two to a
//            word after the instruction
//   SEQ64    `count` u64 values for consecutive keys, a word each
//   ARRAY8   one key whose value is `count` bytes, padded to a word
//   ARRAY32  one key, `count` u32s, padded
//   ARRAY64  one key, `count` u64s
//
// The index is for keys in a set: the n-th region's base is key FB_REGION_BASE
// at index n. Unused padding is zero, and a decoder skips it.
//
// `sysinfo.rs` and `staticinfo.rs` know what a key means.

use alloc::vec::Vec;

const IMM32: u64 = 0;
const SEQ32: u64 = 1;
const SEQ64: u64 = 2;
const ARRAY8: u64 = 3;
const ARRAY32: u64 = 4;
const ARRAY64: u64 = 5;

fn instruction(opcode: u64, index: u16, key: u16, count: u32) -> u64 {
    (count as u64) << 32 | opcode << 28 | ((index as u64) & 0xfff) << 16 | key as u64
}

/// A list being written. Keys handed to the `seq*` calls are the first of a
/// run; the rest follow one by one
#[derive(Default)]
pub struct Encoder {
    words: Vec<u64>,
}

impl Encoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// One u32 (`NVKV_SET_SEQ32_1U`, which is an IMM32)
    pub fn u32(&mut self, index: u16, key: u16, v: u32) {
        self.words.push(instruction(IMM32, index, key, v));
    }

    /// u32s for `key`, `key + 1`, ...
    pub fn seq32(&mut self, index: u16, key: u16, values: &[u32]) {
        self.words.push(instruction(SEQ32, index, key, values.len() as u32));
        for pair in values.chunks(2) {
            let hi = pair.get(1).copied().unwrap_or(0);
            self.words.push(pair[0] as u64 | (hi as u64) << 32);
        }
    }

    /// One u64
    pub fn u64(&mut self, index: u16, key: u16, v: u64) {
        self.seq64(index, key, &[v]);
    }

    /// u64s for `key`, `key + 1`, ...
    pub fn seq64(&mut self, index: u16, key: u16, values: &[u64]) {
        self.words.push(instruction(SEQ64, index, key, values.len() as u32));
        self.words.extend_from_slice(values);
    }

    /// Bytes for one key, padded to a word with zeroes
    pub fn bytes(&mut self, index: u16, key: u16, b: &[u8]) {
        self.words.push(instruction(ARRAY8, index, key, b.len() as u32));
        self.push_padded(b);
    }

    /// u32s for one key
    pub fn array32(&mut self, index: u16, key: u16, v: &[u32]) {
        self.words.push(instruction(ARRAY32, index, key, v.len() as u32));
        let mut b = Vec::with_capacity(v.len() * 4);
        for x in v {
            b.extend_from_slice(&x.to_le_bytes());
        }
        self.push_padded(&b);
    }

    /// A string, sent with its NUL and counted with it (`nvkvSetString8`)
    pub fn string(&mut self, index: u16, key: u16, s: &str) {
        self.words.push(instruction(ARRAY8, index, key, s.len() as u32 + 1));
        let mut b = Vec::with_capacity(s.len() + 1);
        b.extend_from_slice(s.as_bytes());
        b.push(0);
        self.push_padded(&b);
    }

    fn push_padded(&mut self, b: &[u8]) {
        for chunk in b.chunks(8) {
            let mut w = [0u8; 8];
            w[..chunk.len()].copy_from_slice(chunk);
            self.words.push(u64::from_le_bytes(w));
        }
    }

    pub fn words(&self) -> &[u64] {
        &self.words
    }

    /// The list as it goes on the wire, little-endian words
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.words.len() * 8);
        for w in &self.words {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out
    }
}

/// One key's value, as the decoder hands it over. Arrays are the raw bytes,
/// `len` elements long
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value<'a> {
    U32(u32),
    U64(u64),
    Bytes(&'a [u8]),
    U32s(&'a [u8]),
    U64s(&'a [u8]),
}

impl<'a> Value<'a> {
    pub fn as_u32(&self) -> Option<u32> {
        match *self {
            Value::U32(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Value::U64(v) => Some(v),
            _ => None,
        }
    }

    /// A string sent as bytes: up to its NUL, printable characters only
    pub fn as_str(&self) -> Option<alloc::string::String> {
        match *self {
            Value::Bytes(b) => Some(
                b.iter().take_while(|&&c| c != 0).filter(|&&c| (0x20..0x7f).contains(&c)).map(|&c| c as char).collect(),
            ),
            _ => None,
        }
    }
}

/// Why a list could not be read to its end
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// An instruction's values run past the end (`NV_ERR_OUT_OF_RANGE`)
    OutOfRange { at: usize },
    /// An opcode NVKV does not have (`NV_ERR_INVALID_COMMAND`)
    BadOpcode { at: usize, opcode: u8 },
}

/// Hand every key in `bytes` to `f` as (index, key, value), in order; a SEQ
/// instruction is one call per key. `f` returning false stops early, as
/// `nvkvDecode`'s handler can
pub fn decode(bytes: &[u8], mut f: impl FnMut(u16, u16, Value) -> bool) -> Result<(), DecodeError> {
    let n = bytes.len() / 8;
    let word = |i: usize| u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap());
    let mut i = 0;
    while i < n {
        let at = i;
        let w = word(i);
        i += 1;
        let key = w as u16;
        let index = ((w >> 16) & 0xfff) as u16;
        let opcode = (w >> 28) & 0xf;
        let count = (w >> 32) as usize;
        let span = |elem: usize| (count * elem).div_ceil(8);
        let need = match opcode {
            IMM32 => 0,
            SEQ32 | ARRAY32 => span(4),
            SEQ64 | ARRAY64 => count,
            ARRAY8 => span(1),
            _ => return Err(DecodeError::BadOpcode { at, opcode: opcode as u8 }),
        };
        if i + need > n {
            return Err(DecodeError::OutOfRange { at });
        }
        let data = &bytes[i * 8..(i + need) * 8];
        let go = match opcode {
            IMM32 => f(index, key, Value::U32(count as u32)),
            SEQ32 => (0..count).all(|k| {
                let v = u32::from_le_bytes(data[k * 4..k * 4 + 4].try_into().unwrap());
                f(index, key.wrapping_add(k as u16), Value::U32(v))
            }),
            SEQ64 => (0..count).all(|k| {
                let v = u64::from_le_bytes(data[k * 8..k * 8 + 8].try_into().unwrap());
                f(index, key.wrapping_add(k as u16), Value::U64(v))
            }),
            ARRAY8 => f(index, key, Value::Bytes(&data[..count])),
            ARRAY32 => f(index, key, Value::U32s(&data[..count * 4])),
            _ => f(index, key, Value::U64s(&data[..count * 8])),
        };
        if !go {
            return Ok(());
        }
        i += need;
    }
    Ok(())
}
