// Simple CSV parser
// Parses comma-separated values, Supports quoted fields with escapes
// No heap allocation for parsing - returns field pointers into the input
// Handles: commas, double-quoted fields, escaped quotes (""), newlines in quotes

use crate::string;

const MAX_FIELDS: usize = 64;
const MAX_ROWS: usize = 256;

// parsed CSV field (pointer + length into original data)
#[repr(C)]
#[derive(Copy, Clone)]
pub struct CsvField {
    pub start: u32,
    pub len: u16,
    pub quoted: bool,
}

const EMPTY_FIELD: CsvField = CsvField { start: 0, len: 0, quoted: false };

// parsed CSV row
#[repr(C)]
pub struct CsvRow {
    pub fields: [CsvField; MAX_FIELDS],
    pub nfields: usize,
}

// CSV parser state
#[repr(C)]
pub struct MikuCsv {
    pub rows: [CsvRow; MAX_ROWS],
    pub nrows: usize,
    pub delimiter: u8,
}

// create CSV parser with comma delimiter
#[no_mangle]
pub extern "C" fn miku_csv_new() -> MikuCsv {
    miku_csv_with_delimiter(b',')
}

// create CSV parser with custom delimiter
#[no_mangle]
pub extern "C" fn miku_csv_with_delimiter(delim: u8) -> MikuCsv {
    MikuCsv {
        rows: unsafe { core::mem::zeroed() },
        nrows: 0,
        delimiter: delim,
    }
}

// parse CSV data
// Returns number of rows parsed
#[no_mangle]
pub extern "C" fn miku_csv_parse(
    csv: *mut MikuCsv,
    data: *const u8,
    data_len: usize,
) -> i32 {
    if csv.is_null() || data.is_null() || data_len == 0 {
        return -1;
    }

    let csv = unsafe { &mut *csv };
    csv.nrows = 0;
    let delim = csv.delimiter;
    let mut pos = 0usize;

    unsafe {
        while pos < data_len && csv.nrows < MAX_ROWS {
            let row = &mut csv.rows[csv.nrows];
            row.nfields = 0;

            loop {
                if row.nfields >= MAX_FIELDS { break; }

                // skip leading whitespace (not in quotes)
                while pos < data_len
                    && (*data.add(pos) == b' ' || *data.add(pos) == b'\t')
                {
                    pos += 1;
                }

                if pos >= data_len || *data.add(pos) == b'\n' {
                    break;
                }

                let field = &mut row.fields[row.nfields];

                if *data.add(pos) == b'"' {
                    // quoted field
                    pos += 1;
                    field.start = pos as u32;
                    field.quoted = true;
                    let fstart = pos;

                    while pos < data_len {
                        if *data.add(pos) == b'"' {
                            if pos + 1 < data_len && *data.add(pos + 1) == b'"' {
                                pos += 2; // escaped quote
                                continue;
                            }
                            break;
                        }
                        pos += 1;
                    }
                    field.len = (pos - fstart) as u16;
                    if pos < data_len && *data.add(pos) == b'"' {
                        pos += 1;
                    }
                    // skip to delimiter or newline
                    while pos < data_len
                        && *data.add(pos) != delim
                        && *data.add(pos) != b'\n'
                        && *data.add(pos) != b'\r'
                    {
                        pos += 1;
                    }
                } else {
                    // unquoted field
                    field.start = pos as u32;
                    field.quoted = false;
                    let fstart = pos;

                    while pos < data_len
                        && *data.add(pos) != delim
                        && *data.add(pos) != b'\n'
                        && *data.add(pos) != b'\r'
                    {
                        pos += 1;
                    }

                    // trim trailing whitespace
                    let mut fend = pos;
                    while fend > fstart
                        && (*data.add(fend - 1) == b' ' || *data.add(fend - 1) == b'\t')
                    {
                        fend -= 1;
                    }
                    field.len = (fend - fstart) as u16;
                }

                row.nfields += 1;

                if pos < data_len && *data.add(pos) == delim {
                    pos += 1;
                    continue;
                }
                break;
            }

            // skip newline
            if pos < data_len && *data.add(pos) == b'\r' { pos += 1; }
            if pos < data_len && *data.add(pos) == b'\n' { pos += 1; }

            if row.nfields > 0 {
                csv.nrows += 1;
            }
        }
    }

    csv.nrows as i32
}

// get number of rows
#[no_mangle]
pub extern "C" fn miku_csv_rows(csv: *const MikuCsv) -> usize {
    if csv.is_null() { return 0; }
    unsafe { (*csv).nrows }
}

// get number of fields in a row
#[no_mangle]
pub extern "C" fn miku_csv_cols(csv: *const MikuCsv, row: usize) -> usize {
    if csv.is_null() { return 0; }
    let csv = unsafe { &*csv };
    if row >= csv.nrows { return 0; }
    csv.rows[row].nfields
}

// Get pointer and length of a field
// Returns pointer into original data, writes length to *out_len
#[no_mangle]
pub extern "C" fn miku_csv_field(
    csv: *const MikuCsv,
    data: *const u8,
    row: usize,
    col: usize,
    out_len: *mut usize,
) -> *const u8 {
    if csv.is_null() || data.is_null() { return core::ptr::null(); }
    let csv = unsafe { &*csv };
    if row >= csv.nrows { return core::ptr::null(); }
    let r = &csv.rows[row];
    if col >= r.nfields { return core::ptr::null(); }
    let f = &r.fields[col];
    if !out_len.is_null() {
        unsafe { *out_len = f.len as usize; }
    }
    unsafe { data.add(f.start as usize) }
}

// compare field with a string
#[no_mangle]
pub extern "C" fn miku_csv_field_eq(
    csv: *const MikuCsv,
    data: *const u8,
    row: usize,
    col: usize,
    s: *const u8,
) -> bool {
    let mut flen = 0usize;
    let fptr = miku_csv_field(csv, data, row, col, &mut flen);
    if fptr.is_null() || s.is_null() { return false; }
    let slen = string::miku_strlen(s);
    if slen != flen { return false; }
    string::miku_strncmp(fptr, s, flen) == 0
}

// get field as integer
#[no_mangle]
pub extern "C" fn miku_csv_field_int(
    csv: *const MikuCsv,
    data: *const u8,
    row: usize,
    col: usize,
    default: i64,
) -> i64 {
    let mut flen = 0usize;
    let fptr = miku_csv_field(csv, data, row, col, &mut flen);
    if fptr.is_null() || flen == 0 { return default; }

    // parse manually since string is not null-terminated at field boundary
    let mut result: i64 = 0;
    let mut neg = false;
    let mut i = 0usize;

    unsafe {
        if *fptr == b'-' { neg = true; i = 1; }
        else if *fptr == b'+' { i = 1; }

        while i < flen {
            let c = *fptr.add(i);
            if c < b'0' || c > b'9' { return default; }
            result = result.wrapping_mul(10).wrapping_add((c - b'0') as i64);
            i += 1;
        }
    }

    if neg { result.wrapping_neg() } else { result }
}

// get field as unsigned integer
#[no_mangle]
pub extern "C" fn miku_csv_field_u64(
    csv: *const MikuCsv,
    data: *const u8,
    row: usize,
    col: usize,
    default: u64,
) -> u64 {
    let mut flen = 0usize;
    let fptr = miku_csv_field(csv, data, row, col, &mut flen);
    if fptr.is_null() || flen == 0 { return default; }
    let mut result: u64 = 0;
    let mut i = 0usize;
    unsafe {
        if *fptr == b'+' { i = 1; }
        while i < flen {
            let c = *fptr.add(i);
            if c < b'0' || c > b'9' { return default; }
            result = result * 10 + (c - b'0') as u64;
            i += 1;
        }
    }
    result
}

// check if field is empty
#[no_mangle]
pub extern "C" fn miku_csv_field_empty(
    csv: *const MikuCsv,
    row: usize,
    col: usize,
) -> bool {
    if csv.is_null() { return true; }
    let csv = unsafe { &*csv };
    if row >= csv.nrows { return true; }
    let r = &csv.rows[row];
    if col >= r.nfields { return true; }
    r.fields[col].len == 0
}

// find column index by header name (row 0)
#[no_mangle]
pub extern "C" fn miku_csv_find_col(
    csv: *const MikuCsv,
    data: *const u8,
    name: *const u8,
) -> i32 {
    if csv.is_null() || data.is_null() || name.is_null() { return -1; }
    let csv = unsafe { &*csv };
    if csv.nrows == 0 { return -1; }
    let header = &csv.rows[0];
    let nlen = string::miku_strlen(name);
    for col in 0..header.nfields {
        let f = &header.fields[col];
        if f.len as usize == nlen {
            if string::miku_strncmp(
                unsafe { data.add(f.start as usize) },
                name,
                nlen,
            ) == 0 {
                return col as i32;
            }
        }
    }
    -1
}

// CSV Writer //

const WRITER_BUF_MAX: usize = 8192;

// CSV writer state
#[repr(C)]
pub struct MikuCsvWriter {
    buf: *mut u8,
    cap: usize,
    len: usize,
    col: usize,    // current column in row
    delim: u8,
    error: bool,
}

unsafe impl Send for MikuCsvWriter {}

// initialize writer with heap buffer
#[no_mangle]
pub extern "C" fn miku_csv_writer_new(delim: u8) -> MikuCsvWriter {
    let buf = crate::heap::miku_malloc(WRITER_BUF_MAX);
    MikuCsvWriter {
        buf,
        cap: if buf.is_null() { 0 } else { WRITER_BUF_MAX },
        len: 0,
        col: 0,
        delim: if delim == 0 { b',' } else { delim },
        error: buf.is_null(),
    }
}

// initialize writer into caller-provided buffer
#[no_mangle]
pub extern "C" fn miku_csv_writer_init(buf: *mut u8, cap: usize, delim: u8) -> MikuCsvWriter {
    MikuCsvWriter {
        buf,
        cap,
        len: 0,
        col: 0,
        delim: if delim == 0 { b',' } else { delim },
        error: buf.is_null() || cap == 0,
    }
}

fn writer_put(w: &mut MikuCsvWriter, byte: u8) {
    if w.error { return; }
    if w.len >= w.cap {
        w.error = true;
        return;
    }
    unsafe { *w.buf.add(w.len) = byte; }
    w.len += 1;
}

fn writer_put_bytes(w: &mut MikuCsvWriter, data: *const u8, len: usize) {
    if w.error { return; }
    if w.len + len > w.cap {
        w.error = true;
        return;
    }
    unsafe {
        crate::mem::miku_memcpy(w.buf.add(w.len), data, len);
    }
    w.len += len;
}

fn needs_quoting(data: *const u8, len: usize, delim: u8) -> bool {
    unsafe {
        for i in 0..len {
            let c = *data.add(i);
            if c == delim || c == b'"' || c == b'\n' || c == b'\r' {
                return true;
            }
        }
    }
    false
}

// write a field (auto-quotes if needed)
#[no_mangle]
pub extern "C" fn miku_csv_write_field(w: *mut MikuCsvWriter, data: *const u8, len: usize) {
    if w.is_null() { return; }
    let w = unsafe { &mut *w };
    if w.error { return; }

    // delimiter between fields
    if w.col > 0 {
        writer_put(w, w.delim);
    }

    if data.is_null() || len == 0 {
        w.col += 1;
        return;
    }

    if needs_quoting(data, len, w.delim) {
        writer_put(w, b'"');
        unsafe {
            for i in 0..len {
                let c = *data.add(i);
                if c == b'"' {
                    writer_put(w, b'"');
                }
                writer_put(w, c);
            }
        }
        writer_put(w, b'"');
    } else {
        writer_put_bytes(w, data, len);
    }
    w.col += 1;
}

// write a field from C string
#[no_mangle]
pub extern "C" fn miku_csv_write_cstr(w: *mut MikuCsvWriter, s: *const u8) {
    if s.is_null() {
        miku_csv_write_field(w, core::ptr::null(), 0);
        return;
    }
    let len = string::miku_strlen(s);
    miku_csv_write_field(w, s, len);
}

// write integer field
#[no_mangle]
pub extern "C" fn miku_csv_write_int(w: *mut MikuCsvWriter, val: i64) {
    let mut buf = [0u8; 21];
    crate::num::miku_itoa(val, buf.as_mut_ptr());
    let len = string::miku_strlen(buf.as_ptr());
    miku_csv_write_field(w, buf.as_ptr(), len);
}

// end row (write newline)
#[no_mangle]
pub extern "C" fn miku_csv_write_row_end(w: *mut MikuCsvWriter) {
    if w.is_null() { return; }
    let w = unsafe { &mut *w };
    writer_put(w, b'\n');
    w.col = 0;
}

// get written data length
#[no_mangle]
pub extern "C" fn miku_csv_writer_len(w: *const MikuCsvWriter) -> usize {
    if w.is_null() { return 0; }
    unsafe { (*w).len }
}

// get pointer to written data
#[no_mangle]
pub extern "C" fn miku_csv_writer_data(w: *const MikuCsvWriter) -> *const u8 {
    if w.is_null() { return core::ptr::null(); }
    unsafe { (*w).buf }
}

// check if writer had an error (buffer overflow)
#[no_mangle]
pub extern "C" fn miku_csv_writer_error(w: *const MikuCsvWriter) -> bool {
    if w.is_null() { return true; }
    unsafe { (*w).error }
}

// free heap-allocated writer (only if created with miku_csv_writer_new)
#[no_mangle]
pub extern "C" fn miku_csv_writer_free(w: *mut MikuCsvWriter) {
    if w.is_null() { return; }
    let w = unsafe { &mut *w };
    if !w.buf.is_null() {
        crate::heap::miku_free(w.buf);
        w.buf = core::ptr::null_mut();
    }
}

// reset writer to start over
#[no_mangle]
pub extern "C" fn miku_csv_writer_reset(w: *mut MikuCsvWriter) {
    if w.is_null() { return; }
    let w = unsafe { &mut *w };
    w.len = 0;
    w.col = 0;
    w.error = w.buf.is_null();
}

// iterate rows calling callback
// callback(row_idx, csv, data, user_ctx)
#[no_mangle]
pub extern "C" fn miku_csv_foreach_row(
    csv: *const MikuCsv,
    data: *const u8,
    cb: extern "C" fn(usize, *const MikuCsv, *const u8, *mut u8),
    ctx: *mut u8,
) {
    if csv.is_null() || data.is_null() { return; }
    let csv_ref = unsafe { &*csv };
    for i in 0..csv_ref.nrows {
        cb(i, csv, data, ctx);
    }
}
