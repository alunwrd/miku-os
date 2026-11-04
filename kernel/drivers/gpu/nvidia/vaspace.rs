// A GPU address space that knows what is in it
//
// `dev/gpu-memory-compute.md`, 3.2. OpenRM's `OBJGVASPACE` (`gpu_vaspace.c`): a
// range from 1 MiB up (`gvaspaceGetReservedVaspaceBase`), a heap of
// reservations (`gvaspaceAlloc_IMPL`, first fit, or at a fixed address), and in
// each reservation, a `GVAS_BLOCK`, the tree of mappings made inside it.
// Freeing a block unmaps "any leaked mappings" first (`_gvaspaceInternalFree`).
//
// Our spaces are created `IS_EXTERNALLY_OWNED`, so this driver owns their page
// tables and RM does no VA management (`gvaspaceAlloc` refuses, `:1423`).
// `gmmu` writes the entries; this decides what is where, and is the one caller
// of the writer for a mapping somebody will look for again.
//
// Keyed by page directory: a process's own space has the kernel's layout and
// addresses, and a lookup, an unbind or a pool going away must act on the right
// one.
//
// `map` replaces what was there, so a slot re-used for a shorter range leaves
// no valid tail. `unmap_phys` is what a pool going away calls, so the tables do
// not go on translating to frames the page allocator has given away.
//
// The driver's layout is reserved when a space is created (`chanmem::layout`).
// A mapping outside every reservation gets an implicit block and a WARN line

use alloc::string::String;
use alloc::vec::Vec;

use super::gmmu::{self, Aperture};
use super::mmio::MmioRegion;

/// The first address handed out: OpenRM never gives out the first megabyte
/// (`gvaspaceGetReservedVaspaceBase`), so a null-ish pointer is never valid
pub const VA_START: u64 = 1 << 20;
/// The end of what this allocator hands out, far below the 57 bits the tables
/// reach: where the user half of a process ends, all a GPU pointer in ring 3
/// needs
pub const VA_END: u64 = 1 << 47;

const PAGE: u64 = 4096;

/// One mapping inside a block
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub va: u64,
    pub phys: u64,
    pub bytes: u64,
    pub aper: Aperture,
    /// Mapped privileged: memory the hardware walks, not memory a program
    /// addresses (`map_at_priv`)
    pub privileged: bool,
    /// The pages it took: 2 MiB, 64 KiB, 4 KiB (3.8). What `nvidia va all`
    /// shows
    pub pages: gmmu::Pages,
}

/// A reservation, and what is mapped in it
#[derive(Clone, Debug)]
pub struct Block {
    pub va: u64,
    pub bytes: u64,
    pub owner: &'static str,
    /// Made by a mapping that found no reservation (`map`); goes with its last
    /// mapping
    pub implicit: bool,
    pub maps: Vec<Mapping>,
}

impl Block {
    fn end(&self) -> u64 {
        self.va + self.bytes
    }

    fn contains(&self, va: u64, bytes: u64) -> bool {
        va >= self.va && va + bytes <= self.end()
    }
}

/// One address space: its page directory and its reservations, in address
/// order
#[derive(Clone, Debug)]
pub struct Space {
    pub pdb: u64,
    pub name: String,
    pub blocks: Vec<Block>,
}

fn overlaps(a: u64, an: u64, b: u64, bn: u64) -> bool {
    a < b + bn && b < a + an
}

impl Space {
    pub fn new(pdb: u64, name: &str) -> Space {
        Space { pdb, name: String::from(name), blocks: Vec::new() }
    }

    fn insert(&mut self, b: Block) {
        let at = self.blocks.partition_point(|x| x.va < b.va);
        self.blocks.insert(at, b);
    }

    fn is_free(&self, va: u64, bytes: u64) -> bool {
        va >= VA_START
            && va.checked_add(bytes).is_some_and(|e| e <= VA_END)
            && !self.blocks.iter().any(|b| overlaps(b.va, b.bytes, va, bytes))
    }

    /// `FIXED_ADDRESS_ALLOCATE`: exactly here, or not at all
    pub fn reserve_at(&mut self, va: u64, bytes: u64, owner: &'static str) -> Result<(), &'static str> {
        if bytes == 0 || va % PAGE != 0 {
            return Err("a reservation is whole pages");
        }
        let bytes = bytes.next_multiple_of(PAGE);
        if !self.is_free(va, bytes) {
            return Err("that range is reserved already, or outside the space");
        }
        self.insert(Block { va, bytes, owner, implicit: false, maps: Vec::new() });
        Ok(())
    }

    /// First fit from the bottom, as `eheapAlloc` does without `GROWS_DOWN`
    pub fn reserve(&mut self, bytes: u64, align: u64, owner: &'static str) -> Result<u64, &'static str> {
        if bytes == 0 {
            return Err("an empty reservation");
        }
        let bytes = bytes.next_multiple_of(PAGE);
        let align = align.max(PAGE).next_power_of_two();
        let mut at = VA_START.next_multiple_of(align);
        for b in &self.blocks {
            if b.end() <= at {
                continue;
            }
            if at + bytes <= b.va {
                break;
            }
            at = b.end().next_multiple_of(align);
        }
        if at.checked_add(bytes).map_or(true, |e| e > VA_END) {
            return Err("no room left in the address space");
        }
        self.insert(Block { va: at, bytes, owner, implicit: false, maps: Vec::new() });
        Ok(at)
    }

    /// Take a reservation out; the mappings still in it, for the caller to
    /// unmap. `None` if nothing starts at `va`
    pub fn release(&mut self, va: u64) -> Option<Vec<Mapping>> {
        let i = self.blocks.iter().position(|b| b.va == va)?;
        Some(self.blocks.remove(i).maps)
    }

    /// The block `[va, va + bytes)` lies inside
    pub fn block_of(&self, va: u64, bytes: u64) -> Option<usize> {
        self.blocks.iter().position(|b| b.contains(va, bytes))
    }

    pub fn lookup(&self, va: u64) -> Option<&Mapping> {
        self.blocks.iter().flat_map(|b| b.maps.iter()).find(|m| m.va == va)
    }

    /// Everything that has to come out before `[va, va + bytes)` can be mapped,
    /// and where the new mapping goes. `commit` records, once the entries are
    /// written. An implicit block is made here when no reservation holds the
    /// range; `Err` when it straddles one
    pub(super) fn prepare(&mut self, va: u64, bytes: u64) -> Result<(usize, Vec<Mapping>, bool), &'static str> {
        let (bi, implicit) = match self.block_of(va, bytes) {
            Some(i) => (i, false),
            None => {
                if !self.is_free(va, bytes) {
                    return Err("the range straddles a reservation");
                }
                self.insert(Block { va, bytes, owner: "unreserved", implicit: true, maps: Vec::new() });
                (self.block_of(va, bytes).unwrap(), true)
            }
        };
        let b = &mut self.blocks[bi];
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.maps.len() {
            if overlaps(b.maps[i].va, b.maps[i].bytes, va, bytes) {
                out.push(b.maps.swap_remove(i));
            } else {
                i += 1;
            }
        }
        Ok((bi, out, implicit))
    }

    pub(super) fn commit(&mut self, m: Mapping) {
        if let Some(i) = self.block_of(m.va, m.bytes) {
            self.blocks[i].maps.push(m);
        }
    }

    /// Take the mapping at `va` out of the record
    pub fn forget_mapping(&mut self, va: u64) -> Option<Mapping> {
        for bi in 0..self.blocks.len() {
            if let Some(i) = self.blocks[bi].maps.iter().position(|m| m.va == va) {
                let m = self.blocks[bi].maps.swap_remove(i);
                if self.blocks[bi].implicit && self.blocks[bi].maps.is_empty() {
                    self.blocks.remove(bi);
                }
                return Some(m);
            }
        }
        None
    }

    /// Every mapping reaching into `[phys, phys + bytes)` of `aper`, out of the
    /// record.
    ///
    /// The memory as well as the number: a system-memory address and a
    /// video-memory offset count from zero in two different places, and a pool
    /// of RAM at X going away is no reason to take down a context buffer at
    /// video-memory offset X (`dev/gpu-memory-compute.md`, 3.7)
    pub(super) fn forget_phys(&mut self, phys: u64, bytes: u64, aper: Aperture) -> Vec<Mapping> {
        let mut out = Vec::new();
        for b in self.blocks.iter_mut() {
            let mut i = 0;
            while i < b.maps.len() {
                if b.maps[i].aper == aper && overlaps(b.maps[i].phys, b.maps[i].bytes, phys, bytes) {
                    out.push(b.maps.swap_remove(i));
                } else {
                    i += 1;
                }
            }
        }
        self.blocks.retain(|b| !(b.implicit && b.maps.is_empty()));
        out
    }

    pub fn mapping_count(&self) -> usize {
        self.blocks.iter().map(|b| b.maps.len()).sum()
    }
}

/// Every address space this driver owns the tables of
static SPACES: spin::Mutex<Vec<Space>> = spin::Mutex::new(Vec::new());

fn with_space<R>(pdb: u64, f: impl FnOnce(&mut Space) -> R) -> Option<R> {
    SPACES.lock().iter_mut().find(|s| s.pdb == pdb).map(f)
}

/// A new space over `pdb`, with the driver's layout reserved in it. A space
/// that exists is left as it is: the kernel's outlives the channels
/// `chan-cycle` rebuilds in it
pub fn create(pdb: u64, name: &str, layout: &[(&'static str, u64, u64)]) {
    let mut spaces = SPACES.lock();
    if spaces.iter().any(|s| s.pdb == pdb) {
        return;
    }
    let mut s = Space::new(pdb, name);
    for &(owner, va, bytes) in layout {
        if let Err(e) = s.reserve_at(va, bytes, owner) {
            crate::kerror!("[gpu/va] {}: the layout's {} at {:#x} + {:#x}: {}", name, owner, va, bytes, e);
        }
    }
    crate::kdebug!("[gpu/va] {} over pdb {:#x}: {} reservation(s)", name, pdb, s.blocks.len());
    spaces.push(s);
}

/// The space is going away: unmap what is in it (OpenRM's "unmap any leaked
/// mappings"), so its page tables come back, and forget it. Returns how many
/// mappings were left
pub fn destroy(bar0: &MmioRegion, pdb: u64) -> usize {
    let Some(s) = ({
        let mut spaces = SPACES.lock();
        spaces.iter().position(|s| s.pdb == pdb).map(|i| spaces.remove(i))
    }) else {
        return 0;
    };
    let maps: Vec<Mapping> = s.blocks.into_iter().flat_map(|b| b.maps).collect();
    for m in &maps {
        let _ = gmmu::unmap_at(bar0, pdb, m.va, m.bytes);
    }
    crate::kdebug!("[gpu/va] {} over pdb {:#x} destroyed, {} mapping(s) left in it taken out",
        s.name, pdb, maps.len());
    maps.len()
}

/// Everything forgotten: the driver is unloading and `gmmu` drops every table
/// wholesale
pub fn forget_all() {
    SPACES.lock().clear();
}

/// Map `bytes` of `phys` at `va` in the space over `pdb`.
///
/// Whatever was mapped over any part of the range goes first. At the same
/// address that is expected (a slot re-used for another buffer); anywhere else
/// it is said at WARN, since two users believed they owned the addresses. A map
/// failing partway takes its own entries back
pub fn map(bar0: &MmioRegion, pdb: u64, va: u64, phys: u64, bytes: u64, aper: Aperture) -> Result<(), &'static str> {
    map_as(bar0, pdb, va, phys, bytes, aper, false)
}

/// `map`, privileged: a graphics context and what is promoted with it
pub fn map_priv(bar0: &MmioRegion, pdb: u64, va: u64, phys: u64, bytes: u64, aper: Aperture) -> Result<(), &'static str> {
    map_as(bar0, pdb, va, phys, bytes, aper, true)
}

fn map_as(
    bar0: &MmioRegion,
    pdb: u64,
    va: u64,
    phys: u64,
    bytes: u64,
    aper: Aperture,
    privileged: bool,
) -> Result<(), &'static str> {
    let sw = crate::kcore::time::Stopwatch::start();
    let r = map_as_timed(bar0, pdb, va, phys, bytes, aper, privileged);
    took("map", pdb, va, bytes, sw.elapsed_us());
    r
}

/// Every map and unmap in `nvidia log mem debug`, with how long it held the
/// caller, and a warning past 2 ms; a frame at 240 Hz is 4.2
fn took(what: &str, pdb: u64, va: u64, bytes: u64, us: u64) {
    if us >= 2000 {
        crate::kwarn!("[gpu/va] {} of {:#x} + {:#x} in pdb {:#x} took {} us", what, va, bytes, pdb, us);
    } else {
        crate::kdebug!("[gpu/va] {} {:#x} + {:#x} in pdb {:#x}: {} us", what, va, bytes, pdb, us);
    }
}

fn map_as_timed(
    bar0: &MmioRegion,
    pdb: u64,
    va: u64,
    phys: u64,
    bytes: u64,
    aper: Aperture,
    privileged: bool,
) -> Result<(), &'static str> {
    if pdb == 0 {
        return Err("no page directory");
    }
    let bytes = bytes.next_multiple_of(PAGE);
    let prepared = {
        let mut spaces = SPACES.lock();
        if !spaces.iter().any(|s| s.pdb == pdb) {
            crate::kwarn!("[gpu/va] a mapping at {:#x} in pdb {:#x}, which no space was made for - making one", va, pdb);
            spaces.push(Space::new(pdb, "unnamed"));
        }
        let s = spaces.iter_mut().find(|s| s.pdb == pdb).unwrap();
        s.prepare(va, bytes).map(|(bi, old, implicit)| (s.blocks[bi].owner, old, implicit))
    };
    let (owner, old, implicit) = match prepared {
        Ok(p) => p,
        Err(e) => {
            crate::kerror!("[gpu/va] {:#x} + {:#x} in pdb {:#x}: {}", va, bytes, pdb, e);
            return Err(e);
        }
    };
    if implicit {
        crate::kwarn!("[gpu/va] {:#x} + {:#x} in pdb {:#x} is in no reservation - mapped anyway, as an implicit block",
            va, bytes, pdb);
    }
    for m in &old {
        if m.va != va {
            crate::kwarn!("[gpu/va] {:#x} + {:#x} ({}) overlaps the mapping at {:#x} + {:#x} - that one comes out",
                va, bytes, owner, m.va, m.bytes);
        }
        let _ = gmmu::unmap_at(bar0, pdb, m.va, m.bytes);
    }
    let r = if privileged {
        gmmu::map_at_priv(bar0, pdb, va, phys, bytes, aper)
    } else {
        gmmu::map_at(bar0, pdb, va, phys, bytes, aper)
    };
    match r {
        Ok(pages) => {
            with_space(pdb, |s| s.commit(Mapping { va, phys, bytes, aper, privileged, pages }));
            Ok(())
        }
        Err(e) => {
            // Whatever part of it landed is taken back
            let _ = gmmu::unmap_at(bar0, pdb, va, bytes);
            if implicit {
                with_space(pdb, |s| s.blocks.retain(|b| !(b.implicit && b.maps.is_empty())));
            }
            crate::kerror!("[gpu/va] mapping {:#x} + {:#x} ({}) in pdb {:#x} failed: {}", va, bytes, owner, pdb, e);
            Err(e)
        }
    }
}

/// Take the mapping at `va` out. Nothing there is not an error, so a teardown
/// can be retried
pub fn unmap(bar0: &MmioRegion, pdb: u64, va: u64) {
    if let Some(Some(m)) = with_space(pdb, |s| s.forget_mapping(va)) {
        let sw = crate::kcore::time::Stopwatch::start();
        let _ = gmmu::unmap_at(bar0, pdb, m.va, m.bytes);
        took("unmap", pdb, m.va, m.bytes, sw.elapsed_us());
    }
}

/// What is mapped at `va` in the space over `pdb`: `(phys, bytes)`, what a
/// cache asks before trusting itself
pub fn lookup(pdb: u64, va: u64) -> Option<(u64, u64)> {
    with_space(pdb, |s| s.lookup(va).map(|m| (m.phys, m.bytes))).flatten()
}

/// Memory is being given back: take every mapping reaching into it out of every
/// space, each in its own page directory. Returns how many. `aper` says which
/// memory the range is in; see `forget_phys`
pub fn unmap_phys(bar0: &MmioRegion, phys: u64, bytes: u64, aper: Aperture) -> usize {
    let hit: Vec<(u64, Mapping)> = SPACES
        .lock()
        .iter_mut()
        .flat_map(|s| {
            let pdb = s.pdb;
            s.forget_phys(phys, bytes, aper).into_iter().map(move |m| (pdb, m))
        })
        .collect();
    for (pdb, m) in &hit {
        let sw = crate::kcore::time::Stopwatch::start();
        let _ = gmmu::unmap_at(bar0, *pdb, m.va, m.bytes);
        took("unmap (memory given back)", *pdb, m.va, m.bytes, sw.elapsed_us());
    }
    hit.len()
}

/// Mappings in every space, for `nvidia vram` and the teardown test
pub fn mapping_count() -> usize {
    SPACES.lock().iter().map(|s| s.mapping_count()).sum()
}

/// A copy of every space, for `nvidia va`
pub fn spaces() -> Vec<Space> {
    SPACES.lock().clone()
}
