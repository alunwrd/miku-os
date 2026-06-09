extern crate alloc;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use spin::Mutex;
use crate::elf::{self, PT_LOAD, PF_W};
use crate::pmm;
use crate::grub;
use crate::vmm::AddressSpace;
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
    data: Vec<u8>,
    load_count: u32,
    segments: Vec<SharedSegment>,
    elf_header_frame: u64,
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

    fn new(soname: &str, data: Vec<u8>) -> Self {
        let mut name = [0u8; MAX_NAME];
        let nlen = soname.len().min(MAX_NAME);
        name[..nlen].copy_from_slice(&soname.as_bytes()[..nlen]);
        Self {
            name,
            name_len: nlen,
            data,
            load_count: 0,
            segments: Vec::new(),
            elf_header_frame: 0,
            total_map_pages: 0,
            lo_vaddr: 0,
            parsed: false,
        }
    }
}

impl Drop for CachedLib {
    fn drop(&mut self) {
        for seg in &self.segments {
            for &frame in &seg.frames {
                pmm::free_frame(frame);
            }
        }
        if self.elf_header_frame != 0 {
            pmm::free_frame(self.elf_header_frame);
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
    mgr.libs.push(CachedLib::new(soname, data));
    crate::serial_println!("[solib] preloaded '{}' ({} bytes)", soname, size);
}

pub fn resolve(soname: &str) -> Option<Vec<u8>> {
    let mut mgr = MANAGER.lock();

    for lib in mgr.libs.iter_mut() {
        if lib.matches(soname) {
            lib.load_count += 1;
            crate::serial_println!(
                "[solib] cache hit '{}' ({} bytes, loads={})",
                soname, lib.data.len(), lib.load_count
            );
            return Some(lib.data.clone());
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

        if let Some(data) = crate::vfs_read::read_file(path_str) {
            crate::serial_println!(
                "[solib] loaded '{}' from {} ({} bytes)",
                soname, path_str, data.len()
            );
            let mut mgr = MANAGER.lock();
            if mgr.libs.len() < MAX_CACHED_LIBS {
                let ret = data.clone();
                mgr.libs.push(CachedLib::new(soname, data));
                return Some(ret);
            }
            return Some(data);
        }
    }

    crate::serial_println!("[solib] not found: '{}'", soname);
    None
}

pub fn resolve_path(full_path: &str) -> Option<Vec<u8>> {
    let soname = full_path.rsplit('/').next().unwrap_or(full_path);
    let mgr = MANAGER.lock();
    for lib in mgr.libs.iter() {
        if lib.matches(soname) {
            return Some(lib.data.clone());
        }
    }
    drop(mgr);
    resolve(soname)
}

pub fn map_into_process(soname: &str, cr3: u64) -> Result<u64, i64> {
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
        let data = mgr.libs[lib_idx].data.clone();
        parse_and_prepare(&mut mgr.libs[lib_idx], &data);
    }

    let lib = &mut mgr.libs[lib_idx];
    if lib.segments.is_empty() {
        return Err(-22); // EINVAL
    }

    lib.load_count += 1;
    let lo = lib.lo_vaddr;
    let ehdr_frame = lib.elf_header_frame;
    let total_pages = lib.total_map_pages;
    // saturating - total_pages should be sane but defend against
    // pathological libraries that bypass parse() checks
    let map_size = match (total_pages as u64).checked_add(1).and_then(|v| v.checked_mul(PAGE_SIZE)) {
        Some(v) => v,
        None    => return Err(-12),
    };
    let base_va = match crate::mmap::kernel_find_free(cr3, map_size) {
        Some(v) => v,
        None => return Err(-12), // ENOMEM
    };

    let aspace = AddressSpace::from_raw(cr3);
    let hhdm = grub::hhdm();
    let bias = base_va.wrapping_sub(lo);

    if ehdr_frame != 0 && lo > 0 {
        let flags = PageTableFlags::USER_ACCESSIBLE | PageTableFlags::NO_EXECUTE;
        pmm::ref_inc(ehdr_frame);
        if !aspace.map_page(base_va, ehdr_frame, flags) {
            pmm::ref_dec(ehdr_frame);
            let _ = aspace.into_raw();
            return Err(-12);
        }
    }

    for seg in lib.segments.iter() {
        let seg_va_start = seg.vaddr_start + bias;

        if !seg.writable && !seg.frames.is_empty() {
            let mut flags = PageTableFlags::USER_ACCESSIBLE;
            if seg.pflags & 1 == 0 {
                flags |= PageTableFlags::NO_EXECUTE;
            }

            for (i, &src_frame) in seg.frames.iter().enumerate() {
                let va = seg_va_start + (i as u64) * PAGE_SIZE;
                pmm::ref_inc(src_frame);
                if !aspace.map_page(va, src_frame, flags) {
                    pmm::ref_dec(src_frame);
                    let _ = aspace.into_raw();
                    return Err(-12);
                }
            }
        } else {
            let mut flags = PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE;
            if seg.pflags & 1 == 0 {
                flags |= PageTableFlags::NO_EXECUTE;
            }
            
            for i in 0..seg.num_pages {
                let va = seg_va_start + (i as u64) * PAGE_SIZE;
                let frame = match pmm::alloc_frame() {
                    Some(f) => f,
                    None => {
                        let _ = aspace.into_raw();
                        return Err(-12);
                    }
                };
                
                unsafe {
                    core::ptr::write_bytes((frame + hhdm) as *mut u8, 0, PAGE_SIZE as usize);
                    if i < seg.frames.len() {
                        core::ptr::copy_nonoverlapping(
                            (seg.frames[i] + hhdm) as *const u8,
                            (frame + hhdm) as *mut u8,
                            PAGE_SIZE as usize,
                        );
                    }
                }
                
                if !aspace.map_page(va, frame, flags) {
                    pmm::free_frame(frame);
                    let _ = aspace.into_raw();
                    return Err(-12);
                }
            }
        }
    }

    // kernel_find_free returns a base s.t. base+map_size <= MMAP_LIMIT,
    // so saturating_add cannot reach U64::MAX here, but be explicit
    let vma_end = base_va.saturating_add(map_size);
    crate::mmap::kernel_register_vma(cr3, base_va, vma_end, 5);
    
    let _ = aspace.into_raw();

    let shared_pages: usize = lib.segments.iter().filter(|s| !s.writable).map(|s| s.num_pages).sum();
    let private_pages: usize = lib.segments.iter().filter(|s| s.writable).map(|s| s.num_pages).sum();

    crate::serial_println!(
        "[solib] mapped '{}' at {:#x} (shared={} private={} pages)",
        soname, base_va, shared_pages, private_pages
    );

    Ok(base_va)
}

fn parse_and_prepare(lib: &mut CachedLib, data: &[u8]) {
    let info = match elf::parse(data) {
        Ok(i) => i,
        Err(_) => return,
    };

    let hhdm = grub::hhdm();
    let (mut lo, mut hi) = info.memory_bounds();
    let mut segments: Vec<SharedSegment> = Vec::new();

    for phdr in info.load_segments() {
        if phdr.p_memsz == 0 {
            continue;
        }

        let writable = phdr.p_flags & PF_W != 0;
        let page_start = phdr.p_vaddr & !0xFFF;
        // elf::parse already rejects p_vaddr+p_memsz wrap, but solib
        // sometimes loads files that bypass that path; be defensive
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

    if lo == u64::MAX {
        return;
    }
    lo &= !0xFFF;
    hi = hi.saturating_add(0xFFF) & !0xFFF;
    if hi <= lo { return; }

    let mut ehdr_frame = 0u64;
    let first_seg_start = segments.first().map(|s| s.vaddr_start).unwrap_or(0);
    if first_seg_start > lo || lo > 0 {
        if let Some(frame) = pmm::alloc_frame() {
            unsafe {
                core::ptr::write_bytes((frame + hhdm) as *mut u8, 0, PAGE_SIZE as usize);
                let len = data.len().min(PAGE_SIZE as usize);
                core::ptr::copy_nonoverlapping(data.as_ptr(), (frame + hhdm) as *mut u8, len);
            }
            ehdr_frame = frame;
        }
    }

    let total = ((hi - lo) / PAGE_SIZE) as usize + if ehdr_frame != 0 { 1 } else { 0 };

    lib.segments = segments;
    lib.elf_header_frame = ehdr_frame;
    lib.total_map_pages = total;
    lib.lo_vaddr = lo;
    lib.parsed = true;

    crate::serial_println!(
        "[solib] prepared '{}': {} segs, {} pages",
        lib.name_str(), lib.segments.len(), total
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

            if let Some(data) = crate::vfs_read::read_file(path_str) {
                crate::serial_println!("[solib] ldconfig: '{}' ({} bytes)", lib_name, data.len());
                preload(lib_name, data);
                found += 1;
            }
        }
    }
    crate::serial_println!("[solib] ldconfig: {} libraries cached", found);
}

fn scan_dir_for_libs(dir: &str) -> Vec<String> {
    crate::vfs::core::with_vfs(|vfs| -> Vec<String> {
        let mut libs = Vec::new();
        let dir_id = match vfs.resolve_path(0, dir) {
            Ok(id) => id,
            Err(_) => return libs,
        };
        let mut entries = [crate::vfs::types::DirEntry::empty(); 32];
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
