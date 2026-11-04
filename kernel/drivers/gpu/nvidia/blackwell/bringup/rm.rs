// After GSP_INIT: RM, and then the engines
//
// The client -> device -> subdevice chain and one question with a known answer,
// INTx masked, and what GSP_INIT brought back: the framebuffer layout the
// video-memory allocator is seeded from, and the internal client's handles
// (`static_info`). That is RM coming up (the tail of `kgspInitRm_IMPL`), with
// no need of a display.
//
// Then the engines, in the chip's order (`engines.rs`): the GSP's interrupt,
// the display, the channels (`dev/gsp-foundation.md`, step 1.11).

use crate::drivers::gpu::nvidia::said;
use crate::kcore::klog::Level;
use super::super::chanmem::{CHAN_INST, CHAN_INST_COMPUTE, CHAN_INTERNAL, CHAN_PDB, CHAN_SCRATCH};
use super::super::gspboot::{BootReport, DisplayMemory};
use super::record;

pub(super) fn bring_up(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    gpu: &crate::drivers::gpu::nvidia::pci::GpuDevice,
    rpc: &mut crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc,
    si: &crate::drivers::gpu::nvidia::gsp_common::staticinfo::StaticInfo,
    disp_mem: &mut DisplayMemory,
    report: &mut BootReport,
) {
    use crate::drivers::gpu::nvidia::gsp_common::rmapi::{
        status_str, PciInfo, RmClient, RmError, NV2080_CTRL_CMD_BUS_GET_PCI_INFO,
    };
    // Kept as well as printed: this scrolls off a 1080p console, so it is
    // repeated at the end, on the panel
    let say = |s: &str| {
        crate::println!("      {}", s);
        crate::pstore_println!("{}", s);
        record(s);
    };
    say(&alloc::format!(
        "RM: GSP-RM {} up, \"{}\", RPC version {}.{}",
        crate::drivers::gpu::nvidia::gsp_common::GSP_RM_VERSION, si.gpu_name,
        si.vgx_version >> 16, si.vgx_version & 0xffff
    ));
    match RmClient::bootstrap(rpc) {
        Ok(client) => {
            report.rm_client = true;
            say("RM: client/device/subdevice allocated");
            // Published for after the boot. A later display change (a mode
            // change, a blank, a monitor arriving) needs a client, and a second
            // one per command would leak a chain on every failure
            *super::super::chanmem::RM_CLIENT.lock() =
                Some((client.h_client, client.h_device, client.h_subdevice));
            match client.control_query(
                rpc, client.h_subdevice,
                NV2080_CTRL_CMD_BUS_GET_PCI_INFO, PciInfo::SIZE,
            ) {
                Ok(buf) => match PciInfo::parse(&buf) {
                    Some(info) => {
                        report.rm_control = true;
                        // devid from RM against devid from config space: equal
                        // means the whole path (alloc, control, parameter
                        // marshalling) is right
                        say(&alloc::format!(
                            "RM: pci {:04x}:{:04x} subsys {:04x}:{:04x} rev {:#x} (pci says {:04x})",
                            info.vendor(), info.device(),
                            info.subsystem_vendor(), info.subsystem_device(),
                            info.revision_id, gpu.device
                        ));
                        if info.device() != gpu.device {
                            said(Level::Error, &say, "RM: devid MISMATCH - the control path is marshalling wrong");
                        } else {
                            // ext_device_id is documented as the same device id
                            // unpacked, so agreement says the high/low split is
                            // read right
                            say(&alloc::format!(
                                "RM: devid matches config space; ext devid {:#06x}",
                                info.ext_device_id
                            ));
                        }
                    }
                    None => say("RM: BUS_GET_PCI_INFO reply too short"),
                },
                Err(RmError::Status(s)) => {
                    said(Level::Error, &say, &alloc::format!("RM: BUS_GET_PCI_INFO failed: {}", status_str(s)))
                }
                Err(e) => said(Level::Error, &say, &alloc::format!("RM: BUS_GET_PCI_INFO failed: {:?}", e)),
            }

            // A scanning head raises vblank and supervisor interrupts, so the
            // legacy INTx line is masked before any can fire.
            //
            // It stays masked: the handler is on an MSI, the two are mutually
            // exclusive, and the first engine to load (`interrupts::gsp_load`)
            // gives the card somewhere else to send them
            say(&alloc::format!(
                "PCI: command {:#06x} before masking INTx",
                crate::drivers::gpu::nvidia::pci::read_command(gpu)
            ));
            crate::drivers::gpu::nvidia::pci::disable_intx(gpu);
            say(&alloc::format!(
                "PCI: command {:#06x} after",
                crate::drivers::gpu::nvidia::pci::read_command(gpu)
            ));
            static_info(si, &say);
            super::super::engines::load(
                &mut super::super::engines::BootCtx {
                    bar0, gpu, rpc, client: &client, disp_mem, say: &say,
                },
                &say,
            );
        }
        Err(RmError::Status(s)) => {
            said(Level::Error, &say, &alloc::format!("RM: bootstrap failed: {}", status_str(s)))
        }
        Err(e) => said(Level::Error, &say, &alloc::format!("RM: bootstrap failed: {:?}", e)),
    }
}

/// GSP-RM's static information: the framebuffer layout the video-memory
/// allocator is seeded from, and the internal client's handles.
///
/// Taken as soon as RM answers, as `kgspInitRm_IMPL` does: a card with no
/// monitor needs the allocator and the handles too. The channels' blocks are
/// carved here, where the layout is first known
fn static_info(
    si: &crate::drivers::gpu::nvidia::gsp_common::staticinfo::StaticInfo,
    say: &dyn Fn(&str),
) {
    use crate::drivers::gpu::nvidia::gsp_common::vram::{self, Placement};

    let usable: alloc::vec::Vec<_> = si.usable_regions().collect();
    say(&alloc::format!(
        "RM: fb {} MiB, {} regions ({} usable)",
        si.fb_length >> 20, si.regions.len(), usable.len()
    ));
    // Everything below picks its address from this layout. The allocator is
    // seeded first, so each pick can be registered against it (see `hold`)
    crate::drivers::gpu::nvidia::gsp_common::vram::init(si);

    // Ask for a block instead of naming one.
    //
    // `Low` on all but the display's instance memory: the top of video memory
    // is GSP-RM's write-protected region and its heap, and a scanout surface
    // there reads back black without saying why
    let take = |size: u64, align: u64, place: Placement, owner: &'static str| -> u64 {
        match vram::alloc(size, align, place, owner) {
            Some(a) => a,
            None => {
                say(&alloc::format!(
                    "VRAM: no room for {} ({} KiB)", owner, size >> 10
                ));
                0
            }
        }
    };

    // The page directory RM built for BAR1, for `attach_window`. And one page
    // of video memory for a page directory of our own, the root of the copy
    // engine channel's address space, carved here where the FB layout is known
    // and used once the RM client exists
    *CHAN_PDB.lock() = take(4 << 10, 4 << 10, Placement::Low, "channel page directory");
    // Logged with the `RM:` prefix, for the closing block. An address here with
    // a refusal below means ordering is wrong; a zero here means the carve
    // failed against the assumed FB layout
    say(&alloc::format!(
        "RM: channel page directory carved at {:#x}", *CHAN_PDB.lock()
    ));
    // And the instance block with USERD a page above it, clear of the page
    // directory below and the scanout surface above
    *CHAN_INST.lock() = take(
        crate::drivers::gpu::nvidia::gsp_common::chan::CHAN_MEM_SIZE,
        4 << 10, Placement::Low, "copy-engine channel instance",
    );
    *CHAN_SCRATCH.lock() = take(0x2000, 4 << 10, Placement::Low, "copy-engine scratch");
    *CHAN_INST_COMPUTE.lock() = take(
        crate::drivers::gpu::nvidia::gsp_common::chan::CHAN_MEM_SIZE,
        4 << 10, Placement::Low, "compute channel instance",
    );

    // The internal client owns the internal subdevice, so the triple travels
    // together, published for everything after the boot that talks to GSP-RM's
    // own objects
    *CHAN_INTERNAL.lock() = Some((
        si.h_internal_client, si.h_internal_device, si.h_internal_subdevice,
    ));
}
