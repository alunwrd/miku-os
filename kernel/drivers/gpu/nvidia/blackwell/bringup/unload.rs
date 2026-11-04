// Taking a booted GSP-RM down, so it can be booted again
//
// `dev/gsp-foundation.md`, section 14. OpenRM on a driver unload: the engines
// in reverse (`gpuStateUnload`), then `kgspUnloadRm` from `gpuStateDestroy`
// (`gpu.c:3272`). The processes owning the clients freed them first; here the
// kernel is the one client and frees its own tree between the two.
//
//   unload     the shell: every engine down, the client freed, GSP-RM told
//              and waited for with OpenRM's timeouts, WPR2 checked, and every
//              record of that GSP-RM forgotten, so `boot_opts` can run again
//   shutdown   the way to a reboot: the short budget, a second in all

use crate::drivers::gpu::nvidia::said;
use crate::kcore::klog::Level;
use crate::drivers::gpu::nvidia::gsp_common::kgsp::unload::{self as kgsp_unload, Budget, UnloadReport};
use crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc;
use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::drivers::gpu::nvidia::pci::GpuDevice;

use super::super::gspboot::{self, DisplayMemory, RM};
use super::super::hal_gb20x;

/// What a whole unload did
#[derive(Clone, Copy, Debug)]
pub struct Unloaded {
    pub gsp: UnloadReport,
    /// `NV_PFB_PRI_MMU_WPR2_ADDR_HI` after the halt
    pub wpr2_hi: u32,
    /// The rings and the boot image went back to the allocator. False when the
    /// core did not halt or WPR2 stayed up: they stay pinned for good
    pub freed: bool,
}

/// The shell's unload. Refused while anything still uses the card
pub fn unload(bar0: &MmioRegion, gpu: &GpuDevice, say: &dyn Fn(&str)) -> Result<Unloaded, &'static str> {
    unload_with(bar0, gpu, false, Budget::OPENRM, say)
}

/// A recovery's unload (`recover.rs`): nothing waited for. The caller stopped
/// the compositor; a process's own GPU is dropped here, since a dead GSP-RM
/// took its channel with it
pub fn unload_forced(bar0: &MmioRegion, gpu: &GpuDevice, say: &dyn Fn(&str)) -> Result<Unloaded, &'static str> {
    unload_with(bar0, gpu, true, Budget::OPENRM, say)
}

fn unload_with(
    bar0: &MmioRegion,
    gpu: &GpuDevice,
    force: bool,
    budget: Budget,
    say: &dyn Fn(&str),
) -> Result<Unloaded, &'static str> {
    if !gspboot::is_booted() {
        return Err("GSP-RM is not loaded");
    }
    if crate::mayland::render::running() {
        return Err("the compositor is running - stop it first (mayland stop)");
    }
    // A program's video memory is a window onto a block of it through BAR1
    // (3.7). After a reload that block is somebody else's, so the windows go
    // before GSP-RM: refused while a program holds one, revoked when forced,
    // leaving the program a handle that names nothing
    let (held, _) = crate::drivers::gpu::nvidia::user_vram::held();
    if held != 0 {
        if !force {
            return Err("programs hold video memory - end them first");
        }
        let n = crate::mayland::server::shm::revoke_video();
        crate::drivers::gpu::nvidia::user_vram::forget();
        say(&alloc::format!("{} program video-memory pool(s) revoked", n));
    }
    let users = super::super::compute_chan::users();
    if !users.is_empty() {
        say(&alloc::format!("pids holding the GPU: {:?}", users));
        if !force {
            return Err("processes still hold the GPU - end them first");
        }
        // Their own address spaces and channels. What they loaded into the
        // kernel's goes with the programs when the channels come down
        for pid in users {
            if super::super::compute_chan::client_has_own(pid) {
                super::super::compute_chan::client_drop(pid, say);
            }
        }
    }

    // Variable refresh off while RM can be asked: the next boot inherits a head
    // left stall-locked. A forced unload has no RM to ask, and the reset after
    // it clears the head
    if !force {
        super::super::control::vrr_all_off(say);
    }

    // The rings out of `RM`, so from here other callers find no GSP-RM. Taking
    // the lock waits out an RPC in flight, most likely the event thread's drain
    let Some(mut rm) = RM.lock().take() else {
        return Err("GSP-RM is booted and has no rings");
    };
    let mut rpc = GspRpc::new(bar0, &mut rm.bootargs);

    // The engines, and then the kernel's client with everything under it
    match super::super::chanmem::rm_client() {
        Some(client) => {
            let mut scratch = DisplayMemory::default();
            super::super::engines::unload(
                &mut super::super::engines::BootCtx {
                    bar0, gpu, rpc: &mut rpc, client: &client, disp_mem: &mut scratch, say,
                },
                say,
            );
            match client.free(&mut rpc, client.h_client) {
                Ok(()) => say("RM: client freed, and every object under it"),
                Err(e) => said(Level::Warn, &say, &alloc::format!("RM: freeing the client failed: {:?} - going on", e)),
            }
        }
        // RM never answered this boot, so no engine came up; only the interrupt
        // tree may have
        None => super::super::interrupts::gsp_unload(gpu, say),
    }

    // BAR1 as GSP-RM left it, before GSP-RM is told to go
    // (`dev/gpu-memory-compute.md`, 3.1)
    if let Err(e) = super::super::bar1::off(bar0) {
        said(Level::Error, &say, &alloc::format!("BAR1: binding it back failed: {}", e));
    }
    let gsp = kgsp_unload::unload_rm(&mut rpc, bar0, budget);
    drop(rpc);
    let wpr2_hi = hal_gb20x::wpr2_hi(bar0);
    let wpr2_down = !hal_gb20x::is_wpr2_up(wpr2_hi);

    // Everything that named this GSP-RM
    super::super::chanmem::forget();
    crate::drivers::gpu::nvidia::gsp_common::vram::forget();
    crate::drivers::gpu::nvidia::gsp_common::perf::forget();
    kgsp_unload::forget();
    let freed = gspboot::uninstall(rm, gsp.halted, wpr2_down);
    // The crash queue is GSP-RM's to write to as long as the rings are, and so
    // is the page it writes its clocks and temperatures into
    crate::drivers::gpu::nvidia::gsp_common::kgsp::crashcat::release(freed);
    crate::drivers::gpu::nvidia::gsp_common::perf::rusd::release(freed);

    say(&alloc::format!(
        "GSP: unload rpc {}, suspended {} (mbox0 {:#x}), halted {} (cpuctl {:#010x}), WPR2 {} (hi {:#010x}), {}",
        if gsp.rpc_ok { "ok" } else { "FAILED" },
        gsp.suspended, gsp.mbox0, gsp.halted, gsp.cpuctl,
        if wpr2_down { "down" } else { "STILL UP" }, wpr2_hi,
        if freed { "boot image and rings freed" } else { "boot image and rings kept pinned - reset the GPU before booting it again" },
    ));
    Ok(Unloaded { gsp, wpr2_hi, freed })
}

/// The reboot's unload: the same as `nvidia gsp unload` (heads dark, engines
/// down, the client and everything under it freed, then GSP-RM told) with the
/// short budget, and nobody's GPU waited for. `None` when nothing was booted.
///
/// OpenRM's `RmShutdownAdapter` frees every client's resources on the GPU
/// (`rmapiDelPendingDevices`) and unloads the engines (`gpuStateUnload`) before
/// GSP-RM is unloaded. UNLOADING_GUEST_DRIVER with objects alive makes GSP-RM
/// take a load page fault inside the unload (`stval 0x5c`, a NULL member)
pub fn shutdown(bar0: &MmioRegion, gpu: &GpuDevice, say: &dyn Fn(&str)) -> Option<UnloadReport> {
    if !gspboot::is_booted() {
        return None;
    }
    match unload_with(bar0, gpu, true, Budget::SHUTDOWN, say) {
        Ok(u) => Some(u.gsp),
        Err(e) => {
            said(Level::Warn, &say, &alloc::format!("the reboot's unload did not run: {}", e));
            None
        }
    }
}
