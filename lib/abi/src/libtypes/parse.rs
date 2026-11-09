//! parse_miku: JSON, CSV, INI, arguments

// FFI-safe string slice
#[repr(C)]
#[derive(Copy, Clone)]
pub struct ArgSlice {
    pub ptr: *const u8,
    pub len: usize,
}

// argument parser state
#[repr(C)]
pub struct MikuArgs {
    pub opts: [OptDef; MAX_OPTS],
    pub opt_count: usize,
    pub positional: [ArgSlice; MAX_ARGS],
    pub pos_count: usize,
    pub program: ArgSlice,
    pub error: [u8; 128],
    pub has_error: bool,
}

// option definition
#[repr(C)]
#[derive(Copy, Clone)]
pub struct OptDef {
    pub short: u8,                // short flag char, 0 if none
    pub long: [u8; 32],           // long name (null-terminated)
    pub kind: u8,                 // ARG_FLAG / ARG_VALUE / ARG_INT
    pub value: [u8; 64],          // parsed string value
    pub int_value: i64,           // parsed integer value
    pub present: bool,            // was this option seen?
    pub help: [u8; 64],           // help text
}

pub const MAX_OPTS: usize = 32;

pub const MAX_ARGS: usize = 64;

// CSV parser state
#[repr(C)]
pub struct MikuCsv {
    pub rows: [CsvRow; MAX_ROWS],
    pub nrows: usize,
    pub delimiter: u8,
}

// CSV writer state
#[repr(C)]
pub struct MikuCsvWriter {
    pub buf: *mut u8,
    pub cap: usize,
    pub len: usize,
    pub col: usize,    // current column in row
    pub delim: u8,
    pub error: bool,
}

// parsed CSV row
#[repr(C)]
pub struct CsvRow {
    pub fields: [CsvField; MAX_FIELDS],
    pub nfields: usize,
}

pub const MAX_ROWS: usize = 256;

// parsed CSV field (pointer + length into original data)
#[repr(C)]
#[derive(Copy, Clone)]
pub struct CsvField {
    pub start: u32,
    pub len: u16,
    pub quoted: bool,
}

pub const MAX_FIELDS: usize = 64;

pub type IniCallback = extern "C" fn(*const u8, *const u8, *mut u8);

#[repr(C)]
pub struct MikuIni {
    pub entries: [IniEntry; MAX_ENTRIES],
    pub count: usize,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct IniEntry {
    pub section: [u8; MAX_SEC_LEN + 1],
    pub key: [u8; MAX_KEY_LEN + 1],
    pub val: [u8; MAX_VAL_LEN + 1],
    pub used: bool,
}

pub const MAX_ENTRIES: usize = 128;

pub const MAX_SEC_LEN: usize = 31;

pub const MAX_KEY_LEN: usize = 63;

pub const MAX_VAL_LEN: usize = 127;

// parser state
#[repr(C)]
pub struct MikuJsonParser {
    pub pos: usize,
    pub next_tok: usize,
    pub super_stack: [i32; MAX_DEPTH],
    pub depth: usize,
}

// a single JSON token
#[repr(C)]
#[derive(Copy, Clone)]
pub struct MikuJsonToken {
    pub ttype: u8,        // token type (JSON_*)
    pub start: u32,       // start offset in input
    pub end: u32,         // end offset (exclusive)
    pub size: u32,        // number of child tokens (for object/array)
    pub parent: i32,      // parent token index, -1 for root
}

#[repr(C)]
pub struct MikuJsonWriter {
    pub buf: *mut u8,
    pub cap: usize,
    pub len: usize,
    pub depth: usize,
    pub container_stack: [u8; JSON_WRITE_MAX_DEPTH], // '{' or '['
    pub needs_comma: [bool; JSON_WRITE_MAX_DEPTH],
    pub error: bool,
}

pub const MAX_DEPTH: usize = 32;

pub const JSON_WRITE_MAX_DEPTH: usize = 32;

unsafe impl Send for MikuCsvWriter {}
