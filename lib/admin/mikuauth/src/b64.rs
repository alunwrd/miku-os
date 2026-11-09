// The base64 of a password hash: RFC 4648's alphabet, no padding

pub use miku_base::base64::{decode_unpadded as decode, encode_unpadded as encode};

pub const MAX_DECODED: usize = 64;
