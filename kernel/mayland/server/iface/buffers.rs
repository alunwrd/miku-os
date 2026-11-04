// may_shm, may_shm_pool, may_buffer: where the pixels live
//
// Wayland passes a file descriptor here because its compositor is another
// process. Ours is the kernel, so a pool is named by a handle and the checks a
// descriptor would carry are explicit: the handle must belong to the process on
// the other end of this connection, and every buffer cut from it must lie
// inside it.

use crate::mayland::protocol::{self, Interface};
use crate::mayland::wire::{Reader};

use crate::mayland::server::{dispatch, shm};
use crate::mayland::server::state::*;

pub(in crate::mayland) fn on_shm(s: &mut Server, idx: usize, _sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    if opcode != protocol::shm::CREATE_POOL {
        return Err("may_shm: unknown request");
    }
    let id = r.new_id().map_err(|_| "create_pool: truncated")?;
    let handle = r.uint().map_err(|_| "create_pool: truncated")?;
    let size = r.int().map_err(|_| "create_pool: truncated")?;
    if size <= 0 {
        return Err("create_pool: size must be positive");
    }

    // The handle has to belong to the process on the other end of this
    // connection. Without this check a client could name any pool in the
    // system and read another client's pixels
    let pool_size = match shm::claim(handle, s.clients[idx].pid) {
        Ok(n) => n,
        Err(e) => {
            crate::serial_println!(
                "[mayland] create_pool refused: handle {} size {} client {} pid {} - {:?}",
                handle, size, s.clients[idx].id, s.clients[idx].pid, e);
            shm::dump();
            return Err("create_pool: no such shared-memory handle for this client");
        }
    };
    if (size as usize) > pool_size {
        return Err("create_pool: size larger than the shared-memory object");
    }
    // A pool of video memory (`gpu_alloc`) is taken like any: the frame reads
    // it where it lies (`scene::pixels_of`, dev/gpu-window-buffers.md)

    dispatch::bind_new(s, idx, id, Interface::ShmPool, 1)?;
    let client = s.clients[idx].id;
    s.pools.push(PoolObj { client, id, handle, size: size as usize });
    Ok(())
}

pub(in crate::mayland) fn on_shm_pool(s: &mut Server, idx: usize, sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    let cid = s.clients[idx].id;
    let pi = s.pools.iter().position(|p| p.client == cid && p.id == sender)
        .ok_or("may_shm_pool: object has no pool behind it")?;

    match opcode {
        protocol::shm_pool::CREATE_BUFFER => {
            let id     = r.new_id().map_err(|_| "create_buffer: truncated")?;
            let offset = r.int().map_err(|_| "create_buffer: truncated")?;
            let width  = r.int().map_err(|_| "create_buffer: truncated")?;
            let height = r.int().map_err(|_| "create_buffer: truncated")?;
            let stride = r.int().map_err(|_| "create_buffer: truncated")?;
            let format = r.uint().map_err(|_| "create_buffer: truncated")?;

            if offset < 0 || width <= 0 || height <= 0 || stride <= 0 {
                return Err("create_buffer: negative or zero geometry");
            }
            if !FORMATS.contains(&format) {
                return Err("create_buffer: unadvertised pixel format");
            }
            if stride < width.saturating_mul(4) {
                return Err("create_buffer: stride is shorter than a row of pixels");
            }
            // The last row must lie inside the pool. Checked in usize with
            // saturating arithmetic so a huge height cannot wrap past it
            let need = (stride as usize)
                .checked_mul(height as usize)
                .and_then(|b| b.checked_add(offset as usize))
                .ok_or("create_buffer: geometry overflows")?;
            if need > s.pools[pi].size {
                return Err("create_buffer: buffer runs past the end of the pool");
            }
            // A buffer in video memory is read in place by the effects pass,
            // which maps it into a window slot from the start of its page, and
            // a slot is as large as the largest copy the compositor makes of a
            // system-memory one. One past it would fail the pass for every
            // window on the screen, every frame
            let in_video = matches!(shm::location(s.pools[pi].handle), Ok((_, shm::Memory::Video)));
            let span = (offset as u64 & 0xfff) + (stride as u64) * (height as u64);
            if in_video && span > crate::mayland::render::texture::MAX_BYTES {
                return Err("create_buffer: a buffer in video memory is at most 16 MiB from its page");
            }

            dispatch::bind_new(s, idx, id, Interface::Buffer, 1)?;
            let client = s.clients[idx].id;
            let handle = s.pools[pi].handle;
            s.buffers.push(BufferObj {
                client, id, pool: handle,
                offset: offset as usize,
                width, height,
                stride: stride as usize,
                format,
                busy: false,
            });
            Ok(())
        }
        protocol::shm_pool::RESIZE => {
            let size = r.int().map_err(|_| "resize: truncated")?;
            if size <= 0 {
                return Err("resize: size must be positive");
            }
            let handle = s.pools[pi].handle;
            let backing = shm::size(handle).map_err(|_| "resize: pool is gone")?;
            // Wayland's pool resize only grows, and only within the shared
            // object the client handed over. Growing past it would mean
            // remapping, which is the client's job
            if (size as usize) < s.pools[pi].size {
                return Err("resize: a pool may only grow");
            }
            if (size as usize) > backing {
                return Err("resize: past the end of the shared-memory object");
            }
            s.pools[pi].size = size as usize;
            Ok(())
        }
        protocol::shm_pool::DESTROY => {
            // Buffers already created out of the pool keep working: the backing
            // memory belongs to the shared-memory handle, not to this object,
            // as in wl_shm_pool.destroy. The handle is remembered, or nothing
            // frees it when the connection goes
            let p = s.pools.remove(pi);
            s.clients[idx].orphan_pools.push(p.handle);
            dispatch::destroy_object(s, idx, sender);
            Ok(())
        }
        _ => Err("may_shm_pool: unknown request"),
    }
}

pub(in crate::mayland) fn on_buffer(s: &mut Server, idx: usize, sender: u32, opcode: u16, _r: &mut Reader)
    -> Result<(), &'static str>
{
    if opcode != protocol::buffer::DESTROY {
        return Err("may_buffer: unknown request");
    }
    // A client may destroy a buffer that is still attached; the surface then
    // has no content until the next attach. Detached here so no dangling id is
    // left
    let cid = s.clients[idx].id;
    for surf in s.surfaces.iter_mut() {
        if surf.client != cid { continue; }
        if surf.pending.buffer == Some(sender) { surf.pending.buffer = None; }
        if surf.current.buffer == Some(sender) { surf.current.buffer = None; }
        // And out of the list waiting to be released. The id is about to be
        // free for reuse, and a release owed to the buffer that had it would
        // land on the next one, handing back a buffer the compositor is reading
        surf.retired.retain(|b| *b != sender);
    }
    s.buffers.retain(|b| !(b.client == cid && b.id == sender));
    dispatch::destroy_object(s, idx, sender);
    Ok(())
}
