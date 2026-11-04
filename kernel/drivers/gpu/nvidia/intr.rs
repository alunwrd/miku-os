// The GPU's top-level interrupt tree, and the MSI that delivers it
//
// Everything the card wants to say arrives in one place: 32-bit leaves in the
// VFN register block at BAR0 0xb80000, with a summary word above them. One MSI
// vector carries all of it; reading the tree tells the CPU which engine spoke.
//
// The register layout is the same from Turing to Blackwell; the number of
// leaves differs, and the chip says which (`Geometry`).
//
// The order around a read is `tu102_vfn_intr_*`
// (`refsrc/nouveau/nvkm/subdev/vfn/tu102.c`): unarm, read the summary, read the
// leaves it points at, clear, rearm.
//
// Which bit means what is not fixed in silicon: GSP-RM assigns the vectors at
// boot and hands out the assignment through `gsp_common::intr::kernel_table`. A
// vector is `leaf * 32 + bit`.
//
// Safety
//
// Legacy INTx is masked at bring-up (`blackwell/init.rs`): an interrupt nobody
// handles stops the machine. So nothing here unmasks a bit with no handler:
// `allow` takes one vector at a time, and a bit that fires anyway is cleared
// and counted. `nointr` on the GRUB command line skips all of this and leaves
// the driver polling.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::apic;
use crate::kcore::irq_lock::IrqMutex;

use super::mmio::MmioRegion;
use super::pci::GpuDevice;

/// NV_VFN, the top-level interrupt block
pub const VFN_BASE: u32 = 0x00b8_0000;

/// Per-leaf status. Reading gives the pending bits; writing the same value back
/// clears them (write-1-to-clear), `tu102_vfn_intr_reset`
const VFN_LEAF: u32 = VFN_BASE + 0x1000;
/// Write a mask here to let those bits through
const VFN_ALLOW: u32 = VFN_BASE + 0x1200;
/// Write a mask here to stop them
const VFN_BLOCK: u32 = VFN_BASE + 0x1400;
/// Summary: bit N is set when either of leaves 2N and 2N+1 has something.
/// One register holds every subtree either shape of tree has
const VFN_TOP: u32 = VFN_BASE + 0x1600;
/// Let the tree raise interrupts again (`CPU_INTR_TOP_EN_SET`)
const VFN_REARM: u32 = VFN_BASE + 0x1608;
/// Stop it raising them while the leaves are being read
/// (`CPU_INTR_TOP_EN_CLEAR`)
const VFN_UNARM: u32 = VFN_BASE + 0x1610;

/// The shape of one chip's tree.
///
/// Two numbers, kept apart as in OpenRM: how many leaves carry interrupts
/// (`intrGetNumLeaves`), two per top-level subtree up to the last stall
/// subtree, and how many leaf registers exist (`intrGetLeafSize`,
/// `CPU_INTR_LEAF__SIZE_1`). Clearing and blocking walks the registers; reading
/// walks the leaves
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub leaves: u32,
    pub leaf_regs: u32,
}

impl Geometry {
    /// TU10x, GA10x and AD10x: stall subtrees end at 3
    /// (`tu102/dev_vm_addendum.h`), eight leaf registers (`tu102/dev_vm.h:55`)
    pub const TU102: Geometry = Geometry { leaves: 8, leaf_regs: 8 };
    /// GH100 and every Blackwell: `g_intr_nvoc.c` gives everything that is not
    /// TU/GA/AD the GH100 functions. Stall subtrees end at 5
    /// (`gh100/dev_vm_addendum.h:45`), so twelve leaves, out of sixteen leaf
    /// registers (`gh100/dev_vm.h:44`)
    pub const GH100: Geometry = Geometry { leaves: 12, leaf_regs: 16 };
    /// No tree: nothing is routable. What `TREE` holds until `enable`
    pub const NONE: Geometry = Geometry { leaves: 0, leaf_regs: 0 };

    /// Top-level subtrees, two leaves each
    /// (`NV_CTRL_INTR_SUBTREE_TO_LEAF_IDX_*`)
    pub const fn subtrees(self) -> u32 {
        self.leaves.div_ceil(2)
    }

    /// Every subtree, as `TOP_EN_SET`/`TOP_EN_CLEAR` take them
    pub const fn subtree_mask(self) -> u32 {
        if self.subtrees() >= 32 { u32::MAX } else { (1 << self.subtrees()) - 1 }
    }

    /// Which leaf and which bit a vector lands in, or `None` past the tree
    pub const fn split(self, vector: u32) -> Option<(u32, u32)> {
        if vector >= self.leaves * 32 {
            return None;
        }
        Some((vector / 32, vector % 32))
    }
}

/// How many things can ask for a vector: GSP, DISP, DISP_LOW, the two engines
/// whose fences wake waiters, and room for three more. Walked inside the
/// interrupt handler, so small
const MAX_HANDLERS: usize = 8;

/// Everything the interrupt handler needs, under one lock.
///
/// Written from thread context at bring-up and read from interrupt context,
/// hence `IrqMutex`, like `apic::MSI_HANDLERS`. The handler copies this out and
/// drops the lock before it dispatches
#[derive(Copy, Clone)]
struct Tree {
    bar0: Option<MmioRegion>,
    geometry: Geometry,
    /// The subtrees armed: all except the non-stall ones, what
    /// `_intrEnableStall_TU102` step 3 arms
    arm: u32,
    handlers: [Option<(u32, Handler)>; MAX_HANDLERS],
}

/// What a handler is. It is handed BAR0, so nothing downstream needs a static
/// to reach the card
pub type Handler = fn(&MmioRegion);

static TREE: IrqMutex<Tree> = IrqMutex::new(Tree {
    bar0: None,
    geometry: Geometry::NONE,
    arm: 0,
    handlers: [None; MAX_HANDLERS],
});

/// Interrupts serviced, top-level reads that found nothing, and bits pending
/// with nobody registered. The third is a vector allowed by mistake or an
/// unknown engine, and it is being thrown away
static SERVED: AtomicU64 = AtomicU64::new(0);
static SPURIOUS: AtomicU64 = AtomicU64::new(0);
static UNCLAIMED: AtomicU64 = AtomicU64::new(0);

/// The MSI vector this GPU was given, once one was programmed
static VECTOR: AtomicU64 = AtomicU64::new(NO_VECTOR);
const NO_VECTOR: u64 = u64::MAX;

/// The vector and the capability it was written into, for `disable` to hand
/// back. Thread context only: the interrupt path reads `TREE` and `VECTOR`
static MSI: spin::Mutex<Option<(apic::MsiTarget, crate::drivers::bus::msi::MsiCapability)>> =
    spin::Mutex::new(None);

/// True once `enable` has programmed an MSI and the tree is live. Everything
/// with a polling fallback checks this
pub fn active() -> bool {
    VECTOR.load(Ordering::Relaxed) != NO_VECTOR
}

/// The tree's shape and the subtrees armed, once `enable` has run
pub fn shape() -> Option<(Geometry, u32)> {
    let t = *TREE.lock();
    active().then_some((t.geometry, t.arm))
}

/// (served, spurious, unclaimed, msi vector)
pub fn stats() -> (u64, u64, u64, Option<u8>) {
    let v = VECTOR.load(Ordering::Relaxed);
    (
        SERVED.load(Ordering::Relaxed),
        SPURIOUS.load(Ordering::Relaxed),
        UNCLAIMED.load(Ordering::Relaxed),
        if v == NO_VECTOR { None } else { Some(v as u8) },
    )
}

/// Install a handler for one vector. It runs in interrupt context: atomics and
/// MMIO only, no allocation, no `spin::Mutex`, no printing
pub fn register(vector: u32, handler: Handler) -> Result<(), &'static str> {
    let mut t = TREE.lock();
    if t.geometry.split(vector).is_none() {
        return Err("vector outside the tree");
    }
    for slot in t.handlers.iter_mut() {
        match slot {
            // Re-registering a vector replaces it, so a second bring-up does
            // not exhaust the array
            Some((v, _)) if *v == vector => {
                *slot = Some((vector, handler));
                return Ok(());
            }
            _ => {}
        }
    }
    match t.handlers.iter_mut().find(|s| s.is_none()) {
        Some(slot) => {
            *slot = Some((vector, handler));
            Ok(())
        }
        None => Err("no handler slots left"),
    }
}

/// Let one vector through. Called only for a vector with a handler; see the
/// note at the top
pub fn allow(vector: u32) -> Result<(), &'static str> {
    let t = *TREE.lock();
    let (leaf, bit) = t.geometry.split(vector).ok_or("vector outside the tree")?;
    let bar0 = t.bar0.ok_or("interrupt tree has no BAR0 yet")?;
    if !t.handlers.iter().any(|h| matches!(h, Some((v, _)) if *v == vector)) {
        return Err("no handler registered for that vector");
    }
    // Cleared, then unmasked, as in `nvkm_intr_allow_locked`: a bit going
    // pending between `enable` wiping the leaf and this call would fire when
    // let through, reporting an event that never happened
    bar0.write32(VFN_LEAF + leaf * 4, 1 << bit);
    bar0.write32(VFN_ALLOW + leaf * 4, 1 << bit);
    Ok(())
}

/// The whole interrupt path, on the MSI vector, so everything it touches is
/// interrupt-safe.
///
/// The order is `tu102_vfn_intr_pending` with the arm/rearm around it: stop the
/// tree, read the summary, read every leaf it points at, dispatch, clear what
/// was read, start the tree again
fn service() {
    let t = *TREE.lock();
    let Some(bar0) = t.bar0 else { return };

    bar0.write32(VFN_UNARM, t.arm);
    let top = bar0.read32(VFN_TOP);
    if top == 0 {
        SPURIOUS.fetch_add(1, Ordering::Relaxed);
        bar0.write32(VFN_REARM, t.arm);
        return;
    }

    for leaf in 0..t.geometry.leaves {
        if top & (1 << (leaf / 2)) == 0 {
            continue;
        }
        let stat = bar0.read32(VFN_LEAF + leaf * 4);
        if stat == 0 {
            continue;
        }
        let mut claimed = 0u32;
        for h in t.handlers.iter().flatten() {
            let (hleaf, hbit) = (h.0 / 32, h.0 % 32);
            if hleaf == leaf && stat & (1 << hbit) != 0 {
                claimed |= 1 << hbit;
                (h.1)(&bar0);
            }
        }
        // The whole leaf is cleared, not just what was claimed: a bit left set
        // re-asserts at once, and an interrupt storm is worse than a lost
        // event. The counter below shows a lost one
        if stat & !claimed != 0 {
            UNCLAIMED.fetch_add(1, Ordering::Relaxed);
        }
        bar0.write32(VFN_LEAF + leaf * 4, stat);
    }

    SERVED.fetch_add(1, Ordering::Relaxed);
    bar0.write32(VFN_REARM, t.arm);
}

/// Arm more subtrees: the non-stall one, once the vectors in it with a handler
/// are allowed (`_intrEnableStall_TU102` arms stall only; the non-stall subtree
/// is armed when something consumes it). Only what `allow` opened can arrive
/// through it
pub fn arm_also(subtrees: u32) {
    let (bar0, arm) = {
        let mut t = TREE.lock();
        let Some(bar0) = t.bar0 else { return };
        t.arm |= subtrees & t.geometry.subtree_mask();
        (bar0, t.arm)
    };
    bar0.write32(VFN_REARM, arm);
}

/// Give the card an MSI vector and start reading the tree.
///
/// Nothing is let through by this call: every leaf is blocked until `register`
/// and `allow`. `nonstall` is the subtrees GSP-RM put the non-stalling
/// interrupts in (`gsp_common::intr::KernelTable::nonstall_subtrees`); every
/// other subtree is armed. Returns the vector, for the log line
pub fn enable(
    gpu: &GpuDevice,
    bar0: MmioRegion,
    geometry: Geometry,
    nonstall: u32,
) -> Result<u8, &'static str> {
    if crate::arch::x86_64::grub::cmdline_flag("nointr") {
        return Err("nointr on the command line");
    }
    if active() {
        return Err("already enabled");
    }
    let caps = crate::drivers::bus::msi::read_caps(gpu.pci_fn());
    // MSI, not MSI-X: one vector carries the whole tree. `program_msi` writes
    // the MSI capability only
    let cap = caps.msi.ok_or("no MSI capability on this device")?;

    let arm = geometry.subtree_mask() & !nonstall;
    {
        let mut t = TREE.lock();
        t.bar0 = Some(bar0);
        t.geometry = geometry;
        t.arm = arm;
    }

    // A clean slate, before the card can send an interrupt: the per-leaf masks
    // hold whatever GSP-RM's bring-up left. Every subtree unarmed, every leaf
    // register blocked, every pending bit cleared, so only a bit `allow` opened
    // can arrive
    bar0.write32(VFN_UNARM, u32::MAX);
    for leaf in 0..geometry.leaf_regs {
        bar0.write32(VFN_BLOCK + leaf * 4, 0xffff_ffff);
        bar0.write32(VFN_LEAF + leaf * 4, 0xffff_ffff);
    }

    let target = apic::alloc_msi_vector(apic::lapic_id(), |_| service(), 0)
        .ok_or("no MSI vector free")?;
    if let Err(e) = crate::drivers::bus::msi::program_msi(gpu.pci_fn(), &cap, target.address, target.data) {
        apic::free_msi_vector(target);
        let mut t = TREE.lock();
        t.bar0 = None;
        t.geometry = Geometry::NONE;
        t.arm = 0;
        return Err(e);
    }

    // Armed last. Until this write the tree raises nothing, whatever the
    // per-leaf masks say
    bar0.write32(VFN_REARM, arm);
    *MSI.lock() = Some((target, cap));
    VECTOR.store(target.vector as u64, Ordering::Release);
    Ok(target.vector)
}

/// Take the tree down: `enable` in reverse.
///
/// Unarmed and every leaf blocked and cleared first, so the card has nothing to
/// raise; then the MSI enable bit off, so it has nowhere to send it; then the
/// vector back to the APIC and the handlers gone. OpenRM's `intrStateUnload`
/// leaves the tree disabled the same way (`_intrSetIntrEn`,
/// `INTERRUPT_TYPE_DISABLED`). Afterwards `active()` is false and every wait
/// polls, as an unload's own waits need.
///
/// Returns false when there was no tree to take down
pub fn disable(gpu: &GpuDevice) -> bool {
    let t = *TREE.lock();
    let Some(bar0) = t.bar0 else { return false };

    // The hardware first, with the handlers still installed: an interrupt
    // landing in the middle is serviced
    bar0.write32(VFN_UNARM, u32::MAX);
    for leaf in 0..t.geometry.leaf_regs {
        bar0.write32(VFN_BLOCK + leaf * 4, 0xffff_ffff);
        bar0.write32(VFN_LEAF + leaf * 4, 0xffff_ffff);
    }
    let msi = MSI.lock().take();
    if let Some((_, cap)) = msi {
        crate::drivers::bus::msi::disable_msi(gpu.pci_fn(), &cap);
    }

    // Then the software, and the vector last: once free the APIC may give it to
    // somebody else
    VECTOR.store(NO_VECTOR, Ordering::Release);
    {
        let mut t = TREE.lock();
        t.bar0 = None;
        t.geometry = Geometry::NONE;
        t.arm = 0;
        t.handlers = [None; MAX_HANDLERS];
    }
    if let Some((target, _)) = msi {
        apic::free_msi_vector(target);
    }
    true
}
