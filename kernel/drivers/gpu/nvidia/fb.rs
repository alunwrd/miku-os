//////////////////////////////////////////////////////////////////////////////////
// Framebuffer NVIDIA BAR correlation                                           //
//                                                                              //
// On a machine whose primary display adapter is an NVIDIA card, the boot       //
// framebuffer set up by the firmware (UEFI GOP or legacy VBE, reported to      //
// us via GRUB's multiboot2 'framebuffer' tag) is physically located in the     //
// card's BAR1 (the large prefetchable aperture that maps onto video RAM)       // 
// Writing a pixel into that buffer drives the card's scanout hardware and      //
// the image appears on the monitor                                             //
//                                                                              //
// This module gives a tiny helper used during driver init: given the boot      //
// framebuffer's physical address, find which GPU BAR (if any) contains it      //
// The BAR index and offset are then stored on the Gtx1650 struct so later      //
// code can assert, log, or reuse the mapping                                   //
//////////////////////////////////////////////////////////////////////////////////

use super::pci::GpuDevice;

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
