// may_compositor, may_surface, may_region: pixels arriving
//
// `may_compositor` is the factory for surfaces and regions, and `may_surface`
// is the double-buffered state machine underneath every role: attach, damage,
// frame, commit.
//
// A surface here has no role yet and is not on the screen. What it becomes is
// decided by `xdg` or `layer`

use alloc::vec::Vec;

use crate::mayland::protocol::{self, Interface};
use crate::mayland::wire::{Reader};
use crate::mayland::shell::layout;

use crate::mayland::server::{dispatch, surface};
use crate::mayland::server::state::*;

pub(in crate::mayland) fn on_compositor(s: &mut Server, idx: usize, _sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    match opcode {
        protocol::compositor::CREATE_SURFACE => {
            let id = r.new_id().map_err(|_| "create_surface: truncated")?;
            dispatch::bind_new(s, idx, id, Interface::Surface, 1)?;
            let client = s.clients[idx].id;
            let z = s.next_z;
            s.next_z += 1;
            s.surfaces.push(Surface {
                client,
                id,
                z,
                backdrop: false,
                parent: None,
                rel: (0, 0),
                popup_id: 0,
                layer: None,
                no_focus: false,
                // A window opens on the workspace being looked at
                workspace: *CURRENT_WS.lock(),
                sticky: false,
                last_w: 0,
                last_h: 0,
                pending: SurfaceState { buffer: None, scale: 1, fence: 0 },
                current: SurfaceState { buffer: None, scale: 1, fence: 0 },
                input: None,
                pending_input: None,
                pending_damage: Vec::new(),
                damage: Vec::new(),
                damage_read: None,
                pending_frames: Vec::new(),
                frames: Vec::new(),
                retired: Vec::new(),
                pos: (0, 0),
                goal: layout::Rect::ZERO,
                home: layout::Rect::ZERO,
                told: None,
                goal_sized: false,
                placed: false,
                floating: false,
                fullscreen: false,
                pinned: false,
                fx: crate::mayland::render::Fx::default(),
                anim: crate::mayland::shell::anim::Anim::NONE,
                fresh: true,
                ghost: None,
                unconsumed: false,
                commits: 0,
                role: Role::None,
                xdg_id: 0,
                toplevel_id: 0,
                title: alloc::string::String::new(),
                app_id: alloc::string::String::new(),
                configure_sent: 0,
                configure_acked: 0,
            });
            Ok(())
        }
        protocol::compositor::CREATE_REGION => {
            let id = r.new_id().map_err(|_| "create_region: truncated")?;
            if s.clients[idx].regions.len() >= MAX_REGIONS {
                return Err("create_region: too many regions");
            }
            dispatch::bind_new(s, idx, id, Interface::Region, 1)?;
            s.clients[idx].regions.push((id, Vec::new()));
            Ok(())
        }
        _ => Err("may_compositor: unknown request"),
    }
}

pub(in crate::mayland) fn on_surface(s: &mut Server, idx: usize, sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    let cid = s.clients[idx].id;
    let si = s.surfaces.iter().position(|x| x.client == cid && x.id == sender)
        .ok_or("may_surface: object has no surface behind it")?;

    match opcode {
        protocol::surface::ATTACH => {
            let buf = r.object().map_err(|_| "attach: truncated")?;
            // Where the new buffer's top left sits relative to the old one, in
            // surface coordinates. Not a position on the screen: a window that
            // could place itself would overrule every layout on its next
            // commit. Read and dropped: it is for a surface that shrinks from
            // one edge, and no role here has one yet
            let _dx = r.int().map_err(|_| "attach: truncated")?;
            let _dy = r.int().map_err(|_| "attach: truncated")?;
            if buf != 0 && s.clients[idx].objects.iface_of(buf) != Some(Interface::Buffer) {
                return Err("attach: argument is not a may_buffer");
            }
            s.surfaces[si].pending.buffer = if buf == 0 { None } else { Some(buf) };
            Ok(())
        }
        // The same request here: they differ by the buffer scale, and
        // `set_buffer_scale` refuses anything but 1, so surface and buffer
        // coordinates are the same numbers
        protocol::surface::DAMAGE | protocol::surface::DAMAGE_BUFFER => {
            let x = r.int().map_err(|_| "damage: truncated")?;
            let y = r.int().map_err(|_| "damage: truncated")?;
            let w = r.int().map_err(|_| "damage: truncated")?;
            let h = r.int().map_err(|_| "damage: truncated")?;
            if w > 0 && h > 0 {
                s.surfaces[si].pending_damage.push((x, y, w, h));
            }
            Ok(())
        }
        protocol::surface::FRAME => {
            let id = r.new_id().map_err(|_| "frame: truncated")?;
            dispatch::bind_new(s, idx, id, Interface::Callback, 1)?;
            s.surfaces[si].pending_frames.push(id);
            Ok(())
        }
        protocol::surface::SET_FENCE => {
            // Two words, because the wire carries u32s and a fence is 64 bits
            let lo = r.uint().map_err(|_| "set_fence: truncated")? as u64;
            let hi = r.uint().map_err(|_| "set_fence: truncated")? as u64;
            s.surfaces[si].pending.fence = (hi << 32) | lo;
            Ok(())
        }
        protocol::surface::COMMIT => {
            // The rule xdg-shell enforces: a window may not show pixels until
            // it has agreed a size. Otherwise the first frame is drawn for a
            // size nobody proposed, and every later resize is a guess about
            // which buffer answered which configure
            if s.surfaces[si].role == Role::Toplevel
                && s.surfaces[si].pending.buffer.is_some()
                && s.surfaces[si].configure_acked == 0
            {
                return Err("commit: a buffer before any configure was acknowledged");
            }
            surface::commit(s, si);
            Ok(())
        }
        protocol::surface::SET_BUFFER_SCALE => {
            let scale = r.int().map_err(|_| "set_buffer_scale: truncated")?;
            if scale <= 0 {
                return Err("set_buffer_scale: scale must be positive");
            }
            // Refused: nothing reads this, since the copy engine takes the
            // buffer at its own size, and a client told its scale was accepted
            // would draw at twice the resolution into a quarter of the area.
            // `set_buffer_transform` answers the same
            if scale != 1 {
                return Err("set_buffer_scale: only scale 1 is presented here");
            }
            s.surfaces[si].pending.scale = scale;
            Ok(())
        }
        protocol::surface::SET_BUFFER_TRANSFORM => {
            let t = r.int().map_err(|_| "set_buffer_transform: truncated")?;
            if t != protocol::surface::TRANSFORM_NORMAL {
                return Err("set_buffer_transform: only the normal transform exists here");
            }
            Ok(())
        }
        protocol::surface::OFFSET => {
            // The same two numbers as attach's, split into their own request.
            // Dropped for the same reason
            let _ = r.int().map_err(|_| "offset: truncated")?;
            let _ = r.int().map_err(|_| "offset: truncated")?;
            Ok(())
        }
        protocol::surface::SET_OPAQUE_REGION => {
            // Accepted and ignored: a hint. A compositor ignoring it draws
            // correctly and only draws more
            let _ = r.object().map_err(|_| "set_region: truncated")?;
            Ok(())
        }
        protocol::surface::SET_INPUT_REGION => {
            // The region as it is now, for the next commit; none is the whole
            // surface. The client may change or destroy the region after this
            // and it changes nothing here, as in Wayland
            let region = r.object().map_err(|_| "set_input_region: truncated")?;
            let steps = if region == 0 {
                None
            } else {
                match s.clients[idx].regions.iter().find(|(id, _)| *id == region) {
                    Some((_, steps)) => Some(steps.clone()),
                    None => return Err("set_input_region: no such region"),
                }
            };
            s.surfaces[si].pending_input = Some(steps);
            Ok(())
        }
        protocol::surface::DESTROY => {
            // Off any focus grab's list: a grab of nothing lets go
            crate::mayland::input::grab::focus_forget_surface((s.clients[idx].id, sender));
            // Callbacks the surface still owes are dropped, not fired: the
            // client asked for the surface to be gone
            let (frames, pending): (Vec<u32>, Vec<u32>) = (
                core::mem::take(&mut s.surfaces[si].frames),
                core::mem::take(&mut s.surfaces[si].pending_frames),
            );
            for id in frames.into_iter().chain(pending) {
                let _ = s.clients[idx].objects.remove(id);
                s.clients[idx].delete_id(id);
            }
            surface::release_current(s, si);
            // Anything a later commit took the place of goes back too: the
            // surface is gone and the buffers are the client's
            for bid in core::mem::take(&mut s.surfaces[si].retired) {
                surface::release_buffer(s, cid, bid);
            }
            surface::unmake_window(s, si);
            s.surfaces.retain(|x| !(x.client == cid && x.id == sender));
            dispatch::destroy_object(s, idx, sender);
            Ok(())
        }
        _ => Err("may_surface: unknown request"),
    }
}

pub(in crate::mayland) fn on_region(s: &mut Server, idx: usize, sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    match opcode {
        protocol::region::ADD | protocol::region::SUBTRACT => {
            let mut n = [0i32; 4];
            for v in &mut n {
                *v = r.int().map_err(|_| "region: truncated")?;
            }
            let Some((_, steps)) = s.clients[idx].regions.iter_mut().find(|(id, _)| *id == sender)
            else { return Err("region: not a region") };
            if steps.len() >= MAX_REGION_STEPS {
                return Err("region: too many rectangles");
            }
            // A rectangle of no size holds no point, and is no step
            if n[2] > 0 && n[3] > 0 {
                steps.push(RegionStep { add: opcode == protocol::region::ADD, x: n[0], y: n[1], w: n[2], h: n[3] });
            }
            Ok(())
        }
        protocol::region::DESTROY => {
            s.clients[idx].regions.retain(|(id, _)| *id != sender);
            dispatch::destroy_object(s, idx, sender);
            Ok(())
        }
        _ => Err("may_region: unknown request"),
    }
}
