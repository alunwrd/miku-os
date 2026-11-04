// Shared memory pools
//
// How a client hands the compositor a few megabytes of pixels without copying
// them.
//
// Wayland uses a file descriptor passed over the socket, which both sides mmap,
// because its compositor is another process. Ours is the kernel, which already
// owns every physical page, so the descriptor step would only cost an
// SCM_RIGHTS implementation.
//
// So a pool is physically contiguous memory with a u32 handle. The client gets
// it mapped into its address space; the compositor reaches it through the HHDM.
//
//   - contiguous means the copy engine can read a client's buffer directly,
//     with nothing touching the pixels on the way
//   - a handle is not a capability. Ownership is recorded and checked here,
//     because unlike a descriptor nothing else holds the right

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::mm::dma::DmaBuffer;

/// Biggest pool a client may ask for. 64 MiB is four 4K framebuffers
pub const MAX_POOL_BYTES: usize = 64 * 1024 * 1024;

/// How many pools can be live at once, across all clients
///
/// Four of the busiest client's share. The table is a list, and what it costs
/// is the memory its pools hold, checked when each pool is made; this only
/// keeps the count bounded
pub const MAX_POOLS: usize = 64;

/// How many one process may hold of those
///
/// The table is system-wide, so without this one client could take all of it
/// with one-page `may_pool` calls, and the console grid, the keymaps and the
/// wallpaper would fail to allocate.
///
/// A window needs two buffers, which is one pool, and a client gets one more
/// for its keymap (`iface/seat.rs`). end4's shell on aosh has seven windows of
/// its own (the background, the bar, the overview, the screen's corners...),
/// its keymap, and the pool it copies windows into for the overview
/// (`may_desktop.capture`). Sixteen is that with room for a few more.
///
/// Wayland needs no equivalent because a pool there is a file descriptor and
/// the memory is the client's own; ours is physically contiguous kernel memory
pub const MAX_POOLS_PER_OWNER: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShmError {
    /// size was zero or above MAX_POOL_BYTES
    BadSize,
    /// No contiguous run of that many frames was available
    NoMemory,
    /// MAX_POOLS live already
    Full,
    /// This owner already holds MAX_POOLS_PER_OWNER of them
    OwnerFull,
    /// A video-memory pool asked for what only system memory has: a plain
    /// physical address, or bytes the kernel can read through its own map
    Video,
    /// No pool under that handle
    BadHandle,
    /// The pool exists but belongs to another owner
    NotOwner,
    /// Already mapped, and into a different address space
    AlreadyMapped,
}

/// Where a pool's pixels live
///
/// Almost every pool owns its frames. The console is the exception: it already
/// renders into a screen-sized physically contiguous shadow, and a second copy
/// would be eight more megabytes of contiguous memory, which this machine may
/// not have after a GSP-RM firmware load. So a pool can be a window onto memory
/// the kernel allocated elsewhere and keeps; the pool frees nothing.
///
/// A pool can also be video memory (`dev/gpu-memory-compute.md`): a block the
/// NVIDIA driver handed out under BAR1's aperture
/// (`drivers::gpu::nvidia::user_vram`), which the owner maps write-combining
/// through BAR1 and the GPU reads in place. Its number is an offset in video
/// memory, not an address in system memory, so `phys` refuses it and the GPU
/// paths ask `location`
enum Backing {
    Owned(DmaBuffer),
    Borrowed { phys: u64, virt: *mut u8, len: usize },
    Video { vram: u64, cpu_phys: u64, len: usize },
}

/// Which memory a pool's address is in
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Memory {
    System,
    Video,
}

impl Backing {
    /// The address the CPU maps it at: BAR1's for video memory
    fn cpu_phys(&self) -> u64 {
        match self {
            Backing::Owned(b) => b.phys(),
            Backing::Borrowed { phys, .. } => *phys,
            Backing::Video { cpu_phys, .. } => *cpu_phys,
        }
    }
    /// The address an engine is given, and in which memory
    fn location(&self) -> (u64, Memory) {
        match self {
            Backing::Video { vram, .. } => (*vram, Memory::Video),
            other => (other.cpu_phys(), Memory::System),
        }
    }
    fn len(&self) -> usize {
        match self {
            Backing::Owned(b) => b.size(),
            Backing::Borrowed { len, .. } | Backing::Video { len, .. } => *len,
        }
    }
    /// The bytes through the kernel's own map. Not for video memory, which the
    /// kernel maps only to zero it: `with_bytes` refuses it first
    fn as_slice(&self) -> &[u8] {
        match self {
            Backing::Owned(b) => b.as_slice(),
            Backing::Borrowed { virt, len, .. } =>
                unsafe { core::slice::from_raw_parts(*virt, *len) },
            Backing::Video { .. } => &[],
        }
    }
    fn as_mut_slice(&mut self) -> &mut [u8] {
        match self {
            Backing::Owned(b) => b.as_mut_slice(),
            Backing::Borrowed { virt, len, .. } =>
                unsafe { core::slice::from_raw_parts_mut(*virt, *len) },
            Backing::Video { .. } => &mut [],
        }
    }
}

// Borrowed holds a raw pointer, which is not Send by default. It is kernel
// memory that outlives the pool by construction (see adopt())
unsafe impl Send for Backing {}

struct Pool {
    handle: u32,
    /// The pid that created it. Pools created by the kernel itself (the
    /// keymap, a test) use 0
    owner: u64,
    mem: Backing,
    /// Bytes the client asked for. The backing is rounded up to whole pages,
    /// and the tail beyond this is not part of the pool
    size: usize,
    /// Where the owning process sees it, if it asked to: (cr3, address)
    ///
    /// Recorded so the mapping can be taken down before the frames are freed. A
    /// pool destroyed while a live process still mapped it would leave that
    /// process pointing at reallocated memory
    mapping: Option<(u64, u64)>,
}

struct Pools {
    pools: Vec<Pool>,
    next_handle: u32,
}

static POOLS: Mutex<Pools> = Mutex::new(Pools {
    pools: Vec::new(),
    next_handle: 1,
});

/// Frames whose pool has been destroyed but which the copy engine may still be
/// reading
///
/// A frame is submitted to the engine as a list of physical addresses and the
/// call returns once the batch is queued; the fence is waited on at the top of
/// the next frame. In between, anything that frees a pool (a client calling
/// `may_unpool`, a process exiting, a demo stopping) would hand those pages to
/// the allocator while the engine reads them.
///
/// So a destroyed pool's frames go here, and the compositor returns them in
/// `reap` once its wait proves the engine is finished with their frame. With no
/// compositor running they are freed at once
static RETIRED: Mutex<Vec<DmaBuffer>> = Mutex::new(Vec::new());

/// Puts a pool's memory beyond the engine's reach before the allocator sees it
///
/// Waiting out the copy engine covers the addresses in this frame's batch, but
/// not the GPU's page tables, where a buffer shown to the effects pass stays
/// translatable until removed. So the translation goes first, then the wait,
/// then the pages.
///
/// Borrowed backings own no pages but can be mapped just the same, so the
/// unbind applies to them too; they only skip the retire list
fn retire(mem: Backing) {
    // Video memory goes back the driver's way: out of every GPU address
    // space, both engines waited out, then the block (`user_vram::retire`)
    if let Backing::Video { vram, len, .. } = mem {
        crate::drivers::gpu::nvidia::user_vram::retire(vram, len as u64);
        return;
    }
    let (phys, len) = (mem.cpu_phys(), mem.len() as u64);
    if let Some(b) = crate::mayland::render::backend() {
        let n = b.unbind_range(phys, len);
        if n != 0 {
            UNBOUND.fetch_add(n as u64, core::sync::atomic::Ordering::Relaxed);
        }
    }
    let Backing::Owned(buf) = mem else { return };
    if !crate::mayland::render::running() {
        drop(buf);
        return;
    }
    RETIRED.lock().push(buf);
}

/// GPU mappings dropped because the memory under them was being given back.
/// Printed by `mayland status`: should be small and should not grow while
/// nothing is being destroyed
static UNBOUND: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// How many GPU mappings have been dropped on the way to the allocator
pub fn unbound() -> u64 {
    UNBOUND.load(core::sync::atomic::Ordering::Relaxed)
}

/// Hands back every frame the engine has finished with
///
/// Called from the compositor right after waiting on the previous frame's
/// copies, when those addresses stop being live. Returns how many pools were
/// freed, for `mayland status`
pub fn reap() -> usize {
    let taken = core::mem::take(&mut *RETIRED.lock());
    let n = taken.len();
    drop(taken);
    n
}

/// How many pools are waiting for the engine. Should be zero between frames
pub fn retired() -> usize {
    RETIRED.lock().len()
}

/// Allocates a pool of at least 'size' bytes, returning its handle
///
/// The memory is physically contiguous and zeroed: these are recycled frames,
/// and a client mapping a pool must not read what the last one left there
pub fn create(owner: u64, size: usize) -> Result<u32, ShmError> {
    if size == 0 || size > MAX_POOL_BYTES {
        return Err(ShmError::BadSize);
    }
    // Asked before the allocation as well as after it: a full table would
    // otherwise take up to 64 MiB of contiguous frames, zero them and throw
    // them away. The check below decides; this keeps the common refusal cheap
    {
        let tbl = POOLS.lock();
        if tbl.pools.len() >= MAX_POOLS {
            return Err(ShmError::Full);
        }
        if over_share(&tbl, owner) {
            return Err(ShmError::OwnerFull);
        }
    }
    let pages = (size + 4095) / 4096;

    let mut mem = Backing::Owned(DmaBuffer::alloc(pages).map_err(|_| ShmError::NoMemory)?);
    for b in mem.as_mut_slice() { *b = 0; }

    let mut tbl = POOLS.lock();
    if tbl.pools.len() >= MAX_POOLS {
        return Err(ShmError::Full);
    }
    // Asked again after the allocation, because the table was released while
    // the frames were found and zeroed, for the same reason the total is
    // checked twice
    if over_share(&tbl, owner) {
        return Err(ShmError::OwnerFull);
    }
    let handle = tbl.next_handle;
    tbl.next_handle = tbl.next_handle.wrapping_add(1).max(1);
    tbl.pools.push(Pool { handle, owner, mem, size, mapping: None });
    Ok(handle)
}

/// Whether this owner has had its share of the table already
///
/// Owner 0 is the kernel (the console's grid, the wallpaper, a window's
/// afterimage) and is not under the per-client limit. Under the cap, a desktop
/// with many windows closing at once could not draw the next one's exit
fn over_share(tbl: &Pools, owner: u64) -> bool {
    owner != 0 && tbl.pools.iter().filter(|p| p.owner == owner).count()
                    >= MAX_POOLS_PER_OWNER
}

/// Makes a pool out of memory the kernel already has and keeps
///
/// For memory that must outlive every pool made of it. The wallpaper's eight
/// megabytes are reserved once at boot and lent out on every `mayland start`.
/// The console's shadow exists whether or not anything composites, and the
/// alternative is a second one with a screen copied into it every frame.
/// Destroying this pool frees nothing.
///
/// The caller must have established two things, which nothing here can check:
/// the region is physically contiguous at 'phys', and it outlives every pool
/// made from it. The console's shadow satisfies both: it is leaked on purpose
/// and its contiguity is verified where it is allocated
///
/// # Safety
/// 'virt' must point at 'size' bytes of live kernel memory whose physical
/// base is 'phys' and which is never freed
pub unsafe fn adopt(owner: u64, phys: u64, virt: *mut u8, size: usize)
    -> Result<u32, ShmError>
{
    if size == 0 || size > MAX_POOL_BYTES {
        return Err(ShmError::BadSize);
    }
    let mut tbl = POOLS.lock();
    if tbl.pools.len() >= MAX_POOLS {
        return Err(ShmError::Full);
    }
    let handle = tbl.next_handle;
    tbl.next_handle = tbl.next_handle.wrapping_add(1).max(1);
    tbl.pools.push(Pool {
        handle,
        owner,
        mem: Backing::Borrowed { phys, virt, len: size },
        size,
        mapping: None,
    });
    Ok(handle)
}

/// Make a pool of video memory the NVIDIA driver has just handed out
/// (`user_vram::alloc`), owned by `owner`. Under the same table limits as any
/// pool; on a refusal the caller gives the block back
pub fn adopt_video(owner: u64, vram: u64, cpu_phys: u64, len: usize, size: usize)
    -> Result<u32, ShmError>
{
    if size == 0 || size > MAX_POOL_BYTES || size > len {
        return Err(ShmError::BadSize);
    }
    let mut tbl = POOLS.lock();
    if tbl.pools.len() >= MAX_POOLS {
        return Err(ShmError::Full);
    }
    if over_share(&tbl, owner) {
        return Err(ShmError::OwnerFull);
    }
    let handle = tbl.next_handle;
    tbl.next_handle = tbl.next_handle.wrapping_add(1).max(1);
    tbl.pools.push(Pool {
        handle, owner, mem: Backing::Video { vram, cpu_phys, len }, size, mapping: None,
    });
    Ok(handle)
}

/// Gives the owning process a window onto the pool, returning the address
///
/// One mapping per pool: asking twice returns the address it already has
pub fn map_into(handle: u32, owner: u64, cr3: u64) -> Result<u64, ShmError> {
    let mut tbl = POOLS.lock();
    let p = tbl.pools.iter_mut().find(|p| p.handle == handle)
        .ok_or(ShmError::BadHandle)?;
    if p.owner != owner {
        return Err(ShmError::NotOwner);
    }
    // A borrowed pool is kernel memory (the wallpaper's reserve or the
    // console's shadow). Mapping it into a process would hand that process the
    // kernel heap, and the reference counting that makes an owned pool survive
    // process teardown does not apply to memory the allocator never gave out as
    // frames
    if matches!(p.mem, Backing::Borrowed { .. }) {
        return Err(ShmError::NotOwner);
    }
    if let Some((mapped_cr3, va)) = p.mapping {
        if mapped_cr3 == cr3 {
            return Ok(va);
        }
        // A second address space wanting the same pool is a sharing model that
        // does not exist here
        return Err(ShmError::AlreadyMapped);
    }
    let bytes = p.mem.len() as u64;
    let prot = crate::mm::mmap::PROT_READ | crate::mm::mmap::PROT_WRITE;
    // Video memory through BAR1 is a device's memory, mapped write-combining
    let va = match p.mem {
        Backing::Video { cpu_phys, .. } =>
            crate::mm::mmap::map_mmio_into_user_wc(cr3, cpu_phys, bytes, prot),
        _ => crate::mm::mmap::map_phys_into_user(cr3, p.mem.cpu_phys(), bytes, prot),
    }.ok_or(ShmError::NoMemory)?;
    p.mapping = Some((cr3, va));
    Ok(va)
}

/// Physical base of a pool, for handing to the copy engine
pub fn phys(handle: u32) -> Result<u64, ShmError> {
    let tbl = POOLS.lock();
    let p = tbl.pools.iter().find(|p| p.handle == handle).ok_or(ShmError::BadHandle)?;
    // A video-memory pool's number is an offset in video memory, and a caller
    // asking for a plain physical address would hand it to an engine as system
    // memory. Refused, not translated: the callers that can use video memory
    // ask `location`
    match p.mem.location() {
        (addr, Memory::System) => Ok(addr),
        (_, Memory::Video) => Err(ShmError::Video),
    }
}

/// The address an engine is given for this pool, and which memory it is in, for
/// the GPU paths, which can use either (`gpu_bind`, `gpu_submit`)
pub fn location(handle: u32) -> Result<(u64, Memory), ShmError> {
    let tbl = POOLS.lock();
    tbl.pools.iter().find(|p| p.handle == handle)
        .map(|p| p.mem.location())
        .ok_or(ShmError::BadHandle)
}

pub fn size(handle: u32) -> Result<usize, ShmError> {
    let tbl = POOLS.lock();
    tbl.pools.iter().find(|p| p.handle == handle)
        .map(|p| p.size)
        .ok_or(ShmError::BadHandle)
}

/// Every pool there is, on the serial line
///
/// For a handle the client believes it owns and the table does not have
pub fn dump() {
    let tbl = POOLS.lock();
    crate::serial_println!("[shm] {} pool(s):", tbl.pools.len());
    for p in tbl.pools.iter() {
        crate::serial_println!("[shm]   handle {} owner {} size {}",
                               p.handle, p.owner, p.size);
    }
}

/// Checks that 'owner' may use this pool, and returns its size
pub fn claim(handle: u32, owner: u64) -> Result<usize, ShmError> {
    let tbl = POOLS.lock();
    let p = tbl.pools.iter().find(|p| p.handle == handle)
        .ok_or(ShmError::BadHandle)?;
    if p.owner != owner {
        return Err(ShmError::NotOwner);
    }
    Ok(p.size)
}

/// Runs 'f' over a byte range of a pool
///
/// A closure because the pool table is behind a lock, and a &[u8] outliving it
/// would let a pool be freed while the compositor reads it. Every access goes
/// through here or through the physical address, which the copy engine consumes
/// while the frame is pinned
pub fn with_bytes<R>(handle: u32, offset: usize, len: usize, f: impl FnOnce(&[u8]) -> R)
    -> Result<R, ShmError>
{
    let tbl = POOLS.lock();
    let p = tbl.pools.iter().find(|p| p.handle == handle)
        .ok_or(ShmError::BadHandle)?;
    if matches!(p.mem, Backing::Video { .. }) {
        return Err(ShmError::Video);
    }
    let end = offset.checked_add(len).ok_or(ShmError::BadSize)?;
    if end > p.size {
        return Err(ShmError::BadSize);
    }
    Ok(f(&p.mem.as_slice()[offset..end]))
}

/// Writes into a pool. Used by the kernel side (the keymap) and by the
/// in-kernel test client; a real client writes through its own mapping
pub fn with_bytes_mut<R>(handle: u32, offset: usize, len: usize, f: impl FnOnce(&mut [u8]) -> R)
    -> Result<R, ShmError>
{
    let mut tbl = POOLS.lock();
    let p = tbl.pools.iter_mut().find(|p| p.handle == handle)
        .ok_or(ShmError::BadHandle)?;
    if matches!(p.mem, Backing::Video { .. }) {
        return Err(ShmError::Video);
    }
    let end = offset.checked_add(len).ok_or(ShmError::BadSize)?;
    if end > p.size {
        return Err(ShmError::BadSize);
    }
    Ok(f(&mut p.mem.as_mut_slice()[offset..end]))
}

/// Rows of one pool copied into another, under one hold of the table:
/// `rows` rows of `row` bytes, `src_stride` apart from `src_off` in `src`,
/// `dst_stride` apart from `dst_off` in `dst`. A window's pixels into a
/// shell's pool (`may_desktop.capture`)
///
/// One function rather than `with_bytes` inside `with_bytes_mut`, which would
/// take the table's lock twice. Both pools system memory, both ranges inside
/// them; the same pool both ways is allowed, and the rows may overlap
pub fn copy_rows(src: u32, src_off: usize, src_stride: usize, dst: u32, dst_off: usize, dst_stride: usize,
                 row: usize, rows: usize) -> Result<(), ShmError> {
    // Where the last row ends
    let end = |off: usize, stride: usize| -> Option<usize> {
        match rows {
            0 => Some(off),
            _ => stride.checked_mul(rows - 1)?.checked_add(row)?.checked_add(off),
        }
    };
    let src_end = end(src_off, src_stride).ok_or(ShmError::BadSize)?;
    let dst_end = end(dst_off, dst_stride).ok_or(ShmError::BadSize)?;
    let mut tbl = POOLS.lock();
    let si = tbl.pools.iter().position(|p| p.handle == src).ok_or(ShmError::BadHandle)?;
    let di = tbl.pools.iter().position(|p| p.handle == dst).ok_or(ShmError::BadHandle)?;
    for i in [si, di] {
        if matches!(tbl.pools[i].mem, Backing::Video { .. }) {
            return Err(ShmError::Video);
        }
    }
    if src_end > tbl.pools[si].size || dst_end > tbl.pools[di].size {
        return Err(ShmError::BadSize);
    }
    if si == di {
        let buf = tbl.pools[si].mem.as_mut_slice();
        for r in 0..rows {
            let from = src_off + r * src_stride;
            buf.copy_within(from..from + row, dst_off + r * dst_stride);
        }
        return Ok(());
    }
    let (from, to) = if si < di {
        let (lo, hi) = tbl.pools.split_at_mut(di);
        (&lo[si], &mut hi[0])
    } else {
        let (lo, hi) = tbl.pools.split_at_mut(si);
        (&hi[0], &mut lo[di])
    };
    let (s, d) = (from.mem.as_slice(), to.mem.as_mut_slice());
    for r in 0..rows {
        let (a, b) = (src_off + r * src_stride, dst_off + r * dst_stride);
        d[b..b + row].copy_from_slice(&s[a..a + row]);
    }
    Ok(())
}

pub fn destroy(handle: u32) -> Result<(), ShmError> {
    destroy_inner(handle, None)
}

/// Destroys a pool only if 'owner' made it
///
/// One call rather than `claim` then `destroy`, because the table lock is
/// released between those two and the handle can be another process's by the
/// second
pub fn destroy_owned(handle: u32, owner: u64) -> Result<(), ShmError> {
    destroy_inner(handle, Some(owner))
}

fn destroy_inner(handle: u32, owner: Option<u64>) -> Result<(), ShmError> {
    let mut tbl = POOLS.lock();
    let Some(i) = tbl.pools.iter().position(|p| p.handle == handle) else {
        return Err(ShmError::BadHandle);
    };
    if let Some(o) = owner {
        if tbl.pools[i].owner != o {
            return Err(ShmError::NotOwner);
        }
    }
    let p = tbl.pools.remove(i);
    drop(tbl);
    // The window first, then the frames: the other way round leaves a live page
    // table pointing at memory about to be reallocated. Outside the table lock,
    // since taking a window down waits on the other processors
    // (`mm::tlb::shootdown`); out of the table, nobody else can reach the pool
    unmap(&p);
    retire(p.mem);
    Ok(())
}

/// Takes a pool's window out of its process. Has to come before the frames go
/// back to the allocator
fn unmap(p: &Pool) {
    if let Some((cr3, va)) = p.mapping {
        crate::mm::mmap::unmap_phys_from_user(
            cr3, va, p.mem.cpu_phys(), p.mem.len() as u64,
        );
    }
}

/// Releases every pool a process left behind. Called from process teardown, as
/// for sockets
pub fn destroy_all_for(owner: u64) {
    let mut gone: Vec<Pool> = Vec::new();
    {
        let mut tbl = POOLS.lock();
        let mut i = 0;
        while i < tbl.pools.len() {
            if tbl.pools[i].owner == owner {
                gone.push(tbl.pools.remove(i));
            } else {
                i += 1;
            }
        }
    }
    // Outside the table lock, as in `destroy_inner`, and retired rather than
    // freed: a process can exit while the compositor is half way through a
    // frame that names its pixels
    for p in gone {
        unmap(&p);
        retire(p.mem);
    }
}

/// Every pool of video memory, gone without giving the memory back: GSP-RM is
/// being unloaded by force (`bringup::unload`), and the allocator the blocks
/// came from goes with it. The windows are taken out of their processes first,
/// since after a reload those blocks are somebody else's. Returns how many
pub fn revoke_video() -> usize {
    let gone: Vec<Pool> = {
        let mut tbl = POOLS.lock();
        let mut out = Vec::new();
        let mut i = 0;
        while i < tbl.pools.len() {
            if matches!(tbl.pools[i].mem, Backing::Video { .. }) {
                out.push(tbl.pools.remove(i));
            } else {
                i += 1;
            }
        }
        out
    };
    for p in &gone {
        unmap(p);
    }
    gone.len()
}

/// Every pool window in address space `cr3`, as (address, bytes)
///
/// For `fork`, which leaves them out of the child (`mm::vmm::PTE_LENT`)
pub fn windows_in(cr3: u64) -> Vec<(u64, u64)> {
    let tbl = POOLS.lock();
    tbl.pools.iter()
        .filter_map(|p| match p.mapping {
            Some((c, va)) if c == cr3 => Some((va, p.mem.len() as u64)),
            _ => None,
        })
        .collect()
}

/// Takes every window out of address space `cr3`, keeping the pools
///
/// For `exec`, which frees that address space. A window left recorded would be
/// taken down later through page tables that belong to someone else. Under the
/// table lock, unlike a destroy: these pools stay in the table, and a racing
/// `destroy` must not retire the frames before their window is gone. The
/// address space has no task, so there is no shootdown to wait for
pub fn unmap_windows_in(cr3: u64) {
    let mut tbl = POOLS.lock();
    for p in tbl.pools.iter_mut() {
        if matches!(p.mapping, Some((c, _)) if c == cr3) {
            unmap(p);
            p.mapping = None;
        }
    }
}

pub struct PoolInfo {
    pub handle: u32,
    pub owner: u64,
    /// In system memory, or, with `memory` saying so, an offset in video memory
    pub phys: u64,
    pub memory: Memory,
    pub size: usize,
}

pub fn list() -> Vec<PoolInfo> {
    let tbl = POOLS.lock();
    tbl.pools.iter().map(|p| {
        let (phys, memory) = p.mem.location();
        PoolInfo { handle: p.handle, owner: p.owner, phys, memory, size: p.size }
    }).collect()
}
