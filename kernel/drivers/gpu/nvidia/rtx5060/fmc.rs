// GSP-FMC (Firmware Management Chain) staging + NVDM_PAYLOAD_COT
//
// On Blackwell the host never runs code on a GPU falcon. Instead it hands
// the FSP one "chain of trust" message describing:
//     where the GSP-FMC image sits in sysmem,
//     where the FRTS region is to be carved (frtsVidmemOffset counts back from the END of FB: 1 MiB region, offset 2 MiB-aligned), the FMC's hash / public key / signature (from the FMC ELF),
//     where the GSP_FMC_BOOT_PARAMS block sits in sysmem.
// The FSP verifies the FMC, the FMC sets up WPR2, loads GSP-RM from the
// radix3-mapped sysmem image and starts the GSP RISC-V core. From then on
// it is the same CMDQ/MSGQ world as Turing
//
// Struct layout is byte-identical to nouveau's NVDM_PAYLOAD_COT
// (nvkm/subdev/fsp/gh100.c, #pragma pack(1)). Blackwell parameters come
// from fsp/gb202.c: COT version 2, hash 48 B, pubkey 97 B, signature 96 B.
// The pubkey/signature fields are fixed 384-byte containers; only the
// leading size_pkey/size_sig bytes are meaningful
//
// The FMC image ships in linux-firmware as an ELF32 (nouveau gsp/gh100.c
// validates it against a fixed elf32_hdr template) whose sections are
// literally named "image", "hash", "signature", "publickey"

use crate::serial_println;

pub const COT_VERSION_BLACKWELL: u16 = 2;
pub const COT_HASH_SIZE: usize = 48;
pub const COT_PKEY_SIZE_GB20X: usize = 97;
pub const COT_SIG_SIZE_GB20X: usize = 96;

/// NVDM_PAYLOAD_COT, packed, 860 bytes. Sent as the payload of an
/// NVDM_TYPE_COT message
#[repr(C, packed)]
pub struct NvdmPayloadCot {
    pub version: u16,
    pub size: u16,
    pub gsp_fmc_sysmem_offset: u64,
    pub frts_sysmem_offset: u64,
    pub frts_sysmem_size: u32,
    pub frts_vidmem_offset: u64,
    pub frts_vidmem_size: u32,
    pub hash384: [u8; 48],
    pub public_key: [u8; 384],
    pub signature: [u8; 384],
    pub gsp_boot_args_sysmem_offset: u64,
}

const COT_PAYLOAD_SIZE: usize = 860;
const _: () = assert!(core::mem::size_of::<NvdmPayloadCot>() == COT_PAYLOAD_SIZE);

impl NvdmPayloadCot {
    /// Build the Blackwell (v2) COT payload. FRTS lives in vidmem only:
    /// frts_sysmem_ stay zero (nouveau passes 0/0 on GH100+ too)
    /// 'frts_vidmem_offset' is the distance back from the end of FB
    /// (gh100_fsp_boot_gsp_fmc: ALIGN(rsvd_size, 0x200000))
    pub fn new_gb20x(
        gsp_fmc_sysmem: u64,
        frts_vidmem_offset: u64,
        frts_vidmem_size: u32,
        fmc: &FmcImage,
        gsp_boot_args_sysmem: u64,
    ) -> Self {
        let mut p = Self {
            version: COT_VERSION_BLACKWELL,
            size: COT_PAYLOAD_SIZE as u16,
            gsp_fmc_sysmem_offset: gsp_fmc_sysmem,
            frts_sysmem_offset: 0,
            frts_sysmem_size: 0,
            frts_vidmem_offset,
            frts_vidmem_size,
            hash384: [0; 48],
            public_key: [0; 384],
            signature: [0; 384],
            gsp_boot_args_sysmem_offset: gsp_boot_args_sysmem,
        };
        p.hash384[..fmc.hash.len().min(48)].copy_from_slice(&fmc.hash[..fmc.hash.len().min(48)]);
        p.public_key[..fmc.public_key.len().min(384)]
            .copy_from_slice(&fmc.public_key[..fmc.public_key.len().min(384)]);
        p.signature[..fmc.signature.len().min(384)]
            .copy_from_slice(&fmc.signature[..fmc.signature.len().min(384)]);
        p
    }

    /// View the payload as little-endian u32 words for the EMEM PIO writer
    pub fn as_words(&self) -> [u32; COT_PAYLOAD_SIZE / 4] {
        let bytes = unsafe {
            core::slice::from_raw_parts(self as *const _ as *const u8, COT_PAYLOAD_SIZE)
        };
        let mut words = [0u32; COT_PAYLOAD_SIZE / 4];
        for (i, w) in words.iter_mut().enumerate() {
            *w = u32::from_le_bytes([
                bytes[i * 4], bytes[i * 4 + 1], bytes[i * 4 + 2], bytes[i * 4 + 3],
            ]);
        }
        words
    }
}

////////////////////////////////////////////////////////////////////////
//                         FMC ELF                                    //
///////////////////////////////////////////////////////////////////////
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FmcError {
    NotElf,
    Truncated,
    MissingSection(&'static str),
    BadCryptoSizes,
}

/// The four sections extracted from the FMC ELF, borrowed from the raw blob
pub struct FmcImage<'a> {
    pub image: &'a [u8],
    pub hash: &'a [u8],
    pub signature: &'a [u8],
    pub public_key: &'a [u8],
}

impl<'a> FmcImage<'a> {
    /// Parse the FMC ELF and pull out image/hash/signature/publickey.
    /// Minimal ELF32-LE walker (the blob is an elf32 container, unlike the
    /// ELF64 gsp-rm image); no relocation or load logic needed - the FSP
    /// consumes the raw "image" section as-is
    pub fn parse(blob: &'a [u8]) -> Result<Self, FmcError> {
        if blob.len() < 0x34 || &blob[0..4] != b"\x7fELF" || blob[4] != 1 || blob[5] != 1 {
            return Err(FmcError::NotElf);
        }
        let shoff = u32::from_le_bytes(blob[0x20..0x24].try_into().unwrap()) as usize;
        let shentsize = u16::from_le_bytes(blob[0x2E..0x30].try_into().unwrap()) as usize;
        let shnum = u16::from_le_bytes(blob[0x30..0x32].try_into().unwrap()) as usize;
        let shstrndx = u16::from_le_bytes(blob[0x32..0x34].try_into().unwrap()) as usize;
        if shentsize < 0x28 || shstrndx >= shnum {
            return Err(FmcError::Truncated);
        }

        let sh = |i: usize| -> Result<(usize, usize, usize), FmcError> {
            let base = shoff + i * shentsize;
            if base + 0x28 > blob.len() {
                return Err(FmcError::Truncated);
            }
            let name_off = u32::from_le_bytes(blob[base..base + 4].try_into().unwrap()) as usize;
            let off = u32::from_le_bytes(blob[base + 0x10..base + 0x14].try_into().unwrap()) as usize;
            let size = u32::from_le_bytes(blob[base + 0x14..base + 0x18].try_into().unwrap()) as usize;
            Ok((name_off, off, size))
        };

        // Section name string table
        let (_, strtab_off, strtab_size) = sh(shstrndx)?;
        if strtab_off + strtab_size > blob.len() {
            return Err(FmcError::Truncated);
        }
        let strtab = &blob[strtab_off..strtab_off + strtab_size];
        let name_of = |name_off: usize| -> &'a [u8] {
            let s = &strtab[name_off.min(strtab.len())..];
            let end = s.iter().position(|&b| b == 0).unwrap_or(0);
            &s[..end]
        };

        let mut image = None;
        let mut hash = None;
        let mut signature = None;
        let mut public_key = None;

        for i in 0..shnum {
            let (name_off, off, size) = sh(i)?;
            if off + size > blob.len() || size == 0 {
                continue;
            }
            let data = &blob[off..off + size];
            match name_of(name_off) {
                b"image"     => image = Some(data),
                b"hash"      => hash = Some(data),
                b"signature" => signature = Some(data),
                b"publickey" => public_key = Some(data),
                _ => {}
            }
        }

        Ok(Self {
            image: image.ok_or(FmcError::MissingSection("image"))?,
            hash: hash.ok_or(FmcError::MissingSection("hash"))?,
            signature: signature.ok_or(FmcError::MissingSection("signature"))?,
            public_key: public_key.ok_or(FmcError::MissingSection("publickey"))?,
        })
    }

    /// Enforce the GB20x crypto material sizes (fsp/gb202.c: 48/97/96)
    /// A mismatch means the blob is for another chip or another COT version
    pub fn verify_gb20x(&self) -> Result<(), FmcError> {
        if self.hash.len() == COT_HASH_SIZE
            && self.public_key.len() == COT_PKEY_SIZE_GB20X
            && self.signature.len() == COT_SIG_SIZE_GB20X
        {
            Ok(())
        } else {
            serial_println!(
                "[gb206/fmc] crypto sizes hash={} pkey={} sig={} (want {}/{}/{})",
                self.hash.len(), self.public_key.len(), self.signature.len(),
                COT_HASH_SIZE, COT_PKEY_SIZE_GB20X, COT_SIG_SIZE_GB20X
            );
            Err(FmcError::BadCryptoSizes)
        }
    }

    pub fn log(&self) {
        serial_println!(
            "[gb206/fmc] image={} B hash={} B pubkey={} B sig={} B",
            self.image.len(), self.hash.len(), self.public_key.len(), self.signature.len()
        );
    }
}

/// Compile-time layout self-test callable from the shell: dumps the struct
/// size and a payload built from dummy data
pub fn self_test() {
    serial_println!(
        "[gb206/fmc] NVDM_PAYLOAD_COT size={} (expect {})",
        core::mem::size_of::<NvdmPayloadCot>(), COT_PAYLOAD_SIZE
    );
    let dummy = FmcImage {
        image: &[0u8; 16],
        hash: &[0xAA; COT_HASH_SIZE],
        signature: &[0xBB; COT_SIG_SIZE_GB20X],
        public_key: &[0xCC; COT_PKEY_SIZE_GB20X],
    };
    let p = NvdmPayloadCot::new_gb20x(0x1000_0000, 0x3_FFE0_0000, 0x10_0000, &dummy, 0x2000_0000);
    let words = p.as_words();
    serial_println!(
        "[gb206/fmc] payload[0]={:#010x} (version|size) words={} frts_vid={:#x}+{:#x}",
        words[0], words.len(),
        { p.frts_vidmem_offset }, { p.frts_vidmem_size },
    );
}
