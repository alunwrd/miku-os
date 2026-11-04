// A window's last frame, kept for the length of its exit
//
// A close animation cannot read the client's buffer: the client went away and
// its shared memory with it. So the last frame is copied into memory the
// compositor owns and pushed as a surface of its own (client 0, never a real
// connection id), and what is on the screen for the length of the curve draws
// from that.
//
// A surface of its own, not the dying one wearing a ghost: in the
// `unmake_window` case the client is still there and owns the surface object,
// so it has `wl_surface.destroy` left to send, and that must find something
// behind its id.
//
// The pool is freed when the curve ends. A frame sooner is a use-after-free, a
// frame later is a leak.

use alloc::vec::Vec;

use crate::mayland::protocol;
use crate::mayland::shell::layout;

use crate::mayland::server::{shm};
use crate::mayland::server::state::*;

/// Turns a window that is going away into its own afterimage
///
/// Copies the last frame into a pool the compositor owns and pushes it as a
/// surface of its own, belonging to client 0, which is never a real connection
/// id: nothing looking a window up by client can find it, and the teardown
/// removing every surface of that client leaves it alone.
///
/// False when there is nothing to do it with: no animation configured, no
/// buffer committed, or the memory refused. The window then simply goes
pub(in crate::mayland) fn ghost_window(s: &mut Server, si: usize) -> bool {
    let out = crate::mayland::shell::anim::spec(protocol::wm::anim_class::WINDOWS_OUT);
    let fade = crate::mayland::shell::anim::spec(protocol::wm::anim_class::FADE_OUT);
    if !crate::mayland::shell::anim::enabled() {
        return false;
    }
    if !out.enabled && !fade.enabled {
        return false;
    }
    if s.surfaces[si].role != Role::Toplevel || s.surfaces[si].ghost.is_some() {
        return false;
    }

    // Where its pixels are now, while the client is alive. Asked of the server:
    // which buffer a surface is showing and who owns it is the server's
    // business
    let Some((w, h, stride, format, pool, offset)) = s.window_pixels(si) else {
        return false;
    };
    if w <= 0 || h <= 0 {
        return false;
    }
    let bytes = stride * h as usize;
    let Ok((src, memory)) = shm::location(pool) else { return false };
    let Ok(handle) = shm::create(0, bytes) else { return false };
    let Ok(dst) = shm::phys(handle) else {
        let _ = shm::destroy(handle);
        return false;
    };
    let copied = match memory {
        // One copy through the direct map. A window's last frame is a few
        // megabytes and this happens once, when it closes
        shm::Memory::System => {
            unsafe {
                let s_ptr = crate::arch::x86_64::grub::phys_to_virt(src + offset as u64) as *const u8;
                let d_ptr = crate::arch::x86_64::grub::phys_to_virt(dst) as *mut u8;
                core::ptr::copy_nonoverlapping(s_ptr, d_ptr, bytes);
            }
            true
        }
        // A buffer in video memory comes down on the copy engine: the
        // processor reading it through BAR1 would take tens of milliseconds
        // (dev/gpu-window-buffers.md)
        shm::Memory::Video =>
            crate::mayland::render::texture::download(src + offset as u64, stride, dst, stride, w, h),
    };
    if !copied {
        let _ = shm::destroy(handle);
        return false;
    }

    // The size on `windowsOut` and the alpha on `fadeOut`, each on its own
    // clock, as the opening has them (`layout::start_open`)
    let at = (s.surfaces[si].pos.0 as f32, s.surfaces[si].pos.1 as f32);
    let mut a = crate::mayland::shell::anim::Anim {
        running: true,
        begun_us: layout::now_us(),
        ms: if out.enabled { out.ms } else { 0 },
        curve: out.curve,
        style: out.style,
        alpha_ms: if fade.enabled { fade.ms.max(1) } else { 0 },
        alpha_curve: fade.curve,
        ..crate::mayland::shell::anim::Anim::NONE
    };
    a.from = at;
    a.from_alpha = 1.0;
    a.to_alpha = if fade.enabled { 0.0 } else { 1.0 };
    a.alpha = 1.0;
    a.from_scale = 1.0;
    a.to_scale = if out.enabled && out.style == crate::mayland::shell::anim::style::POPIN {
        out.arg.clamp(0.05, 1.0)
    } else {
        1.0
    };
    a.scale = 1.0;

    let from = (s.surfaces[si].client, s.surfaces[si].id);
    s.push_ghost(si, Ghost { handle, phys: dst, width: w, height: h, stride, format, from }, a);
    true
}

/// Lets go of a ghost whose curve has ended
///
/// Called by the tick, which knows a curve is over
pub(in crate::mayland) fn reap_ghosts(s: &mut Server) -> Vec<u32> {
    let mut freed = Vec::new();
    let mut i = 0;
    while i < s.surfaces.len() {
        let done = s.surfaces[i].ghost.is_some() && !s.surfaces[i].anim.running;
        if done {
            if let Some(g) = s.surfaces[i].ghost {
                freed.push(g.handle);
            }
            s.surfaces.remove(i);
            continue;
        }
        i += 1;
    }
    freed
}
