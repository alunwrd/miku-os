// The memory system's one job before GSP-RM: the sysmem flush buffer
//
// A sysmembar, the GPU making its writes to system memory visible, ends with a
// zero-byte read of system memory at the address in
// `NV_PFB_FBHUB0_PCIE_FLUSH_SYSMEM_ADDR`. The page must exist and stay
// allocated, since a real read may be issued
// (`kmemsysInitFlushSysmemBuffer_GA100`, `kern_mem_sys_ga100.c:80`).
//
// OpenRM's kernel side sets it up in `kmemsysConstructEngine_IMPL`
// (`kern_mem_sys.c:74`), long before GSP-RM boots, since resetting falcons
// during VBIOS init already needs a system flush. It programs it again in
// `kmemsysStatePreLoad_IMPL` (`:238`), and GSP-RM, which does not program the
// register, asserts it is valid (`dev/gsp-foundation.md`, step 1.12).
//
// The page is allocated once and never freed: a GPU unloaded and loaded again
// keeps the same buffer and is only programmed again.

use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::mm::dma::DmaBuffer;
use crate::serial_println;

use super::regs::*;

static FLUSH: spin::Mutex<Option<DmaBuffer>> = spin::Mutex::new(None);

/// What the two registers read, as one address
fn programmed(bar0: &MmioRegion) -> u64 {
    let lo = bar0.read32(PFB_FBHUB0_PCIE_FLUSH_SYSMEM_ADDR_LO) as u64;
    let hi = (bar0.read32(PFB_FBHUB0_PCIE_FLUSH_SYSMEM_ADDR_HI) & FLUSH_SYSMEM_ADDR_HI_MASK) as u64;
    hi << 32 | lo
}

/// `kmemsysProgramSysmemFlushBuffer_GB10B`: the high half first, then the low
fn program(bar0: &MmioRegion, addr: u64) {
    let hi = (addr >> 32) as u32;
    if hi & !FLUSH_SYSMEM_ADDR_HI_MASK != 0 {
        serial_println!("[gb206/memsys] flush buffer {:#x} is above 52 bits", addr);
    }
    bar0.write32(PFB_FBHUB0_PCIE_FLUSH_SYSMEM_ADDR_HI, hi & FLUSH_SYSMEM_ADDR_HI_MASK);
    bar0.write32(PFB_FBHUB0_PCIE_FLUSH_SYSMEM_ADDR_LO, addr as u32);
}

/// Allocate the page on the first boot, and program it
/// (`kmemsysInitFlushSysmemBuffer_GA100`). Called before the COT. Returns the
/// address, or 0 with no page to give
pub(super) fn init_flush_buffer(bar0: &MmioRegion) -> u64 {
    let mut slot = FLUSH.lock();
    if slot.is_none() {
        match DmaBuffer::alloc(1) {
            Ok(mut b) => {
                b.zero();
                *slot = Some(b);
            }
            Err(_) => {
                serial_println!("[gb206/memsys] no page for the sysmem flush buffer");
                return 0;
            }
        }
    }
    let addr = slot.as_ref().map_or(0, |b| b.phys());
    let before = programmed(bar0);
    program(bar0, addr);
    serial_println!(
        "[gb206/memsys] FLUSH @{:#x} (was {:#x}), readback {:#x}",
        addr, before, programmed(bar0)
    );
    addr
}

/// `kmemsysStatePreLoad_IMPL`: program it again once GSP-RM is up, and check
/// that the register holds an address
/// (`kmemsysAssertSysmemFlushBufferValid_GB10B`)
pub(super) fn state_load(bar0: &MmioRegion) -> Result<(), &'static str> {
    let addr = FLUSH.lock().as_ref().map_or(0, |b| b.phys());
    if addr == 0 {
        return Err("no sysmem flush buffer was allocated");
    }
    program(bar0, addr);
    let back = programmed(bar0);
    if back != addr {
        serial_println!("[gb206/memsys] FLUSH readback {:#x}, wrote {:#x}", back, addr);
        return Err("the sysmem flush buffer did not stick");
    }
    Ok(())
}
