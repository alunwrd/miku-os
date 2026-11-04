// Cubin reader
//
// A `.cubin` is an ELF64 object with no program headers: machine code in
// `.text.<name>`, and the driver-filled constant bank in
// `.nv.constant0.<name>`.
//
// Used at build time by `lib/buildgen/shadergen.rs` and at run time by the
// kernel on bytes from `gpu_program`, so input is hostile. Every offset and
// size is checked, and bad input returns an error.

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

/// Everything the loader needs about one kernel in a cubin
pub struct Kernel {
    /// The name it has in its source, as `nvcc` mangled it into the section
    pub name: String,
    /// Machine code, as uploaded
    pub code: Vec<u8>,
    /// Bytes of constant bank 0 it reads; must fit the driver's slot
    pub const0: u32,
}

/// Why a cubin was refused
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bad {
    /// Shorter than an ELF64 header
    TooShort,
    /// No `\x7fELF`, or not 64-bit
    NotElf64,
    /// Section header table outside the file
    Sections,
    /// Section name table outside the file
    Names,
    /// Section contents outside the file
    Contents,
    /// No `.text.<name>` section
    NoKernel,
}

impl Bad {
    pub fn why(self) -> &'static str {
        match self {
            Bad::TooShort => "shorter than an ELF64 header",
            Bad::NotElf64 => "not an ELF64 object",
            Bad::Sections => "the section header table is outside the file",
            Bad::Names => "the section name table is outside the file",
            Bad::Contents => "a section's contents are outside the file",
            Bad::NoKernel => "no .text.<name> section - nothing to run",
        }
    }
}

const EHDR: usize = 0x40;
const E_SHOFF: usize = 0x28;
const E_SHENTSIZE: usize = 0x3a;
const E_SHNUM: usize = 0x3c;
const E_SHSTRNDX: usize = 0x3e;
const SH_NAME: usize = 0x00;
const SH_OFFSET: usize = 0x18;
const SH_SIZE: usize = 0x20;
/// The smallest section header this reads fields out of
const SHENT_MIN: usize = 0x28;

/// Limits on kernels per cubin and their size, for hostile input at run time
pub const MAX_KERNELS: usize = 64;
pub const MAX_CODE: usize = 1 << 20;

/// Read every kernel out of a cubin
pub fn kernels(elf: &[u8]) -> Result<Vec<Kernel>, Bad> {
    if elf.len() < EHDR {
        return Err(Bad::TooShort);
    }
    if &elf[0..4] != b"\x7fELF" || elf[4] != 2 {
        return Err(Bad::NotElf64);
    }

    let u16at = |o: usize| u16::from_le_bytes([elf[o], elf[o + 1]]) as usize;
    let u32at = |o: usize| u32::from_le_bytes([elf[o], elf[o + 1], elf[o + 2], elf[o + 3]]);
    let u64at = |o: usize| u64::from_le_bytes(elf[o..o + 8].try_into().unwrap_or([0; 8]));

    let shoff = u64at(E_SHOFF) as usize;
    let shentsize = u16at(E_SHENTSIZE);
    let shnum = u16at(E_SHNUM);
    let shstrndx = u16at(E_SHSTRNDX);

    // `checked_mul`, since `shnum * shentsize` is two untrusted 16-bit numbers
    let table = shentsize
        .checked_mul(shnum)
        .and_then(|n| shoff.checked_add(n))
        .ok_or(Bad::Sections)?;
    if shoff == 0 || shnum == 0 || shentsize < SHENT_MIN || table > elf.len() {
        return Err(Bad::Sections);
    }
    if shstrndx >= shnum {
        return Err(Bad::Names);
    }

    // The name table, whose bounds every later name is read against
    let strsh = shoff + shstrndx * shentsize;
    let stroff = u64at(strsh + SH_OFFSET) as usize;
    let strsize = u64at(strsh + SH_SIZE) as usize;
    let strend = stroff.checked_add(strsize).ok_or(Bad::Names)?;
    if strend > elf.len() {
        return Err(Bad::Names);
    }

    // A name is a NUL-terminated string in the name table, stopping at its end
    let name_at = |off: usize| -> String {
        if off >= strsize {
            return String::new();
        }
        let start = stroff + off;
        let end = elf[start..strend]
            .iter()
            .position(|&b| b == 0)
            .map(|n| start + n)
            .unwrap_or(strend);
        String::from_utf8_lossy(&elf[start..end]).into_owned()
    };

    // Two passes: code sections name the kernels, then constant banks match by
    // name
    let mut code: Vec<(String, Vec<u8>)> = Vec::new();
    let mut const0: Vec<(String, u32)> = Vec::new();

    for i in 0..shnum {
        let sh = shoff + i * shentsize;
        let name = name_at(u32at(sh + SH_NAME) as usize);
        let offset = u64at(sh + SH_OFFSET) as usize;
        let size = u64at(sh + SH_SIZE) as usize;

        if let Some(k) = name.strip_prefix(".text.") {
            let end = offset.checked_add(size).ok_or(Bad::Contents)?;
            if end > elf.len() {
                return Err(Bad::Contents);
            }
            if size > MAX_CODE || code.len() >= MAX_KERNELS {
                continue;
            }
            code.push((String::from(k), elf[offset..end].to_vec()));
        } else if let Some(k) = name.strip_prefix(".nv.constant0.") {
            const0.push((String::from(k), size as u32));
        }
    }

    if code.is_empty() {
        return Err(Bad::NoKernel);
    }

    Ok(code
        .into_iter()
        .map(|(name, bytes)| {
            let c = const0
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, s)| *s)
                .unwrap_or(0);
            Kernel { name, code: bytes, const0: c }
        })
        .collect())
}
