// Framebuffer NVIDIA BAR correlation
//
// On a machine whose primary display adapter is an NVIDIA card, the boot
// framebuffer set up by the firmware (UEFI GOP or legacy VBE, reported via
// GRUB's multiboot2 'framebuffer' tag) is physically in the card's BAR1, the
// large prefetchable aperture onto video RAM.
//
// Given the boot framebuffer's physical address, this finds which GPU BAR (if
// any) contains it. The BAR index and offset are stored on the Gtx1650 struct

use super::pci::GpuDevice;

/// Where a physical address falls inside a GPU's BARs. Carries the whole
/// window: which BAR, and where that BAR is
#[allow(dead_code)]
#[derive(Copy, Clone, Debug)]
pub struct FbLocation {
    pub bar_index: u8,
    pub offset:    u64,
    pub bar_phys:  u64,
    pub bar_size:  u64,
}

/// Return the BAR that covers 'phys' on this GPU, if any
pub fn find_in_bars(gpu: &GpuDevice, phys: u64) -> Option<FbLocation> {
    for (i, bar) in gpu.bars.iter().enumerate() {
        if bar.is_io || bar.phys == 0 || bar.size == 0 {
            continue;
        }
        let start = bar.phys;
        let end = bar.phys.saturating_add(bar.size);
        if phys >= start && phys < end {
            return Some(FbLocation {
                bar_index: i as u8,
                offset:    phys - start,
                bar_phys:  bar.phys,
                bar_size:  bar.size,
            });
        }
    }
    None
}
