use core::ptr::addr_of;
use core::sync::atomic::{AtomicU64, Ordering};

pub const HHDM_OFFSET: u64 = 0xFFFF800000000000;
pub const KERNEL_VMA:  u64 = 0xFFFFFFFF80000000;
pub const KERNEL_PHYS: u64 = 0x0;

static MB2_VIRT: AtomicU64 = AtomicU64::new(0);

pub static HHDM:               AtomicU64 = AtomicU64::new(HHDM_OFFSET);
static KERNEL_VIRT_OFFSET:     AtomicU64 = AtomicU64::new(KERNEL_VMA);
static KERNEL_PHYS_BASE:       AtomicU64 = AtomicU64::new(KERNEL_PHYS);

pub fn init(mb2_phys: u64) {
    MB2_VIRT.store(mb2_phys + HHDM_OFFSET, Ordering::Relaxed);
}

pub fn set_kernel_address(virt_base: u64, phys_base: u64) {
    KERNEL_VIRT_OFFSET.store(virt_base, Ordering::Relaxed);
    KERNEL_PHYS_BASE.store(phys_base, Ordering::Relaxed);
}

pub fn hhdm() -> u64 {
    HHDM.load(Ordering::Relaxed)
}

pub fn phys_to_virt(phys: u64) -> u64 {
    phys + HHDM.load(Ordering::Relaxed)
}

pub fn virt_to_phys(virt: u64) -> u64 {
    virt - KERNEL_VIRT_OFFSET.load(Ordering::Relaxed)
        + KERNEL_PHYS_BASE.load(Ordering::Relaxed)
}

pub fn any_virt_to_phys(virt: u64) -> u64 {
    let hhdm = HHDM.load(Ordering::Relaxed);
    if virt >= 0xFFFFFFFF80000000 {
        virt - KERNEL_VIRT_OFFSET.load(Ordering::Relaxed) + KERNEL_PHYS_BASE.load(Ordering::Relaxed)
    } else if virt >= hhdm {
        virt - hhdm
    } else {
        panic!("any_virt_to_phys: unknown address {:#x}", virt);
    }
}

fn mb2_virt() -> u64 {
    MB2_VIRT.load(Ordering::Relaxed)
}

const TAG_MMAP:   u32 = 6;
const TAG_FB:     u32 = 8;
const TAG_END:    u32 = 0;
const TAG_MODULE: u32 = 3;
// EFI tags exist only when GRUB itself ran as an EFI application, i.e. the
// firmware POSTed in UEFI mode (GPU option ROM executed as a GOP driver).
// All of them are absent on a CSM/legacy boot
const TAG_EFI32_ST: u32 = 11;
const TAG_EFI64_ST: u32 = 12;
const TAG_EFI_MMAP: u32 = 17;
const TAG_EFI32_IH: u32 = 19;
const TAG_EFI64_IH: u32 = 20;

/// True if the kernel was handed off from an EFI GRUB (UEFI boot); false
/// means BIOS/CSM legacy boot
pub fn booted_via_uefi() -> bool {
    find_tag(TAG_EFI64_ST).is_some()
        || find_tag(TAG_EFI32_ST).is_some()
        || find_tag(TAG_EFI_MMAP).is_some()
        || find_tag(TAG_EFI64_IH).is_some()
        || find_tag(TAG_EFI32_IH).is_some()
}

pub const MMAP_USABLE:   u32 = 1;
pub const MMAP_RESERVED: u32 = 2;
pub const MMAP_ACPI_RC:  u32 = 3;
pub const MMAP_ACPI_NVS: u32 = 4;
pub const MMAP_BAD:      u32 = 5;

#[repr(C, packed)]
struct Mb2Header {
    total_size: u32,
    _reserved:  u32,
}

#[repr(C, packed)]
struct TagHeader {
    tag_type: u32,
    size:     u32,
}

#[repr(C, packed)]
struct MmapTagHeader {
    _tag_type:  u32,
    _size:      u32,
    entry_size: u32,
    _entry_ver: u32,
}

#[repr(C, packed)]
pub struct MmapEntry {
    base:      u64,
    length:    u64,
    mem_type:  u32,
    _reserved: u32,
}

impl MmapEntry {
    pub fn base(&self) -> u64 {
        unsafe { addr_of!(self.base).read_unaligned() }
    }
    pub fn length(&self) -> u64 {
        unsafe { addr_of!(self.length).read_unaligned() }
    }
    pub fn mem_type(&self) -> u32 {
        unsafe { addr_of!(self.mem_type).read_unaligned() }
    }
}

#[repr(C, packed)]
struct FbTagRaw {
    _tag_type: u32,
    _size:     u32,
    addr:      u64,
    pitch:     u32,
    width:     u32,
    height:    u32,
    bpp:       u8,
    fb_type:   u8,
    _reserved: u16,
}

unsafe fn read_tag_type(ptr: u64) -> u32 {
    addr_of!((*(ptr as *const TagHeader)).tag_type).read_unaligned()
}

unsafe fn read_tag_size(ptr: u64) -> u32 {
    addr_of!((*(ptr as *const TagHeader)).size).read_unaligned()
}

fn find_tag(tag_type: u32) -> Option<u64> {
    let base = mb2_virt();
    if base == HHDM_OFFSET {
        return None;
    }
    let total = unsafe {
        addr_of!((*(base as *const Mb2Header)).total_size).read_unaligned()
    } as u64;
    let end = base + total;
    let mut ptr = base + 8;
    while ptr + 8 <= end {
        let t = unsafe { read_tag_type(ptr) };
        let s = unsafe { read_tag_size(ptr) } as u64;
        if t == TAG_END || s == 0 {
            break;
        }
        if t == tag_type {
            return Some(ptr);
        }
        ptr = (ptr + s + 7) & !7;
    }
    None
}

#[repr(C, packed)]
struct ModuleTagHeader {
    _tag_type: u32,
    _size:     u32,
    mod_start: u32,
    mod_end:   u32,
    // null-terminated module string follows
}

/// Locate a multiboot2 module ('module2 <file> <string>' in grub.cfg) whose
/// command-line string contains 'name'. Returns its (start_phys, end_phys).
/// GRUB loads module images into reclaimable RAM before the kernel runs, so
/// the span is reachable through the HHDM. 'find_tag' only returns the first
/// tag of a type, so we walk the tag list directly to match the string
pub fn module(name: &str) -> Option<(u64, u64)> {
    let base = mb2_virt();
    if base == HHDM_OFFSET {
        return None;
    }
    let total = unsafe {
        addr_of!((*(base as *const Mb2Header)).total_size).read_unaligned()
    } as u64;
    let end = base + total;
    let mut ptr = base + 8;
    while ptr + 8 <= end {
        let t = unsafe { read_tag_type(ptr) };
        let s = unsafe { read_tag_size(ptr) } as u64;
        if t == TAG_END || s == 0 {
            break;
        }
        if t == TAG_MODULE && s >= 16 {
            let h = ptr as *const ModuleTagHeader;
            let mod_start = unsafe { addr_of!((*h).mod_start).read_unaligned() } as u64;
            let mod_end = unsafe { addr_of!((*h).mod_end).read_unaligned() } as u64;
            if module_str_contains(ptr + 16, ptr + s, name) && mod_end > mod_start {
                return Some((mod_start, mod_end));
            }
        }
        ptr = (ptr + s + 7) & !7;
    }
    None
}

/// True if the null-terminated module string in [str_ptr, limit) contains
/// needle as a byte substring. Reads one byte at a time and stops at the
/// NUL or the tag boundary, so a malformed (unterminated) string is safe
fn module_str_contains(str_ptr: u64, limit: u64, needle: &str) -> bool {
    let nb = needle.as_bytes();
    if nb.is_empty() {
        return true;
    }
    let mut buf = [0u8; 128];
    let mut len = 0usize;
    let mut p = str_ptr;
    while p < limit && len < buf.len() {
        let c = unsafe { *(p as *const u8) };
        if c == 0 {
            break;
        }
        buf[len] = c;
        len += 1;
        p += 1;
    }
    buf[..len].windows(nb.len()).any(|w| w == nb)
}

pub struct MemMapIter {
    ptr:        u64,
    end:        u64,
    entry_size: u32,
}

impl Iterator for MemMapIter {
    type Item = &'static MmapEntry;

    fn next(&mut self) -> Option<Self::Item> {
        if self.ptr + self.entry_size as u64 > self.end {
            return None;
        }
        let entry = unsafe { &*(self.ptr as *const MmapEntry) };
        self.ptr += self.entry_size as u64;
        Some(entry)
    }
}

pub fn memory_map() -> Option<MemMapIter> {
    let ptr = find_tag(TAG_MMAP)?;
    let entry_size = unsafe {
        addr_of!((*(ptr as *const MmapTagHeader)).entry_size).read_unaligned()
    };
    let tag_size = unsafe { read_tag_size(ptr) } as u64;
    Some(MemMapIter {
        ptr: ptr + 16,
        end: ptr + tag_size,
        entry_size,
    })
}

pub struct FbInfo {
    pub addr:   u64,
    pub pitch:  u32,
    pub width:  u32,
    pub height: u32,
    pub bpp:    u8,
}

pub fn framebuffer() -> Option<FbInfo> {
    let ptr = find_tag(TAG_FB)?;
    let raw = ptr as *const FbTagRaw;
    let addr   = unsafe { addr_of!((*raw).addr).read_unaligned() };
    let pitch  = unsafe { addr_of!((*raw).pitch).read_unaligned() };
    let width  = unsafe { addr_of!((*raw).width).read_unaligned() };
    let height = unsafe { addr_of!((*raw).height).read_unaligned() };
    let bpp    = unsafe { addr_of!((*raw).bpp).read_unaligned() };
    let fbt    = unsafe { addr_of!((*raw).fb_type).read_unaligned() };
    if fbt != 1 {
        return None;
    }
    Some(FbInfo { addr, pitch, width, height, bpp })
}

pub fn mmap_type_str(t: u32) -> &'static str {
    match t {
        MMAP_USABLE   => "USABLE  ",
        MMAP_RESERVED => "RESERVED",
        MMAP_ACPI_RC  => "ACPI_RC ",
        MMAP_ACPI_NVS => "ACPI_NVS",
        MMAP_BAD      => "BAD     ",
        _             => "UNKNOWN ",
    }
}

pub fn mmap_type_color(t: u32) -> (u8, u8, u8) {
    match t {
        MMAP_USABLE   => (100, 220, 150),
        MMAP_RESERVED => (180, 100, 100),
        MMAP_ACPI_RC  => (200, 160, 80),
        MMAP_ACPI_NVS => (180, 120, 60),
        _             => (160, 160, 160),
    }
}
