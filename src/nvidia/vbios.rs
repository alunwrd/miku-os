// VBIOS extraction from the PCI expansion ROM
//
// A VBIOS image is a chain of "ROM images" tagged with a PCIR structure.
// For NVIDIA cards on UEFI systems the PCI expansion ROM is usually the
// full legacy VBIOS followed by an UEFI image. For our purposes we walk
// the chain, find the legacy x86 image (code_type=0), and return its
// bytes. That image drives devinit and contains the DCB (Display Config
// Block) we need later for mode-setting
//
// Format summary (PCI Firmware Specification, chapter 5):
//   Each image begins with 0x55AA, then at offset 0x18 holds a little-
//   endian pointer to the PCIR structure. The PCIR header is:
//     0x00: 'PCIR'
//     0x04: vendor id   (u16)
//     0x06: device id   (u16)
//     0x0C: length      (u16)
//     0x14: code type   (u8)  0 = x86 BIOS, 3 = EFI
//     0x15: last image  (u8)  bit 7 set = last image in the chain
//
// The image size in 512-byte units lives at offset 0x02 of the image
// itself

extern crate alloc;
use alloc::vec::Vec;
use core::ptr;

use crate::grub;
use crate::net::pci::{pci_read32, pci_write32};
use crate::serial_println;

use super::pci::{GpuDevice, CFG_ROM_BASE};

const ROM_SIGNATURE: u16 = 0xAA55;
const PCIR_SIGNATURE: [u8; 4] = *b"PCIR";
const MAX_IMAGES: usize = 8;
const MAX_ROM_SIZE: u64 = 1024 * 1024; // cap at 1 MiB as a sanity limit

#[derive(Clone, Debug)]
pub struct VbiosImage {
    pub bytes: Vec<u8>,
    pub code_type: u8,
    pub vendor: u16,
    pub device: u16,
}

/// Read the full expansion ROM into a Vec<u8> by temporarily enabling it
/// Returns None if the device has no ROM assigned or the read looks bogus
pub fn read_rom(gpu: &GpuDevice) -> Option<Vec<u8>> {
    if gpu.rom_phys == 0 || gpu.rom_size == 0 {
        return None;
    }
    if gpu.rom_size > MAX_ROM_SIZE {
        serial_println!(
            "[nvidia] ROM size {:#x} exceeds sanity cap, clamping",
            gpu.rom_size
        );
    }
    let size = core::cmp::min(gpu.rom_size, MAX_ROM_SIZE) as usize;

    // Enable the ROM (bit 0 of the ROM BAR) and make sure memory-space
    // decoding is on in the command register. The caller already handled
    // CMD_MEM_SPACE but we do not touch it here
    let orig = pci_read32(gpu.bus, gpu.dev, gpu.func, CFG_ROM_BASE);
    let enabled = (orig & 0xFFFF_F800) | 0x1;
    pci_write32(gpu.bus, gpu.dev, gpu.func, CFG_ROM_BASE, enabled);

    let mut buf = Vec::with_capacity(size);
    buf.resize(size, 0);

    let virt = grub::phys_to_virt(gpu.rom_phys);
    unsafe {
        let src = virt as *const u8;
        for i in 0..size {
            buf[i] = ptr::read_volatile(src.add(i));
        }
    }

    // Restore the ROM disabled
    pci_write32(gpu.bus, gpu.dev, gpu.func, CFG_ROM_BASE, orig);

    // Sanity: first image must start with 0x55AA
    if buf.len() < 4 || u16::from_le_bytes([buf[0], buf[1]]) != ROM_SIGNATURE {
        serial_println!(
            "[nvidia] ROM has no 0x55AA at offset 0 (got {:#06x})",
            u16::from_le_bytes([buf[0], buf[1]])
        );
        return None;
    }
    Some(buf)
}

/// Walk the image chain and return every PCIR-tagged image
pub fn parse_images(rom: &[u8]) -> Vec<VbiosImage> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for _ in 0..MAX_IMAGES {
        if offset + 0x1A > rom.len() { break; }
        if u16::from_le_bytes([rom[offset], rom[offset + 1]]) != ROM_SIGNATURE {
            break;
        }
        let pcir_ptr = u16::from_le_bytes([rom[offset + 0x18], rom[offset + 0x19]]) as usize;
        let pcir = offset + pcir_ptr;
        if pcir + 0x18 > rom.len() || rom[pcir..pcir + 4] != PCIR_SIGNATURE {
            break;
        }

        let vendor = u16::from_le_bytes([rom[pcir + 4], rom[pcir + 5]]);
        let device = u16::from_le_bytes([rom[pcir + 6], rom[pcir + 7]]);
        let code_type = rom[pcir + 0x14];
        let last = rom[pcir + 0x15] & 0x80 != 0;

        // Image size: some vendors put it at PCIR+0x10 (size in 512-byte units), but the legacy spot is offset 0x02 of the image itself
        let size_units = rom[offset + 2] as usize;
        let size = size_units * 512;
        if size == 0 || offset + size > rom.len() {
            break;
        }

        out.push(VbiosImage {
            bytes: rom[offset..offset + size].to_vec(),
            code_type,
            vendor,
            device,
        });

        if last { break; }
        offset += size;
    }
    out
}

/// Pick the legacy x86 BIOS image (code_type = 0) from the chain
pub fn pick_legacy(images: &[VbiosImage]) -> Option<&VbiosImage> {
    images.iter().find(|img| img.code_type == 0)
}

pub fn log_images(images: &[VbiosImage]) {
    for (i, img) in images.iter().enumerate() {
        let kind = match img.code_type {
            0 => "x86 BIOS",
            1 => "Open Firmware",
            2 => "PA-RISC",
            3 => "EFI",
            other => match other { 0xFF => "reserved", _ => "unknown" },
        };
        serial_println!(
            "[nvidia] VBIOS image {}: type={} ({}) vendor={:04x} device={:04x} size={} B",
            i, img.code_type, kind, img.vendor, img.device, img.bytes.len()
        );
    }
}

// BIT (BIOS Information Table) parser
//
// Reference: NVIDIA open-gpu-kernel-modules, src/common/shared/nvbios/bit.h 
// The BIT header sits inside the legacy VBIOS image and looks like:
//
//   offset 0..1  Id            u16 little-endian = 0xB8FF
//   offset 2..5  Signature     ASCII "BIT\0"
//   offset 6..7  BcdVersion    u16, BCD encoded
//   offset 8     HeaderSize    u8  (normally 12)
//   offset 9     TokenSize     u8  (normally 6)
//   offset 10    TokenEntries  u8
//   offset 11    HeaderChecksum u8
//
// Each token is TokenSize bytes:
//
//   offset 0     TokenId       u8  (ASCII tag, 'D','I','P',...)
//   offset 1     DataVersion   u8
//   offset 2..3  DataSize      u16
//   offset 4..5  DataPtr       u16  (offset inside the legacy image)
//
// Well-known token IDs (subset):
//   'I' (0x49) init script / PINIT table
//   'D' (0x44) DCB (Display Config Block) pointer
//   'P' (0x50) performance tables
//   'N' (0x4E) NV init data
//   'S' (0x53) string table
//   'i' (0x69) Falcon ucode data
//   'p' (0x70) Falcon ucode code
//   'B' (0x42) BIOS data
//   'C' (0x43) clock config

pub const BIT_SIG: [u8; 6] = [0xFF, 0xB8, b'B', b'I', b'T', 0x00];

#[derive(Copy, Clone, Debug)]
pub struct BitHeader {
    pub offset: usize,
    pub bcd_version: u16,
    pub header_size: u8,
    pub token_size: u8,
    pub token_entries: u8,
}

#[derive(Copy, Clone, Debug)]
pub struct BitToken {
    pub id: u8,
    pub data_version: u8,
    pub data_size: u16,
    pub data_ptr: u16,
}

impl BitToken {
    /// ASCII label for known IDs, otherwise the raw value as a char
    pub fn label(&self) -> &'static str {
        match self.id {
            b'B' => "B (BIOS data)",
            b'C' => "C (clocks)",
            b'D' => "D (DCB ptr)",
            b'I' => "I (init script)",
            b'L' => "L (legacy)",
            b'M' => "M (memory)",
            b'N' => "N (nv init data)",
            b'P' => "P (performance)",
            b'S' => "S (strings)",
            b'V' => "V (virtual P-states)",
            b'i' => "i (falcon data)",
            b'p' => "p (falcon code)",
            b'2' => "2 (2D table)",
            _    => "?",
        }
    }
}

/// Locate the BIT header inside a legacy VBIOS image
pub fn find_bit_header(rom: &[u8]) -> Option<BitHeader> {
    if rom.len() < 12 + BIT_SIG.len() { return None; }
    // Linear scan with early break on Id/sig match
    let mut i = 0usize;
    while i + BIT_SIG.len() + 6 < rom.len() {
        if rom[i..i + BIT_SIG.len()] == BIT_SIG {
            // BIT_SIG is Id(2) + "BIT\0"(4). Header continues at +6
            let hdr_off = i;
            // Minimum legal header size is 12
            let header_size = rom[hdr_off + 8];
            if header_size < 12 { i += 1; continue; }
            let bcd_version = u16::from_le_bytes([rom[hdr_off + 6], rom[hdr_off + 7]]);
            let token_size = rom[hdr_off + 9];
            let token_entries = rom[hdr_off + 10];
            if token_size < 6 { i += 1; continue; }
            return Some(BitHeader {
                offset: hdr_off,
                bcd_version,
                header_size,
                token_size,
                token_entries,
            });
        }
        i += 1;
    }
    None
}

/// Parse all tokens that follow the BIT header
pub fn parse_tokens(rom: &[u8], hdr: &BitHeader) -> Vec<BitToken> {
    let mut out = Vec::new();
    let base = hdr.offset + hdr.header_size as usize;
    let stride = hdr.token_size as usize;
    for n in 0..hdr.token_entries as usize {
        let p = base + n * stride;
        if p + 6 > rom.len() { break; }
        out.push(BitToken {
            id: rom[p],
            data_version: rom[p + 1],
            data_size: u16::from_le_bytes([rom[p + 2], rom[p + 3]]),
            data_ptr:  u16::from_le_bytes([rom[p + 4], rom[p + 5]]),
        });
    }
    out
}

pub fn find_token<'a>(tokens: &'a [BitToken], id: u8) -> Option<&'a BitToken> {
    tokens.iter().find(|t| t.id == id)
}

pub fn log_bit(hdr: &BitHeader, tokens: &[BitToken]) {
    serial_println!(
        "[nvidia] BIT header @ {:#x}: bcd={:#x} hdr_size={} tok_size={} tokens={}",
        hdr.offset, hdr.bcd_version, hdr.header_size, hdr.token_size, hdr.token_entries
    );
    for t in tokens {
        serial_println!(
            "[nvidia] BIT token '{}'/{:#x} v{} size={} ptr={:#x}",
            t.label(), t.id, t.data_version, t.data_size, t.data_ptr
        );
    }
}
