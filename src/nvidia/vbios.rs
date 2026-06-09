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

use super::mmio::MmioRegion;
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

// PRAMIN-window BIOS registers (BAR0 MMIO offsets)
const PDISP_VGA_BIOS_PTR: u32 = 0x0021_c04; // bit0 set => display/image absent
const PDISP_BIOS_WINDOW:  u32 = 0x0062_5f04; // bit3=enabled, [1:0]==1 in vram, [31:8]<<8 = vram addr
const PBUS_PRAMIN_WINDOW: u32 = 0x0000_1700; // window base in 64 KiB units
const PRAMIN_APERTURE:    u32 = 0x0070_0000; // BAR0 window into the selected VRAM region

/// Read the VBIOS from the PRAMIN shadow (the post-devinit BIOS image kept in
/// VRAM). On Turing the PCI expansion ROM only carries the legacy + EFI
/// images; the data image (PCIR code type 0xe0) that holds the falcon ucodes
/// (FWSEC etc.) lives only in this shadow. nouveau prefers PRAMIN over PROM
/// for exactly this reason (shadow.c method order: ramin before prom).
///
/// Mirrors nouveau shadowramin.c 'pramin_init'/'pramin_read' for the GV100+
/// path (TU11x is card_type TU, >= GV100):
///     0x021c04 bit0 set  -> display disabled, no image pointer available
///     0x625f04 bit3      -> window enabled; [1:0]==1 -> image is in VRAM;
///                           ([31:8] << 8) << 8 = 64 KiB-aligned VRAM address
///     0x001700 = addr>>16 maps the 1 MiB aperture at BAR0+0x700000 onto it
/// The previous 0x001700 value is restored before returning.
pub fn read_rom_pramin(bar0: &MmioRegion) -> Option<Vec<u8>> {
    let disp = bar0.read32(PDISP_VGA_BIOS_PTR);
    let win = bar0.read32(PDISP_BIOS_WINDOW);
    crate::println!(
        "    [pramin] 0x021c04={:#010x} 0x625f04={:#010x} 0x001700={:#010x}",
        disp, win, bar0.read32(PBUS_PRAMIN_WINDOW)
    );
    if disp & 0x1 != 0 {
        serial_println!("[nvidia] PRAMIN: display disabled (0x021c04={:#010x})", disp);
        crate::print_warn!("    [pramin] display disabled -> fall back to PROM");
        return None;
    }
    if win & 0x8 == 0 {
        serial_println!("[nvidia] PRAMIN: bios window not enabled (0x625f04={:#010x})", win);
        crate::print_warn!("    [pramin] window not enabled -> fall back to PROM");
        return None;
    }
    if win & 0x3 != 1 {
        serial_println!("[nvidia] PRAMIN: bios image not in VRAM (0x625f04={:#010x})", win);
        crate::print_warn!("    [pramin] image not in VRAM -> fall back to PROM");
        return None;
    }

    // [31:8] << 8 gives a 64 KiB-aligned VRAM byte address (low 16 bits zero)
    let mut addr = ((win as u64) & 0xffff_ff00) << 8;
    if addr == 0 {
        addr = ((bar0.read32(PBUS_PRAMIN_WINDOW) as u64) << 16) + 0x000f_0000;
    }

    let saved = bar0.read32(PBUS_PRAMIN_WINDOW);
    bar0.write32(PBUS_PRAMIN_WINDOW, (addr >> 16) as u32);

    let size = MAX_ROM_SIZE as usize; // the aperture is 1 MiB
    let mut buf = alloc::vec![0u8; size];
    for i in (0..size).step_by(4) {
        let w = bar0.read32(PRAMIN_APERTURE + i as u32);
        buf[i..i + 4].copy_from_slice(&w.to_le_bytes());
    }

    bar0.write32(PBUS_PRAMIN_WINDOW, saved);

    if u16::from_le_bytes([buf[0], buf[1]]) != ROM_SIGNATURE {
        serial_println!(
            "[nvidia] PRAMIN: no 0x55AA at offset 0 (got {:#06x}, vram_addr={:#x})",
            u16::from_le_bytes([buf[0], buf[1]]), addr
        );
        crate::print_warn!(
            "    [pramin] no 0x55AA at vram {:#x} (got {:#06x}) -> fall back to PROM",
            addr, u16::from_le_bytes([buf[0], buf[1]])
        );
        return None;
    }
    serial_println!("[nvidia] PRAMIN: BIOS shadow read from VRAM {:#x} (1 MiB)", addr);
    crate::println!("    [pramin] BIOS shadow read from VRAM {:#x} (1 MiB)", addr);
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

// Raw (untranslated) little-endian reads on the expansion ROM buffer
#[inline]
fn raw_rd16(rom: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*rom.get(off)?, *rom.get(off + 1)?]))
}
#[inline]
fn raw_rd32(rom: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *rom.get(off)?, *rom.get(off + 1)?, *rom.get(off + 2)?, *rom.get(off + 3)?,
    ]))
}

/// One PCIR-tagged ROM image with its ROM-absolute start offset
#[derive(Clone, Copy, Debug)]
pub struct RomImage {
    pub base: usize,
    pub size: usize,
    pub image_type: u8,
    pub last: bool,
}

/// Walk the ROM image chain exactly like nouveau 'nvbios_imagen':
///    image size  = PCIR[+0x10] * 512, overridden by NPDE[+0x08]*512 for
///                   images whose PCIR code type != 0x70
///     image type  = PCIR[+0x14]
///     last        = PCIR[+0x15] & 0x80, overridden by NPDE[+0x0a] likewise
/// NPDE sits at (pcir_data + pcir_hdr_len + 0x0f) & ~0x0f. Keeping the exact
/// sizes matters: the BIT pointer translation base is image[0].size, and a
/// size taken from the wrong field shifts every dereferenced pointer.
pub fn walk_images(rom: &[u8]) -> Vec<RomImage> {
    let mut out = Vec::new();
    let mut base = 0usize;
    for _ in 0..MAX_IMAGES {
        match raw_rd16(rom, base) {
            // 0xaa55 legacy, 0xbb77 NBSI, 0x4e56 'NV': same set nouveau accepts
            Some(0xAA55) | Some(0xBB77) | Some(0x4E56) => {}
            _ => break,
        }
        let pcir_ptr = match raw_rd16(rom, base + 0x18) { Some(p) => p as usize, None => break };
        let pcir = base + pcir_ptr;
        if raw_rd32(rom, pcir) != Some(0x5249_4350) {
            // not 'PCIR'
            break;
        }
        let pcir_hdr_len = raw_rd16(rom, pcir + 0x0a).unwrap_or(0) as usize;
        let mut size = raw_rd16(rom, pcir + 0x10).unwrap_or(0) as usize * 512;
        let image_type = rom.get(pcir + 0x14).copied().unwrap_or(0);
        let mut last = rom.get(pcir + 0x15).map(|b| b & 0x80 != 0).unwrap_or(true);

        // For non-0x70 images nouveau prefers the NPDE size/last if present
        if image_type != 0x70 {
            let npde = (pcir + pcir_hdr_len + 0x0f) & !0x0f;
            if raw_rd32(rom, npde) == Some(0x4544_504e) {
                // 'NPDE'
                size = raw_rd16(rom, npde + 0x08).unwrap_or(0) as usize * 512;
                last = rom.get(npde + 0x0a).map(|b| b & 0x80 != 0).unwrap_or(true);
            }
        } else {
            last = true;
        }

        if size == 0 || base + size > rom.len() {
            out.push(RomImage { base, size: rom.len() - base, image_type, last: true });
            break;
        }
        out.push(RomImage { base, size, image_type, last });
        if last { break; }
        base += size;
    }
    out
}

/// Backwards-compatible debug view: (offset, size, code_type, last)
pub fn image_map(rom: &[u8]) -> Vec<(usize, usize, u8, bool)> {
    walk_images(rom)
        .into_iter()
        .map(|i| (i.base, i.size, i.image_type, i.last))
        .collect()
}

/// A read-through view over the raw expansion ROM that reproduces nouveau's
/// 'nvbios_addr;' pointer translation. NVIDIA BIT pointers live in a virtual
/// space where [0, image0_size) maps to the legacy image and everything at
/// or beyond image0_size maps into the data image (PCIR type 0xe0) located
/// at 'imaged_addr' in the raw ROM. Without this remap, any pointer past the
/// legacy image (e.g. the PMU table) dereferences into padding/garbage.
#[derive(Clone, Copy)]
pub struct VbiosView<'a> {
    pub rom: &'a [u8],
    pub image0_size: usize,
    pub imaged_addr: usize,
}

impl<'a> VbiosView<'a> {
    pub fn new(rom: &'a [u8]) -> Self {
        let images = walk_images(rom);
        let image0_size = images.first().map(|i| i.size).unwrap_or(rom.len());
        let imaged_addr = images
            .iter()
            .find(|i| i.image_type == 0xe0)
            .map(|i| i.base)
            .unwrap_or(0);
        VbiosView { rom, image0_size, imaged_addr }
    }

    /// Translate a virtual VBIOS address to a raw ROM offset
    #[inline]
    pub fn phys(&self, addr: usize) -> usize {
        if addr >= self.image0_size && self.imaged_addr != 0 {
            addr - self.image0_size + self.imaged_addr
        } else {
            addr
        }
    }

    #[inline]
    pub fn rd8(&self, addr: usize) -> Option<u8> {
        self.rom.get(self.phys(addr)).copied()
    }
    #[inline]
    pub fn rd16(&self, addr: usize) -> Option<u16> {
        raw_rd16(self.rom, self.phys(addr))
    }
    #[inline]
    pub fn rd32(&self, addr: usize) -> Option<u32> {
        raw_rd32(self.rom, self.phys(addr))
    }

    /// Borrow 'len' contiguous bytes starting at virtual 'addr'. Valid only
    /// when the whole range lives inside one image (true for the FWSEC
    /// descriptor + ucode, which sit together in the data image)
    pub fn slice(&self, addr: usize, len: usize) -> Option<&'a [u8]> {
        let start = self.phys(addr);
        let end = start.checked_add(len)?;
        if end > self.rom.len() { return None; }
        Some(&self.rom[start..end])
    }
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
