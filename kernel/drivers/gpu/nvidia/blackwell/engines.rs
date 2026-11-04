// A GB20x's engines, in the order they come up
//
// `gpuStateLoad` order for this chip, as far as this driver has engines:
//
//   memsys     the sysmem flush buffer programmed again and checked
//              (`kmemsysStatePreLoad_IMPL`); allocated before the COT
//   gsp intr   the MSI, the tree and the GSP's own vector, first, so every
//              RPC after it sleeps while it waits, and GSP-RM can reach the
//              driver while the display comes up
//   perf       the page GSP-RM writes its clocks, power and temperatures into
//              (`gpu_user_shared_data.c`, created in memmgr's state init, as
//              early; `dev/gpu-perf.md`)
//   display    the outputs, a modeset, the window, and then the display's own
//              vectors (DISP and DISP_LOW)
//   channels   a page directory handed to RM, the copy engine's channel and
//              the compute channel built in it
//
// RM comes up on its own (`bringup::rm::static_info`), and the engines load
// after it in this order. `bringup::unload` runs the list backwards
// (`dev/gsp-foundation.md`, 1.11-1.14).

use crate::drivers::gpu::nvidia::gsp_common::kgsp::engine::{self, Engine};
use crate::drivers::gpu::nvidia::speaking;
use crate::drivers::gpu::nvidia::gsp_common::rmapi::{status_str, RmClient, RmError};
use crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc;
use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::drivers::gpu::nvidia::pci::GpuDevice;

use super::chanmem::CHAN_PDB;
use super::gspboot::DisplayMemory;

/// What an engine is handed as it comes up, and as it goes down.
///
/// An unload hands a `disp_mem` no engine reads: a head may still be fetching
/// from the display's memory (`gspboot::uninstall` retires it)
pub(super) struct BootCtx<'a, 'r> {
    pub bar0: &'a MmioRegion,
    pub gpu: &'a GpuDevice,
    pub rpc: &'a mut GspRpc<'r>,
    pub client: &'a RmClient,
    pub disp_mem: &'a mut DisplayMemory,
    pub say: &'a dyn Fn(&str),
}

struct Memsys;
struct GspIntr;
struct Perf;
struct Display;
struct Channels;

impl Engine<BootCtx<'_, '_>> for Memsys {
    fn name(&self) -> &'static str { "memsys" }
    fn load(&self, cx: &mut BootCtx) -> Result<(), &'static str> {
        // Boot A never programmed it, and `gspbase` is boot A
        if super::bringup::base_init() {
            return Ok(());
        }
        super::memsys::state_load(cx.bar0)
    }
    /// Nothing: `kmemsysStateUnload` does nothing outside a power transition,
    /// and the flush buffer stays allocated and programmed (`memsys.rs`)
    fn unload(&self, _cx: &mut BootCtx) {}
}

impl Engine<BootCtx<'_, '_>> for GspIntr {
    fn name(&self) -> &'static str { "gsp interrupt" }
    fn load(&self, cx: &mut BootCtx) -> Result<(), &'static str> {
        let say = speaking(&super::interrupts::SITE, cx.say);
        super::interrupts::gsp_load(cx.bar0, cx.gpu, cx.rpc, &say)
    }
    fn unload(&self, cx: &mut BootCtx) {
        let say = speaking(&super::interrupts::SITE, cx.say);
        super::interrupts::gsp_unload(cx.gpu, &say);
    }
}

impl Engine<BootCtx<'_, '_>> for Perf {
    fn name(&self) -> &'static str { "perf" }
    /// The page linked and nothing polled yet: polling starts when somebody
    /// reads (`rusd::touch`). On the internal client, as OpenRM sends it. Not
    /// having it costs only the numbers
    fn load(&self, cx: &mut BootCtx) -> Result<(), &'static str> {
        use crate::drivers::gpu::nvidia::gsp_common::perf::{self, rusd};
        let say = speaking(&perf::SITE, cx.say);
        let internal = super::chanmem::internal_client()
            .ok_or("no internal RM handles - GSP_INIT did not answer")?;
        match rusd::link(cx.rpc, &internal) {
            Ok(phys) => {
                let (_, _, refused) = rusd::state();
                say(&alloc::format!(
                    "PERF: shared data page at {:#x}, GSP-RM took it; {}", phys,
                    if refused { "it refused the polling interval, so it will not be polled" }
                    else { "polled while read, every 500 ms" }
                ));
                Ok(())
            }
            Err(rusd::LinkError::NoPage) => Err("no page of system memory for GSP-RM's numbers"),
            Err(rusd::LinkError::Rm(e)) => {
                crate::drivers::gpu::nvidia::said(crate::kcore::klog::Level::Warn, &say,
                    &alloc::format!("PERF: GSP-RM refused the shared data page: {:?}", e));
                Err("GSP-RM has no page for its numbers")
            }
        }
    }
    /// GSP-RM told to stop writing the page; the page itself waits for the
    /// unload's verdict (`rusd::release` in `bringup::unload`)
    fn unload(&self, cx: &mut BootCtx) {
        use crate::drivers::gpu::nvidia::gsp_common::perf::{self, rusd};
        let say = speaking(&perf::SITE, cx.say);
        let Some(internal) = super::chanmem::internal_client() else { return };
        if let Err(e) = rusd::unlink(cx.rpc, &internal) {
            say(&alloc::format!("PERF: GSP-RM did not take the shared data page back: {:?}", e));
        }
    }
}

impl Engine<BootCtx<'_, '_>> for Display {
    fn name(&self) -> &'static str { "display" }
    fn load(&self, cx: &mut BootCtx) -> Result<(), &'static str> {
        let say = speaking(&super::display::SITE, cx.say);
        super::display::enumerate_displays(cx.bar0, cx.rpc, cx.client, cx.disp_mem, &say);
        super::interrupts::disp_load(cx.bar0, &say)
    }
    /// Every lit head shut down as NVKMS does before its core channel goes,
    /// then the vblank off: the console detached, the flip, the live scanout
    /// and the cursor forgotten, the output table emptied. RM's display objects
    /// go with the client
    fn unload(&self, cx: &mut BootCtx) {
        use crate::drivers::gpu::nvidia::gsp_common::output;
        // Dark first, while the table still says which heads are lit and RM
        // can still be told about the links (`dev/display-nvkms.md`, 7)
        let say = speaking(&super::display::SITE, cx.say);
        super::display::shut_down_heads(cx.bar0, cx.rpc, cx.client, &say);
        super::interrupts::disp_unload(cx.bar0);
        crate::drivers::gpu::nvidia::scanout::forget();
        output::clear_live_scanout();
        output::clear_live_cursor();
        output::reset();
        crate::drivers::gpu::nvidia::gsp_common::modeset::vrr::forget();
        crate::drivers::gpu::nvidia::gsp_common::modeset::imp::forget_floors();
        crate::drivers::gpu::nvidia::gsp_common::dispcaps::set(None);
    }
}

impl Engine<BootCtx<'_, '_>> for Channels {
    fn name(&self) -> &'static str { "channels" }
    fn load(&self, cx: &mut BootCtx) -> Result<(), &'static str> {
        use crate::drivers::gpu::nvidia::gsp_common::chan::{ChanVaSpace, H_VASPACE};
        let say = speaking(&super::channel::SITE, cx.say);
        let say = &say;

        // The address space the copy engine's channel will run in.
        //
        // RM accepts the page directory built here or names the reason it will
        // not. `CHAN_PDB` is carved as RM comes up
        // (`bringup::rm::static_info`), the first place with the FB layout
        let pdb = *CHAN_PDB.lock();
        if pdb == 0 {
            say("RM: no video memory carved for a channel page directory");
            return Err("no page directory to build the channels in");
        }
        // A page directory starts empty, and the CPU's only path to video
        // memory is the one the console uses
        {
            let mut w = crate::drivers::gpu::nvidia::pramin::Window::new(cx.bar0);
            for off in (0..4096u64).step_by(4) {
                w.write32(pdb + off, 0);
            }
        }
        match ChanVaSpace::create(cx.rpc, cx.client, pdb, H_VASPACE) {
            Ok(vas) => {
                say(&alloc::format!(
                    "RM: channel vaspace {:#x} owns pdb {:#x} - RM took our page directory",
                    vas.handle, vas.pdb
                ));
                crate::drivers::gpu::nvidia::vaspace::create(vas.pdb, "kernel", &super::chanmem::layout());
                super::channel::build_channel(cx.bar0, cx.rpc, cx.client, &vas, say);
                Ok(())
            }
            Err(RmError::Status(s)) => {
                say(&alloc::format!("RM: channel vaspace refused: {}", status_str(s)));
                Err("RM refused the channels' address space")
            }
            Err(e) => {
                say(&alloc::format!("RM: channel vaspace failed: {:?}", e));
                Err("the channels' address space could not be made")
            }
        }
    }
    /// Both channels down as `chan-cycle` takes them, then every record of what
    /// was built in their address space: the compute context and programs, the
    /// blur, the page tables, the retired frames
    fn unload(&self, cx: &mut BootCtx) {
        use crate::drivers::gpu::nvidia::{blur, gmmu, gsp_common::chan};
        let say = speaking(&super::channel::SITE, cx.say);
        let pdb = *CHAN_PDB.lock();
        if pdb != 0 && !super::channel::ce_destroy(cx.bar0, cx.rpc, cx.client, pdb, &say) {
            say("RM: no channel was up");
        }
        super::compute_chan::forget();
        blur::forget();
        gmmu::forget();
        let freed = chan::forget();
        say(&alloc::format!("RM: channels down, {} retired frame(s) freed", freed));
    }
}

/// The GB20x order
const ENGINES: [&(dyn for<'a, 'r> Engine<BootCtx<'a, 'r>> + Sync); 5] = [&Memsys, &GspIntr, &Perf, &Display, &Channels];

/// Bring every engine up, in the GB20x order
pub(super) fn load(cx: &mut BootCtx, say: &dyn Fn(&str)) {
    let engines: [&dyn Engine<BootCtx>; 5] = ENGINES.map(|e| e as &dyn Engine<BootCtx>);
    let up = engine::load_all(&engines, cx, say);
    say(&alloc::format!("ENGINES: {} of {} up", up, engines.len()));
}

/// Take every engine down, in the reverse order
pub(super) fn unload(cx: &mut BootCtx, say: &dyn Fn(&str)) {
    let engines: [&dyn Engine<BootCtx>; 5] = ENGINES.map(|e| e as &dyn Engine<BootCtx>);
    engine::unload_all(&engines, cx, say);
}
