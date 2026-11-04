extern crate alloc;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use alloc::sync::Arc;
use spin::Mutex;
use crate::process::elf::{self, PF_W, PF_X};
use crate::mm::pmm;
use crate::arch::x86_64::grub;
use crate::mm::vmm::AddressSpace;
use x86_64::structures::paging::PageTableFlags;

const MAX_CACHED_LIBS: usize = 32;
const MAX_SEARCH_PATHS: usize = 4;
const MAX_NAME: usize = 64;
const PAGE_SIZE: u64 = 4096;

struct SharedSegment {
    vaddr_start: u64,
    num_pages: usize,
    pflags: u32,
    frames: Vec<u64>,
    writable: bool,
}

struct CachedLib {
    name: [u8; MAX_NAME],
    name_len: usize,
    data: Arc<Vec<u8>>,
    load_count: u32,
    segments: Vec<SharedSegment>,
    total_map_pages: usize,
    lo_vaddr: u64,
    parsed: bool,
}

impl CachedLib {
    fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
    }

    fn matches(&self, soname: &str) -> bool {
        self.name_len == soname.len() && &self.name[..self.name_len] == soname.as_bytes()
    }

    /// None when the name will not fit: a truncated entry's `matches` could
    /// never be true
    fn new(soname: &str, data: Vec<u8>) -> Option<Self> {
        if soname.len() > MAX_NAME {
            return None;
        }
        let mut name = [0u8; MAX_NAME];
        let nlen = soname.len();
        name[..nlen].copy_from_slice(soname.as_bytes());
        Some(Self {
            name,
            name_len: nlen,
            data: Arc::new(data),
            load_count: 0,
            segments: Vec::new(),
            total_map_pages: 0,
            lo_vaddr: 0,
            parsed: false,
        })
    }
}

impl Drop for CachedLib {
    fn drop(&mut self) {
        for seg in &self.segments {
            for &frame in &seg.frames {
                pmm::free_frame(frame);
            }
        }
    }
}

struct SoLibManager {
    libs: Vec<CachedLib>,
    search_paths: [&'static str; MAX_SEARCH_PATHS],
    path_count: usize,
}

impl SoLibManager {
    const fn new() -> Self {
        Self {
            libs: Vec::new(),
            search_paths: ["/lib", "/usr/lib", "", ""],
            path_count: 2,
        }
    }
}

static MANAGER: Mutex<SoLibManager> = Mutex::new(SoLibManager::new());

/// A shared-object name is a file name, never a path. It comes from ring 3
/// (sys_map_lib) and is concatenated onto a search directory, so '/' or ".."
/// would walk the lookup out of /lib
pub fn soname_ok(soname: &str) -> bool {
    if soname.is_empty() || soname.len() > MAX_NAME {
        return false;
    }
    if soname == "." || soname == ".." || soname.contains('/') {
        return false;
    }
    soname.bytes().all(|b| {
        b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-' || b == b'+'
    })
}

pub fn init() {
    let mgr = MANAGER.lock();
    crate::serial_println!(
        "[solib] initialized, paths: {}",
        mgr.search_paths[..mgr.path_count].join(", ")
    );
}

pub fn add_search_path(path: &'static str) {
    let mut mgr = MANAGER.lock();
    let idx = mgr.path_count;
    if idx < MAX_SEARCH_PATHS {
        mgr.search_paths[idx] = path;
        mgr.path_count += 1;
    }
}

pub fn preload(soname: &str, data: Vec<u8>) {
    if !soname_ok(soname) {
        crate::serial_println!("[solib] refusing to cache a bad soname");
        return;
    }
    let mut mgr = MANAGER.lock();
    if mgr.libs.len() >= MAX_CACHED_LIBS {
        return;
    }
    for lib in mgr.libs.iter() {
        if lib.matches(soname) {
            return;
        }
    }
    let size = data.len();
    if let Some(lib) = CachedLib::new(soname, data) {
        mgr.libs.push(lib);
        crate::trace_println!("[solib] preloaded '{}' ({} bytes)", soname, size);
    }
}

pub fn resolve(soname: &str) -> Option<Arc<Vec<u8>>> {
    if !soname_ok(soname) {
        crate::serial_println!("[solib] refusing lookup of a bad soname");
        return None;
    }
    let mut mgr = MANAGER.lock();

    for lib in mgr.libs.iter_mut() {
        if lib.matches(soname) {
            lib.load_count += 1;
            crate::kdebug!(
                "[solib] cache hit '{}' ({} bytes, loads={})",
                soname, lib.data.len(), lib.load_count
            );
            return Some(Arc::clone(&lib.data));
        }
    }

    let path_count = mgr.path_count;
    let mut paths: [&str; MAX_SEARCH_PATHS] = [""; MAX_SEARCH_PATHS];
    for i in 0..path_count {
        paths[i] = mgr.search_paths[i];
    }
    drop(mgr);

    for i in 0..path_count {
        let prefix = paths[i];
        let mut full = [0u8; 256];
        let plen = prefix.len();
        let slen = soname.len();
        if plen + 1 + slen >= 256 {
            continue;
        }
        full[..plen].copy_from_slice(prefix.as_bytes());
        full[plen] = b'/';
        full[plen + 1..plen + 1 + slen].copy_from_slice(soname.as_bytes());
        let total = plen + 1 + slen;
        let path_str = core::str::from_utf8(&full[..total]).unwrap_or("");

        if let Some(data) = crate::fs::vfs_read::read_file(path_str) {
            crate::serial_println!(
                "[solib] loaded '{}' from {} ({} bytes)",
                soname, path_str, data.len()
            );
            let mut mgr = MANAGER.lock();
            if mgr.libs.len() < MAX_CACHED_LIBS {
                if let Some(lib) = CachedLib::new(soname, data) {
                    let ret = Arc::clone(&lib.data);
                    mgr.libs.push(lib);
                    return Some(ret);
                }
                return None;
            }
            return Some(Arc::new(data));
        }
    }

    crate::serial_println!("[solib] not found: '{}'", soname);
    None
}

pub fn resolve_path(full_path: &str) -> Option<Arc<Vec<u8>>> {
    let soname = full_path.rsplit('/').next().unwrap_or(full_path);
    let mgr = MANAGER.lock();
    for lib in mgr.libs.iter() {
        if lib.matches(soname) {
            return Some(Arc::clone(&lib.data));
        }
    }
    drop(mgr);
    resolve(soname)
}

/// One page this call mapped, so a failure part-way can put the address space
/// back
struct Placed {
    va:     u64,
    phys:   u64,
    shared: bool,
}

fn undo(aspace: &AddressSpace, placed: &[Placed]) {
    for p in placed {
        aspace.unmap_page_no_free(p.va);
        if p.shared {
            pmm::ref_dec(p.phys);
        } else {
            pmm::free_frame(p.phys);
        }
    }
}

pub fn map_into_process(soname: &str, cr3: u64) -> Result<u64, i64> {
    if !soname_ok(soname) {
        return Err(-22); // EINVAL
    }
    {
        let mgr = MANAGER.lock();
        let found = mgr.libs.iter().any(|l| l.matches(soname));
        drop(mgr);
        if !found {
            if resolve(soname).is_none() {
                return Err(-2); // ENOENT
            }
        }
    }

    let mut mgr = MANAGER.lock();
    let lib_idx = match mgr.libs.iter().position(|l| l.matches(soname)) {
        Some(i) => i,
        None => return Err(-2),
    };

    if !mgr.libs[lib_idx].parsed {
        let data = Arc::clone(&mgr.libs[lib_idx].data);
        parse_and_prepare(&mut mgr.libs[lib_idx], &data);
    }

    let lib = &mut mgr.libs[lib_idx];
    if lib.segments.is_empty() {
        return Err(-22); // EINVAL
    }

    let lo = lib.lo_vaddr;
    let total_pages = lib.total_map_pages;
    let map_size = match (total_pages as u64).checked_mul(PAGE_SIZE) {
        Some(v) if v > 0 => v,
        _ => return Err(-12),
    };
    let base_va = match crate::mm::mmap::kernel_find_free(cr3, map_size) {
        Some(v) => v,
        None => return Err(-12), // ENOMEM
    };
    let win_end = match base_va.checked_add(map_size) {
        Some(v) => v,
        None    => return Err(-12),
    };

    let aspace = AddressSpace::from_raw(cr3);
    let hhdm = grub::hhdm();
    let bias = base_va.wrapping_sub(lo);

    // Every page this call places, in order, so a failure can be undone
    let mut placed: Vec<Placed> = Vec::new();

    // Anything that lands outside the window kernel_find_free reserved is
    // a bug in the library's headers, not a page we may map. The address
    // arithmetic below is all checked for the same reason
    let mut fail = |aspace: &AddressSpace, placed: &[Placed], code: i64| -> i64 {
        undo(aspace, placed);
        code
    };

    for seg in lib.segments.iter() {
        // W^X, the same rule the main loader enforces
        if seg.pflags & elf::PF_W != 0 && seg.pflags & elf::PF_X != 0 {
            crate::serial_println!("[solib] '{}' has a W+X segment - refused", soname);
            let e = fail(&aspace, &placed, -22);
            let _ = aspace.into_raw();
            return Err(e);
        }

        let seg_va_start = match seg.vaddr_start.checked_add(bias) {
            Some(v) => v,
            None    => {
                let e = fail(&aspace, &placed, -12);
                let _ = aspace.into_raw();
                return Err(e);
            }
        };

        let mut flags = PageTableFlags::USER_ACCESSIBLE;
        if seg.pflags & elf::PF_X == 0 {
            flags |= PageTableFlags::NO_EXECUTE;
        }
        let share = !seg.writable && !seg.frames.is_empty();
        if !share {
            flags |= PageTableFlags::WRITABLE;
        }

        for i in 0..seg.num_pages {
            let va = match (i as u64)
                .checked_mul(PAGE_SIZE)
                .and_then(|off| seg_va_start.checked_add(off))
            {
                Some(v) => v,
                None    => {
                    let e = fail(&aspace, &placed, -12);
                    let _ = aspace.into_raw();
                    return Err(e);
                }
            };
            if va < base_va || va.saturating_add(PAGE_SIZE) > win_end {
                crate::serial_println!(
                    "[solib] '{}' page {:#x} falls outside its own window", soname, va,
                );
                let e = fail(&aspace, &placed, -22);
                let _ = aspace.into_raw();
                return Err(e);
            }

            let (phys, shared) = if share {
                match seg.frames.get(i) {
                    Some(&f) => { pmm::ref_inc(f); (f, true) }
                    None     => break,
                }
            } else {
                let frame = match pmm::alloc_frame() {
                    Some(f) => f,
                    None => {
                        let e = fail(&aspace, &placed, -12);
                        let _ = aspace.into_raw();
                        return Err(e);
                    }
                };
                unsafe {
                    // Zero first: a private page beyond the cached image
                    // must not expose whatever the frame held before
                    core::ptr::write_bytes((frame + hhdm) as *mut u8, 0, PAGE_SIZE as usize);
                    if let Some(&src) = seg.frames.get(i) {
                        core::ptr::copy_nonoverlapping(
                            (src + hhdm) as *const u8,
                            (frame + hhdm) as *mut u8,
                            PAGE_SIZE as usize,
                        );
                    }
                }
                (frame, false)
            };

            if !aspace.map_page(va, phys, flags) {
                if shared { pmm::ref_dec(phys); } else { pmm::free_frame(phys); }
                let e = fail(&aspace, &placed, -12);
                let _ = aspace.into_raw();
                return Err(e);
            }
            placed.push(Placed { va, phys, shared });
        }
    }

    lib.load_count += 1;

    crate::mm::mmap::kernel_register_vma(cr3, base_va, win_end, 5);

    let _ = aspace.into_raw();

    let shared_pages: usize = lib.segments.iter().filter(|s| !s.writable).map(|s| s.num_pages).sum();
    let private_pages: usize = lib.segments.iter().filter(|s| s.writable).map(|s| s.num_pages).sum();

    // Into the log, not down the wire: ten of these a `std` program, and a line
    // on the UART is milliseconds with interrupts off (`dev/kernel-core.md`,
    // "Bottlenecks round")
    crate::kdebug!(
        "[solib] mapped '{}' at {:#x} (shared={} private={} pages)",
        soname, base_va, shared_pages, private_pages
    );

    Ok(base_va)
}

/// Populates the shared frame set for a cached library, once.
///
/// The mapping hands the caller `base_va` and expects the ELF header to be
/// readable there: that is how ld-miku finds the phdrs of a library it did not
/// load. The first PT_LOAD of a normal shared object starts at file offset 0
/// and contains the header; when it does not, this declines rather than map a
/// second frame at the same address
fn parse_and_prepare(lib: &mut CachedLib, data: &[u8]) {
    let info = match elf::parse(data) {
        Ok(i) => i,
        Err(e) => {
            crate::serial_println!("[solib] '{}' rejected: {}", lib.name_str(), e.as_str());
            return;
        }
    };

    let hhdm = grub::hhdm();
    let (mut lo, mut hi) = info.memory_bounds();
    if lo == u64::MAX {
        return;
    }

    // Same ceiling the process loader applies. This path allocates every frame
    // up front, before anybody has asked for the mapping
    if info.total_mem > elf::MAX_IMAGE_MEM {
        crate::serial_println!("[solib] '{}' asks for too much memory", lib.name_str());
        return;
    }

    let mut segments: Vec<SharedSegment> = Vec::new();
    let mut header_covered = false;

    for phdr in info.load_segments() {
        if phdr.p_memsz == 0 {
            continue;
        }

        // W^X, decided here so a library that violates it never gets
        // frames allocated for it at all
        if phdr.p_flags & PF_W != 0 && phdr.p_flags & PF_X != 0 {
            crate::serial_println!("[solib] '{}' has a W+X segment - refused", lib.name_str());
            for seg in &segments {
                for &f in &seg.frames { pmm::free_frame(f); }
            }
            return;
        }

        if phdr.p_offset == 0 && phdr.p_filesz >= 64 {
            header_covered = true;
        }

        let writable = phdr.p_flags & PF_W != 0;
        let page_start = phdr.p_vaddr & !0xFFF;
        // elf::parse already rejects p_vaddr+p_memsz wrap, but solib loads some
        // files that bypass that path
        let vaddr_end = match phdr.p_vaddr.checked_add(phdr.p_memsz) {
            Some(v) => v,
            None    => continue,
        };
        let page_end = match vaddr_end.checked_add(0xFFF) {
            Some(v) => v & !0xFFF,
            None    => continue,
        };
        if page_end <= page_start { continue; }
        let num_pages = ((page_end - page_start) / PAGE_SIZE) as usize;

        let mut frames = Vec::with_capacity(num_pages);
        let mut oom = false;
        for pg in 0..num_pages {
            let frame = match pmm::alloc_frame() {
                Some(f) => f,
                None => { oom = true; break; },
            };
            unsafe {
                core::ptr::write_bytes((frame + hhdm) as *mut u8, 0, PAGE_SIZE as usize);
            }
            let page_va = page_start + (pg as u64) * PAGE_SIZE;
            let copy_start = page_va.max(phdr.p_vaddr);
            let seg_file_end = phdr.p_vaddr.saturating_add(phdr.p_filesz);
            let page_va_end  = page_va.saturating_add(PAGE_SIZE);
            let copy_end = page_va_end.min(seg_file_end);
            if copy_end > copy_start {
                let dst_off = (copy_start - page_va) as usize;
                let src_off = match (phdr.p_offset as usize)
                    .checked_add((copy_start - phdr.p_vaddr) as usize)
                {
                    Some(v) => v,
                    None    => { frames.push(frame); continue; },
                };
                let len = (copy_end - copy_start) as usize;
                let in_bounds = src_off
                    .checked_add(len)
                    .map(|e| e <= data.len())
                    .unwrap_or(false);
                if in_bounds {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            data.as_ptr().add(src_off),
                            (frame + hhdm + dst_off as u64) as *mut u8,
                            len,
                        );
                    }
                }
            }
            frames.push(frame);
        }

        if oom {
            for &f in &frames { pmm::free_frame(f); }
            for seg in &segments {
                for &f in &seg.frames { pmm::free_frame(f); }
            }
            crate::serial_println!("[solib] OOM in parse_and_prepare, aborting");
            return;
        }

        segments.push(SharedSegment {
            vaddr_start: page_start,
            num_pages,
            pflags: phdr.p_flags,
            frames,
            writable,
        });
    }

    lo &= !0xFFF;
    hi = hi.saturating_add(0xFFF) & !0xFFF;
    if hi <= lo || segments.is_empty() {
        for seg in &segments {
            for &f in &seg.frames { pmm::free_frame(f); }
        }
        return;
    }

    if !header_covered {
        crate::serial_println!(
            "[solib] '{}': first PT_LOAD does not cover the ELF header - not shareable",
            lib.name_str(),
        );
        for seg in &segments {
            for &f in &seg.frames { pmm::free_frame(f); }
        }
        return;
    }

    lib.segments = segments;
    lib.total_map_pages = ((hi - lo) / PAGE_SIZE) as usize;
    lib.lo_vaddr = lo;
    lib.parsed = true;

    crate::kdebug!(
        "[solib] prepared '{}': {} segs, {} pages",
        lib.name_str(), lib.segments.len(), lib.total_map_pages
    );
}

pub fn ldconfig() {
    let mgr = MANAGER.lock();
    let path_count = mgr.path_count;
    let mut paths: [&str; MAX_SEARCH_PATHS] = [""; MAX_SEARCH_PATHS];
    for i in 0..path_count {
        paths[i] = mgr.search_paths[i];
    }
    drop(mgr);

    let mut found = 0u32;
    for i in 0..path_count {
        let dir = paths[i];
        if dir.is_empty() {
            continue;
        }
        let libs = scan_dir_for_libs(dir);
        for lib_name in &libs {
            if lib_name.is_empty() {
                continue;
            }
            let mut full = [0u8; 256];
            let dlen = dir.len();
            let nlen = lib_name.len();
            if dlen + 1 + nlen >= 256 {
                continue;
            }
            full[..dlen].copy_from_slice(dir.as_bytes());
            full[dlen] = b'/';
            full[dlen + 1..dlen + 1 + nlen].copy_from_slice(lib_name.as_bytes());
            let total = dlen + 1 + nlen;
            let path_str = core::str::from_utf8(&full[..total]).unwrap_or("");

            {
                let mgr = MANAGER.lock();
                if mgr.libs.iter().any(|l| l.matches(lib_name)) {
                    continue;
                }
            }

            if let Some(data) = crate::fs::vfs_read::read_file(path_str) {
                crate::serial_println!("[solib] ldconfig: '{}' ({} bytes)", lib_name, data.len());
                preload(lib_name, data);
                found += 1;
            }
        }
    }
    crate::serial_println!("[solib] ldconfig: {} libraries cached", found);
}

fn scan_dir_for_libs(dir: &str) -> Vec<String> {
    crate::fs::vfs::core::with_vfs(|vfs| -> Vec<String> {
        let mut libs = Vec::new();
        let dir_id = match vfs.resolve_path(0, dir) {
            Ok(id) => id,
            Err(_) => return libs,
        };
        let mut entries = [crate::fs::vfs::types::DirEntry::empty(); 32];
        let count = vfs.readdir(dir_id, &mut entries).unwrap_or(0);
        for i in 0..count {
            let name = entries[i].get_name();
            if name.ends_with(".so") || name.contains(".so.") {
                libs.push(String::from(name));
            }
        }
        libs
    })
}

pub fn list() -> Vec<(String, usize, u32, bool)> {
    let mgr = MANAGER.lock();
    mgr.libs.iter().map(|lib| {
        (String::from(lib.name_str()), lib.data.len(), lib.load_count, lib.parsed)
    }).collect()
}

pub fn stats() -> (usize, usize) {
    let mgr = MANAGER.lock();
    let count = mgr.libs.len();
    let bytes: usize = mgr.libs.iter().map(|l| l.data.len()).sum();
    (count, bytes)
}

pub fn invalidate(soname: &str) {
    let mut mgr = MANAGER.lock();
    mgr.libs.retain(|lib| !lib.matches(soname));
}

pub fn flush_all() {
    let mut mgr = MANAGER.lock();
    mgr.libs.clear();
}
