// libos log decoder for GSP-RM (libos v3.1.0 token format)
//
// GSP-RM writes no text into its log buffers. Every LibosLog() call site pushes
// a record of raw u64 tokens: the arguments, a pointer to a 'libosLogMetadata'
// describing the call site, then a timestamp; the put pointer lives in the
// first 8 bytes of the buffer. The metadata and every string it references
// (format string, __FILE__, string literals used as %s arguments) live in the
// .logging / .logging_const / .logging_metadata ELF sections, stripped out of
// the shipped GSP-RM image and published separately as gsp_log_ga10x.bin.
// Decoding a record resolves those pointers as virtual addresses inside that
// ELF.
//
// Reference: open-gpu-kernel-modules 570.144
//   src/common/uproc/os/common/include/libos_log.h        record + metadata
//   src/common/uproc/os/libos-v3.1.0/lib/liblogdecode.c   extraction, printf
//   src/common/uproc/os/libos-v3.1.0/lib/libelf.c         LibosElfMapVirtual
//   src/nvidia/inc/libraries/utils/nvprintf_level.h       RM_GSP_LOG_SPECIAL_*

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

// Where the logging ELF is on disk is the chip's business:
// `blackwell::chips::Chip::gsp_log`. It is the debug half of the firmware's
// build, shipped only in NVIDIA's .run package (firmware/gsp_log_ga10x.bin),
// not in linux-firmware. GB20x maps to the ga10x family (nv-firmware.h:
// NV_FIRMWARE_TYPE_GSP_LOG)

// ELF64
const PT_LOAD: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_NOBITS: u32 = 8;
const PHDR_SIZE: usize = 56;
const SHDR_SIZE: usize = 64;
const SYM_SIZE: usize = 24;

// libosLogMetadata: filename@0, format@8, lineNumber@16 (u32, 8-aligned),
// argumentCount@20, printLevel@21, specialType@22; sizeof = 24
const META_SIZE: u64 = 24;
// libosLogMetadata_extended: versionExtended@0, elfSectionName@8, meta@16
const META_EXT_MARKER: u64 = 0x8000_0000_0000_0000;
const META_EXT_META_OFF: u64 = 16;

const MAX_ARGS: usize = 20;

// RM_GSP_LOG_SPECIAL_*
#[allow(dead_code)] // one value of the format's special-token set
const SPECIAL_NONE: u8 = 0;
const SPECIAL_ASSERT_FAILED: u8 = 1;
const SPECIAL_ASSERT_OK_FAILED: u8 = 2;
const SPECIAL_CHECK_FAILED: u8 = 3;
const SPECIAL_CHECK_OK_FAILED: u8 = 4;

fn rd_u16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}
fn rd_u32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
fn rd_u64(d: &[u8], o: usize) -> Option<u64> {
    d.get(o..o + 8)
        .map(|s| u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

/// Read-only view of the logging ELF, able to resolve a GSP-RM virtual
/// address to bytes in the file
pub struct LogElf<'a> {
    data: &'a [u8],
    phoff: usize,
    phnum: usize,
    phentsize: usize,
    shoff: usize,
    shnum: usize,
    shentsize: usize,
}

impl<'a> LogElf<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < 64 || &data[0..4] != b"\x7fELF" {
            return None;
        }
        // 64-bit, little endian only; GSP-RM is riscv64 but only its section
        // data is read here, so e_machine is not checked
        if data[4] != 2 || data[5] != 1 {
            return None;
        }
        let elf = LogElf {
            data,
            phoff: rd_u64(data, 32)? as usize,
            shoff: rd_u64(data, 40)? as usize,
            phentsize: rd_u16(data, 54)? as usize,
            phnum: rd_u16(data, 56)? as usize,
            shentsize: rd_u16(data, 58)? as usize,
            shnum: rd_u16(data, 60)? as usize,
        };
        Some(elf)
    }

    fn phdr(&self, i: usize) -> Option<&'a [u8]> {
        let sz = if self.phentsize == 0 { PHDR_SIZE } else { self.phentsize };
        let off = self.phoff.checked_add(i.checked_mul(sz)?)?;
        self.data.get(off..off + PHDR_SIZE)
    }

    fn shdr(&self, i: usize) -> Option<&'a [u8]> {
        let sz = if self.shentsize == 0 { SHDR_SIZE } else { self.shentsize };
        let off = self.shoff.checked_add(i.checked_mul(sz)?)?;
        self.data.get(off..off + SHDR_SIZE)
    }

    /// LibosElfMapVirtual: program headers first, then sections by address. The
    /// logging ELF is a container of stripped sections, so the section path
    /// hits
    pub fn map(&self, addr: u64, size: u64) -> Option<&'a [u8]> {
        if size == 0 {
            return None;
        }
        let last = addr.checked_add(size - 1)?;
        for i in 0..self.phnum {
            let p = match self.phdr(i) {
                Some(p) => p,
                None => continue,
            };
            if rd_u32(p, 0)? != PT_LOAD {
                continue;
            }
            let vaddr = rd_u64(p, 16)?;
            let filesz = rd_u64(p, 32)?;
            let offset = rd_u64(p, 8)?;
            if filesz == 0 || addr < vaddr || last >= vaddr.checked_add(filesz)? {
                continue;
            }
            let fo = (offset + (addr - vaddr)) as usize;
            return self.data.get(fo..fo + size as usize);
        }
        for i in 0..self.shnum {
            let s = match self.shdr(i) {
                Some(s) => s,
                None => continue,
            };
            if rd_u32(s, 4)? == SHT_NOBITS {
                continue;
            }
            let saddr = rd_u64(s, 16)?;
            let ssize = rd_u64(s, 32)?;
            let soff = rd_u64(s, 24)?;
            if saddr == 0 || ssize == 0 || addr < saddr || last >= saddr.checked_add(ssize)? {
                continue;
            }
            let fo = (soff + (addr - saddr)) as usize;
            return self.data.get(fo..fo + size as usize);
        }
        None
    }

    /// NUL-terminated string at a virtual address, without the terminator. Its
    /// region bounds the scan, so a corrupt pointer cannot run off the end of
    /// the file
    pub fn map_cstr(&self, addr: u64) -> Option<&'a [u8]> {
        // One byte is enough to locate the region; grow from there
        let mut len = 0u64;
        loop {
            let chunk = self.map(addr + len, 1)?;
            if chunk[0] == 0 {
                break;
            }
            len += 1;
            if len > 512 {
                break;
            }
        }
        self.map(addr, len.max(1)).map(|s| &s[..len as usize])
    }

    /// Nearest preceding symbol, turning a bare image offset (from a NOCAT
    /// assert record) into a function name
    pub fn symbolize(&self, addr: u64) -> Option<(&'a str, u64)> {
        let mut best: Option<(&'a str, u64)> = None;
        for i in 0..self.shnum {
            let s = match self.shdr(i) {
                Some(s) => s,
                None => continue,
            };
            if rd_u32(s, 4)? != SHT_SYMTAB {
                continue;
            }
            let off = rd_u64(s, 24)? as usize;
            let size = rd_u64(s, 32)? as usize;
            let strtab_idx = rd_u32(s, 40)? as usize;
            let str_s = self.shdr(strtab_idx)?;
            let str_off = rd_u64(str_s, 24)? as usize;
            let str_size = rd_u64(str_s, 32)? as usize;
            let strtab = self.data.get(str_off..str_off + str_size)?;
            let tab = self.data.get(off..off + size)?;
            for e in tab.chunks_exact(SYM_SIZE) {
                let value = rd_u64(e, 8)?;
                let ssize = rd_u64(e, 16)?;
                if value == 0 || addr < value {
                    continue;
                }
                // Prefer a symbol whose extent covers the address; otherwise
                // the closest below it
                let covers = ssize != 0 && addr < value + ssize;
                let better = match best {
                    None => true,
                    Some((_, b)) => value > b,
                };
                if covers || better {
                    let name_off = rd_u32(e, 0)? as usize;
                    let name = strtab.get(name_off..).and_then(|r| {
                        let end = r.iter().position(|&b| b == 0).unwrap_or(0);
                        core::str::from_utf8(&r[..end]).ok()
                    });
                    if let Some(n) = name {
                        if !n.is_empty() {
                            best = Some((n, value));
                        }
                    }
                    if covers {
                        return best;
                    }
                }
            }
        }
        best
    }
}

struct Metadata {
    filename: u64,
    format: u64,
    line: u32,
    arg_count: u8,
    level: u8,
    special: u8,
}

fn read_metadata(elf: &LogElf, va: u64) -> Option<Metadata> {
    let raw = elf.map(va, META_SIZE)?;
    let filename = rd_u64(raw, 0)?;
    // An extended record stores the marker where 'filename' would be, with the
    // real metadata 16 bytes further in (_getLoggingMetadata)
    let base = if filename == META_EXT_MARKER {
        let raw = elf.map(va + META_EXT_META_OFF, META_SIZE)?;
        raw
    } else {
        raw
    };
    Some(Metadata {
        filename: rd_u64(base, 0)?,
        format: rd_u64(base, 8)?,
        line: rd_u32(base, 16)?,
        arg_count: *base.get(20)?,
        level: *base.get(21)?,
        special: *base.get(22)?,
    })
}

/// One decoded GSP-RM log line. The printer outputs the lines in order without
/// the timestamp
#[allow(dead_code)]
pub struct LogRecord {
    pub timestamp: u64,
    pub file: String,
    pub line: u32,
    pub level: u8,
    pub text: String,
}

pub fn level_name(level: u8) -> &'static str {
    match level {
        1 => "INFO",
        2 => "NOTICE",
        3 => "WARN",
        4 => "ERROR",
        5 => "HW_ERROR",
        6 => "FATAL",
        _ => "?",
    }
}

/// Why a buffer produced nothing, so the caller can say so
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeStatus {
    /// GSP-RM never advanced the put pointer: it never logged into this buffer
    /// (or never ran)
    Empty,
    /// Records were found
    Ok,
    /// The put pointer is past the end of the ring, or the first record's
    /// metadata pointer does not resolve: usually the wrong log ELF for this
    /// firmware build
    Unusable,
}

/// Decode one libos log buffer.
///
/// Layout: u64 put pointer at offset 0, then a ring of u64 entries. Records are
/// written forwards and read backwards from put: {args..., meta, timestamp}.
/// Returns records in chronological order.
///
/// 'previous_put' is the put value from the last call on this buffer; pass 0 to
/// read everything. The returned put lets a caller poll the buffer while GSP-RM
/// runs and print only what is new
pub fn decode_buffer(
    elf: &LogElf,
    buf: &[u8],
    previous_put: u64,
    limit: usize,
) -> (Vec<LogRecord>, DecodeStatus, u64) {
    let mut out = Vec::new();
    if buf.len() < 16 {
        return (out, DecodeStatus::Unusable, previous_put);
    }
    let entries = (buf.len() / 8 - 1) as u64;
    let put = match rd_u64(buf, 0) {
        Some(p) => p,
        None => return (out, DecodeStatus::Unusable, previous_put),
    };
    if put == 0 {
        return (out, DecodeStatus::Empty, 0);
    }
    if put <= previous_put {
        return (out, DecodeStatus::Ok, put);
    }

    let entry = |i: u64| -> u64 {
        let off = 8 + ((i % entries) as usize) * 8;
        rd_u64(buf, off).unwrap_or(0)
    };

    // Everything below put - entries has been overwritten, and everything at or
    // below previous_put has been printed
    let floor = put.saturating_sub(entries).max(previous_put);
    let mut i = put;
    let mut resolved_any = false;

    while i > floor && out.len() < limit {
        i -= 1;
        let timestamp = entry(i);
        if i <= floor {
            break;
        }
        i -= 1;
        let meta_va = entry(i);
        let meta = match read_metadata(elf, meta_va) {
            Some(m) if (m.arg_count as usize) <= MAX_ARGS => m,
            // A pointer that does not resolve means we are past the oldest
            // intact record, or the ELF does not match this build
            _ => break,
        };
        resolved_any = true;
        let argc = meta.arg_count as u64;
        if i < floor + argc {
            break;
        }
        let mut args = [0u64; MAX_ARGS];
        for j in (0..argc as usize).rev() {
            i -= 1;
            args[j] = entry(i);
        }

        let fmt = elf.map_cstr(meta.format).unwrap_or(b"<unresolved format>");
        let file = elf.map_cstr(meta.filename).unwrap_or(b"?");
        out.push(LogRecord {
            timestamp,
            file: basename(file),
            line: meta.line,
            level: meta.level,
            text: format_record(elf, fmt, &args[..argc as usize], meta.special),
        });
    }

    out.reverse();
    // put moved but nothing decoded means the metadata pointers do not land in
    // this ELF: almost always a version mismatch
    if resolved_any {
        (out, DecodeStatus::Ok, put)
    } else {
        (out, DecodeStatus::Unusable, put)
    }
}

fn basename(path: &[u8]) -> String {
    let start = path
        .iter()
        .rposition(|&b| b == b'/' || b == b'\\')
        .map_or(0, |p| p + 1);
    String::from_utf8_lossy(&path[start..]).into_owned()
}

fn push_cstr(out: &mut String, s: &[u8]) {
    for &b in s {
        out.push(if (0x20..0x7f).contains(&b) { b as char } else { '.' });
    }
}

/// printf subset used by GSP-RM's logs. Arguments arrive as raw u64 (C casts
/// every argument to NvU64 at the call site), so the length modifier decides
/// how much of each is meaningful
fn format_record(elf: &LogElf, fmt: &[u8], args: &[u64], special: u8) -> String {
    let mut out = String::new();
    let mut ai = 0usize;

    // A "special" record carries the assert/check expression as the format
    // string, not to be interpreted as a format
    let fixed = match special {
        SPECIAL_ASSERT_FAILED => {
            out.push_str("Assertion failed: ");
            true
        }
        SPECIAL_CHECK_FAILED => {
            out.push_str("Check failed: ");
            true
        }
        SPECIAL_ASSERT_OK_FAILED | SPECIAL_CHECK_OK_FAILED => {
            let status = args.get(ai).copied().unwrap_or(0);
            ai += 1;
            out.push_str(if special == SPECIAL_ASSERT_OK_FAILED {
                "Assertion failed: status "
            } else {
                "Check failed: status "
            });
            push_hex(&mut out, status as u32 as u64, 8, false);
            out.push_str(" returned from ");
            true
        }
        _ => false,
    };
    if fixed {
        push_cstr(&mut out, fmt);
        return out;
    }

    let mut k = 0usize;
    while k < fmt.len() {
        let c = fmt[k];
        if c != b'%' {
            out.push(if (0x20..0x7f).contains(&c) || c == b'\n' || c == b'\t' {
                c as char
            } else {
                '.'
            });
            k += 1;
            continue;
        }
        k += 1;
        if k >= fmt.len() {
            break;
        }
        if fmt[k] == b'%' {
            out.push('%');
            k += 1;
            continue;
        }

        // flags
        let (mut left, mut zero, mut alt, mut plus, mut space) = (false, false, false, false, false);
        while k < fmt.len() {
            match fmt[k] {
                b'-' => left = true,
                b'0' => zero = true,
                b'#' => alt = true,
                b'+' => plus = true,
                b' ' => space = true,
                _ => break,
            }
            k += 1;
        }
        // width
        let mut width = 0usize;
        if k < fmt.len() && fmt[k] == b'*' {
            width = args.get(ai).copied().unwrap_or(0) as usize;
            ai += 1;
            k += 1;
        } else {
            while k < fmt.len() && fmt[k].is_ascii_digit() {
                width = width * 10 + (fmt[k] - b'0') as usize;
                k += 1;
            }
        }
        // precision
        let mut prec: Option<usize> = None;
        if k < fmt.len() && fmt[k] == b'.' {
            k += 1;
            let mut p = 0usize;
            if k < fmt.len() && fmt[k] == b'*' {
                p = args.get(ai).copied().unwrap_or(0) as usize;
                ai += 1;
                k += 1;
            } else {
                while k < fmt.len() && fmt[k].is_ascii_digit() {
                    p = p * 10 + (fmt[k] - b'0') as usize;
                    k += 1;
                }
            }
            prec = Some(p);
        }
        // length modifier
        let mut wide = false;
        while k < fmt.len() {
            match fmt[k] {
                b'l' | b'z' | b'j' | b't' => {
                    wide = true;
                    k += 1;
                }
                b'h' => {
                    k += 1;
                }
                _ => break,
            }
        }
        if k >= fmt.len() {
            break;
        }
        let conv = fmt[k];
        k += 1;

        let raw = args.get(ai).copied().unwrap_or(0);
        let mut body = String::new();
        match conv {
            b'd' | b'i' => {
                ai += 1;
                let v = if wide { raw as i64 } else { raw as u32 as i32 as i64 };
                if v < 0 {
                    body.push('-');
                } else if plus {
                    body.push('+');
                } else if space {
                    body.push(' ');
                }
                push_dec(&mut body, v.unsigned_abs());
            }
            b'u' => {
                ai += 1;
                push_dec(&mut body, if wide { raw } else { raw as u32 as u64 });
            }
            b'x' | b'X' => {
                ai += 1;
                if alt {
                    body.push_str("0x");
                }
                push_hex(
                    &mut body,
                    if wide { raw } else { raw as u32 as u64 },
                    prec.unwrap_or(0),
                    conv == b'X',
                );
            }
            b'o' => {
                ai += 1;
                push_oct(&mut body, if wide { raw } else { raw as u32 as u64 });
            }
            b'p' => {
                ai += 1;
                body.push_str("0x");
                push_hex(&mut body, raw, 0, false);
            }
            b'c' => {
                ai += 1;
                let ch = raw as u8;
                body.push(if (0x20..0x7f).contains(&ch) { ch as char } else { '.' });
            }
            b's' => {
                ai += 1;
                // %s arguments are pointers into the same logging sections
                match elf.map_cstr(raw) {
                    Some(s) => {
                        let s = match prec {
                            Some(p) if p < s.len() => &s[..p],
                            _ => s,
                        };
                        push_cstr(&mut body, s);
                    }
                    None => {
                        body.push_str("<str@0x");
                        push_hex(&mut body, raw, 0, false);
                        body.push('>');
                    }
                }
            }
            // RM's logs use no floats; consume the argument so the rest of the
            // line stays aligned
            b'f' | b'g' | b'e' | b'F' | b'G' | b'E' => {
                ai += 1;
                body.push_str("<float>");
            }
            other => {
                body.push('%');
                body.push(other as char);
            }
        }

        if body.len() < width {
            let pad = width - body.len();
            if left {
                out.push_str(&body);
                for _ in 0..pad {
                    out.push(' ');
                }
            } else if zero && prec.is_none() {
                // Zero padding goes after any sign or 0x prefix
                let split = if body.starts_with('-') || body.starts_with('+') {
                    1
                } else if body.starts_with("0x") {
                    2
                } else {
                    0
                };
                out.push_str(&body[..split]);
                for _ in 0..pad {
                    out.push('0');
                }
                out.push_str(&body[split..]);
            } else {
                for _ in 0..pad {
                    out.push(' ');
                }
                out.push_str(&body);
            }
        } else {
            out.push_str(&body);
        }
    }
    out
}

fn push_dec(out: &mut String, mut v: u64) {
    let mut buf = [0u8; 20];
    let mut n = 0;
    if v == 0 {
        out.push('0');
        return;
    }
    while v > 0 {
        buf[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    for i in (0..n).rev() {
        out.push(buf[i] as char);
    }
}

fn push_hex(out: &mut String, v: u64, min_digits: usize, upper: bool) {
    let digits = if upper { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
    let mut buf = [0u8; 16];
    let mut n = 0;
    let mut x = v;
    while x > 0 {
        buf[n] = digits[(x & 0xF) as usize];
        x >>= 4;
        n += 1;
    }
    if n == 0 {
        buf[0] = b'0';
        n = 1;
    }
    for _ in n..min_digits {
        out.push('0');
    }
    for i in (0..n).rev() {
        out.push(buf[i] as char);
    }
}

fn push_oct(out: &mut String, mut v: u64) {
    let mut buf = [0u8; 22];
    let mut n = 0;
    if v == 0 {
        out.push('0');
        return;
    }
    while v > 0 {
        buf[n] = b'0' + (v & 7) as u8;
        v >>= 3;
        n += 1;
    }
    for i in (0..n).rev() {
        out.push(buf[i] as char);
    }
}
