// Facade. `blackwell::boot::X` resolves here for callers outside this directory
// (`shell/commands/nvidia_cmds.rs`, `mayland/render/`, `nvidia/scanout.rs`),
// since the sibling modules are private to `blackwell`.

pub use super::chanmem::{CHAN_PDB, internal_client};
pub use super::control::{imp_opinion, imp_opinion_of, light, perf_snapshot, poll_events, set_vrr, settle_report, underflow, with_rm};
pub use super::modeset::mode_possible;
pub use crate::drivers::gpu::nvidia::gsp_common::kgsp::events::trouble_counts;
pub use super::interrupts::{GEOMETRY as INTR_GEOMETRY, gsp_falcon_irq, stats as intr_stats};
pub use super::channel::{ce_after, ce_fence_state, ce_fence_target, ce_health, ce_idle, ce_submit_client_batch, chan_cycle, ce_copy_batch, ce_copy_to_vram, ce_submit_batch_after, ce_submit_batch_to_vram, ce_wait_last, copy_engine_ready, unstick_engines};
pub use super::compute_chan::{ClientLaunch, DispatchError, FenceState, blur_init, blur_surface, client_bind, client_dispatch, client_drop, client_forget_programs, client_has_own, client_load_program, client_own_gpu, client_unbind, compute_epoch, compute_fence_state, compute_ready, compute_test, compute_wait_last, effects_base, effects_map, effects_retarget, effects_submit, effects_unbind, program_names};
pub use super::gspboot::{BootError, BootFlags, GspFmcBootParams, GSP_FMC_BOOT_PARAMS_SIZE, RSVD_SIZE_PMU, frts_vidmem_offset, is_booted, with_disp_mem, with_rm_state, wpr_heap_size_for};
pub use super::hal_gb20x::{is_wpr2_up, poll_bar_firewall};
pub use super::bringup::{boot, boot_opts, reset_and_reload, shutdown, take_rm_log, unload};
pub use super::window::{SURFACE_ALIGN, clear_live_cursor, live_cursor, live_scanout, set_live_surface};
