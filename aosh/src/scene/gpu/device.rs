// aosh's card on MikuOS
//
// Holds video memory for commands, textures, and window buffers, and submits
// each frame as one launch of the kernel's `paint`. The compositor reads window
// buffers in place, and commits carry the launch's fence.
//
// Pools come from `gpu_alloc` (zeroed, write-combining, CPU write only) and are
// bound for dispatch. Programs get 128 MiB of video memory
// (`docs/MikuOS_ABI.md` §3.2a): commands 4, textures 48, and windows (16 for a
// full-screen window's two buffers).

use libmay::conn::{gpu_bind, gpu_dispatch, gpu_info, gpu_wait_code, Conn, GpuParam, Pool};
use libmay::requests::Requests;

use super::record::{Memory, Record, BLOCK_THREADS};

const COMMANDS: usize = 4 << 20;
const TEXTURES: usize = 48 << 20;
/// Windows whose two buffers fit share a pool this big; larger ones get their
/// own
const SMALL: usize = 4 << 20;
/// The target pools `paint.cu` takes
pub const POOLS: usize = 4;
/// Maximum wait for the previous frame
const WAIT_US: u64 = 250_000;

/// A pool of video memory windows' buffers are cut from
pub struct Shared {
    pool: Pool,
    /// The compositor's object for it (`may_shm.create_pool`)
    pub obj: u32,
    /// Free `(at, len)`, by `at`
    free: Vec<(usize, usize)>,
    /// Made for one big window, and given back with it
    own: bool,
}

/// The texture pool, as the recording's store writes it
struct Video {
    addr: *mut u8,
    size: usize,
}

impl Memory for Video {
    fn size(&self) -> usize {
        self.size
    }
    fn write(&mut self, at: usize, bytes: &[u8]) {
        if at + bytes.len() <= self.size {
            // SAFETY: inside the mapping, which the device keeps as long as the
            // store
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.addr.add(at), bytes.len()) };
        }
    }
}

pub struct Device {
    /// `paint`'s index among the programs `gpu_info` lists
    program: u32,
    commands: Pool,
    /// Held for the store, which writes it through `Video`
    textures: Pool,
    pub rec: Record,
    pools: [Option<Shared>; POOLS],
    /// Last frame's fence until it passes
    in_flight: Option<u64>,
}

/// `paint`'s index, from `gpu_info`'s `program N name` lines
fn program_index(info: &str) -> Option<u32> {
    info.lines().find_map(|l| {
        let mut w = l.split_whitespace();
        (w.next() == Some("program")).then_some(())?;
        let i = w.next()?.parse().ok()?;
        (w.next() == Some("paint")).then_some(i)
    })
}

/// A pool of video memory, bound
fn video(size: usize) -> Result<Pool, String> {
    let mut p = Pool::video(size).map_err(|e| format!("gpu_alloc of {} KiB: {}", size / 1024, libmay::conn::gpu_errno(e)))?;
    if gpu_bind(&p).is_none() {
        p.free();
        return Err("the pool could not be bound".into());
    }
    Ok(p)
}

impl Device {
    /// Opens the card if `paint` and video memory for commands and textures are
    /// available. Returns the reason otherwise
    pub fn open() -> Result<Device, String> {
        let mut buf = [0u8; 4096];
        let n = gpu_info(&mut buf);
        let info = std::str::from_utf8(&buf[..n]).unwrap_or("");
        if n == 0 || info.lines().any(|l| l == "backend none") {
            return Err("there is no card to draw on".into());
        }
        let program = program_index(info).ok_or("the kernel has no 'paint' program (built without CUDA, or its compute is down)")?;
        let commands = video(COMMANDS)?;
        let mut textures = match video(TEXTURES) {
            Ok(t) => t,
            Err(e) => {
                let mut c = commands;
                c.free();
                return Err(e);
            }
        };
        let mem = Video { addr: textures.bytes().as_mut_ptr(), size: textures.size };
        Ok(Device { program, commands, textures, rec: Record::new(Box::new(mem)), pools: Default::default(), in_flight: None })
    }

    /// Space for two window buffers of `bytes` each: pool and offset. Creates a
    /// pool (`create_pool`) if none has room; `None` if out of video memory
    pub fn place(&mut self, c: &mut Conn, shm: u32, bytes: usize) -> Option<(usize, usize)> {
        let want = bytes * 2;
        let own = want > SMALL;
        if !own {
            for (i, p) in self.pools.iter_mut().enumerate() {
                let Some(p) = p.as_mut().filter(|p| !p.own) else { continue };
                if let Some(k) = p.free.iter().position(|&(_, l)| l >= want) {
                    let (at, l) = p.free[k];
                    if l == want {
                        p.free.remove(k);
                    } else {
                        p.free[k] = (at + want, l - want);
                    }
                    return Some((i, at));
                }
            }
        }
        let slot = self.pools.iter().position(|p| p.is_none())?;
        let size = if own { want } else { SMALL };
        let pool = video(size).map_err(|e| crate::say(&format!("no video memory for a window: {e}"))).ok()?;
        let obj = c.create_pool(shm, pool.handle, size as i32);
        let free = if size > want { vec![(want, size - want)] } else { Vec::new() };
        self.pools[slot] = Some(Shared { pool, obj, free, own });
        Some((slot, 0))
    }

    /// Releases a window's buffers. A single-window pool goes too, after the
    /// compositor has seen its object destroyed
    pub fn release(&mut self, c: &mut Conn, pool: usize, at: usize, bytes: usize) {
        let Some(p) = self.pools.get_mut(pool).and_then(|p| p.as_mut()) else { return };
        if p.own {
            // Waits for the GPU first
            self.wait();
            if let Some(mut p) = self.pools[pool].take() {
                c.destroy_pool(p.obj);
                c.flush();
                p.pool.free();
            }
            return;
        }
        let len = bytes * 2;
        let i = p.free.partition_point(|&(a, _)| a < at);
        p.free.insert(i, (at, len));
        if i + 1 < p.free.len() && p.free[i].0 + p.free[i].1 == p.free[i + 1].0 {
            p.free[i].1 += p.free[i + 1].1;
            p.free.remove(i + 1);
        }
        if i > 0 && p.free[i - 1].0 + p.free[i - 1].1 == p.free[i].0 {
            p.free[i - 1].1 += p.free[i].1;
            p.free.remove(i);
        }
    }

    /// The compositor's pool object a window's buffers are made in
    pub fn pool_obj(&self, pool: usize) -> u32 {
        self.pools.get(pool).and_then(|p| p.as_ref()).map_or(0, |p| p.obj)
    }

    /// A pool's memory, for pixels the processor painted
    pub fn pool_bytes(&mut self, pool: usize) -> Option<&mut [u8]> {
        self.pools.get_mut(pool).and_then(|p| p.as_mut()).map(|p| p.pool.bytes())
    }

    /// Waits for the last run, after which its inputs may be rewritten. False
    /// on timeout or a dead channel
    pub fn wait(&mut self) -> bool {
        let Some(f) = self.in_flight.take() else { return true };
        let r = gpu_wait_code(f, WAIT_US);
        if r != 0 {
            crate::say(&format!("the last frame's drawing did not finish: {}", libmay::conn::gpu_errno(r)));
        }
        r == 0
    }

    /// Submits the frame's commands as one launch. Returns its fence, `None` if
    /// empty, or the error
    pub fn submit(&mut self) -> Result<Option<u64>, String> {
        let tiles = self.rec.tiles();
        if tiles == 0 {
            return Ok(None);
        }
        let words = self.rec.words();
        let bytes = words.len() * 4;
        if bytes > self.commands.size {
            let size = bytes.next_power_of_two();
            let grown = video(size)?;
            let mut old = std::mem::replace(&mut self.commands, grown);
            old.free();
        }
        // SAFETY: u32 to its four bytes, in the GPU's byte order
        let raw = unsafe { std::slice::from_raw_parts(words.as_ptr() as *const u8, bytes) };
        self.commands.bytes()[..bytes].copy_from_slice(raw);
        let first = self.pools.iter().flatten().next().map(|p| p.pool.handle).unwrap_or(self.commands.handle);
        let mut params = vec![GpuParam::sized(self.commands.handle, 0), GpuParam::sized(self.textures.handle, 0)];
        for p in &self.pools {
            params.push(GpuParam::sized(p.as_ref().map_or(first, |p| p.pool.handle), 0));
        }
        let fence = gpu_dispatch(self.program, (tiles, 1, 1), (BLOCK_THREADS, 1, 1), &params);
        if fence < 0 {
            return Err(format!("the frame's launch was refused: {}", libmay::conn::gpu_errno(fence)));
        }
        self.in_flight = Some(fence as u64);
        Ok(Some(fence as u64))
    }

    /// Releases everything, for the processor to draw from now on
    pub fn close(mut self, c: &mut Conn) {
        self.wait();
        for p in self.pools.iter_mut() {
            if let Some(mut p) = p.take() {
                c.destroy_pool(p.obj);
                c.flush();
                p.pool.free();
            }
        }
        self.commands.free();
        self.textures.free();
    }
}
