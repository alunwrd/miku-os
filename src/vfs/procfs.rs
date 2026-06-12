use crate::vfs::types::*;
use core::sync::atomic::{AtomicU64, Ordering};

pub struct ProcFs;

impl ProcFs {
    pub const fn new() -> Self {
        Self
    }
}

static TICK_COUNT: AtomicU64 = AtomicU64::new(0);
static WALL_CLOCK: AtomicU64 = AtomicU64::new(0);

pub fn tick() {
    TICK_COUNT.fetch_add(1, Ordering::Relaxed);
}

pub fn uptime_ticks() -> u64 {
    TICK_COUNT.load(Ordering::Relaxed)
}

pub fn set_wall_clock(unix_secs: u64) {
    WALL_CLOCK.store(unix_secs, Ordering::Relaxed);
}

pub fn wall_clock() -> u64 {
    WALL_CLOCK.load(Ordering::Relaxed)
}

pub fn proc_read(name: &str, buf: &mut [u8], vnode_used: usize) -> VfsResult<usize> {
    // diskstats needs more room than the shared 192-byte scratch
    if name == "diskstats" {
        let mut big = [0u8; 1024];
        let len = format_diskstats(&mut big);
        let to_copy = len.min(buf.len());
        buf[..to_copy].copy_from_slice(&big[..to_copy]);
        return Ok(to_copy);
    }

    let mut tmp = [0u8; 192];
    let len = match name {
        "version" => {
            let s = b"MikuOS v0.2.7-rc (x86_64)\nbuilt with love <3\n";
            let l = s.len().min(192);
            tmp[..l].copy_from_slice(&s[..l]);
            l
        }
        "uptime" => {
            let ticks = uptime_ticks();
            let secs = ticks / 18;
            let mins = secs / 60;
            let hours = mins / 60;
            format_uptime(&mut tmp, hours, mins % 60, secs % 60, ticks)
        }
        "meminfo" => format_meminfo(&mut tmp, vnode_used, MAX_VNODES, MAX_DATA_PAGES),
        "mounts" => format_mounts(&mut tmp),
        "cpuinfo" => {
            let s = b"arch: x86_64\nvendor: unknown\nfeatures: vfs tmpfs devfs procfs ext2 ext3 ext4\n";
            let l = s.len().min(192);
            tmp[..l].copy_from_slice(&s[..l]);
            l
        }
        "stat" => format_stat(&mut tmp),
        "heap" => format_heap(&mut tmp),
        _ => return Err(VfsError::NotFound),
    };

    let to_copy = len.min(buf.len());
    buf[..to_copy].copy_from_slice(&tmp[..to_copy]);
    Ok(to_copy)
}

fn format_uptime(buf: &mut [u8; 192], hours: u64, mins: u64, secs: u64, ticks: u64) -> usize {
    let mut pos = 0;
    pos += write_str(buf, pos, "up ");
    pos += write_u64(buf, pos, hours);
    pos += write_str(buf, pos, "h ");
    pos += write_u64(buf, pos, mins);
    pos += write_str(buf, pos, "m ");
    pos += write_u64(buf, pos, secs);
    pos += write_str(buf, pos, "s (");
    pos += write_u64(buf, pos, ticks);
    pos += write_str(buf, pos, " ticks)\n");
    pos
}

fn format_meminfo(
    buf: &mut [u8; 192],
    vnode_used: usize,
    vnode_max: usize,
    pages_total: usize,
) -> usize {
    let heap_used = crate::allocator::used();
    let heap_free = crate::allocator::free();
    let heap_total = crate::allocator::HEAP_SIZE;

    let mut pos = 0;
    pos += write_str(buf, pos, "vnodes: ");
    pos += write_u64(buf, pos, vnode_used as u64);
    pos += write_str(buf, pos, "/");
    pos += write_u64(buf, pos, vnode_max as u64);
    pos += write_str(buf, pos, "\npages:  ");
    pos += write_u64(buf, pos, pages_total as u64);
    pos += write_str(buf, pos, "\nheap:   ");
    pos += write_u64(buf, pos, heap_used as u64);
    pos += write_str(buf, pos, "/");
    pos += write_u64(buf, pos, heap_total as u64);
    pos += write_str(buf, pos, " (");
    pos += write_u64(buf, pos, heap_free as u64);
    pos += write_str(buf, pos, " free)\n");
    pos
}

fn format_mounts(buf: &mut [u8; 192]) -> usize {
    let mut pos = 0;
    pos += write_str(buf, pos, "tmpfs on / type tmpfs (rw)\n");
    pos += write_str(buf, pos, "devfs on /dev type devfs (rw)\n");
    pos += write_str(buf, pos, "procfs on /proc type procfs (ro)\n");
    pos
}

fn format_stat(buf: &mut [u8; 192]) -> usize {
    let mut pos = 0;
    let ticks = uptime_ticks();
    pos += write_str(buf, pos, "ticks: ");
    pos += write_u64(buf, pos, ticks);
    pos += write_str(buf, pos, "\nmax_vnodes: ");
    pos += write_u64(buf, pos, MAX_VNODES as u64);
    pos += write_str(buf, pos, "\nmax_pages: ");
    pos += write_u64(buf, pos, MAX_DATA_PAGES as u64);
    pos += write_str(buf, pos, "\nmax_fds: ");
    pos += write_u64(buf, pos, MAX_OPEN_FILES as u64);
    pos += write_str(buf, pos, "\nheap_kb: ");
    pos += write_u64(buf, pos, (crate::allocator::HEAP_SIZE / 1024) as u64);
    pos += write_str(buf, pos, "\n");
    pos
}

fn format_heap(buf: &mut [u8; 192]) -> usize {
    let used = crate::allocator::used();
    let free = crate::allocator::free();
    let total = crate::allocator::HEAP_SIZE;

    let mut pos = 0;
    pos += write_str(buf, pos, "total: ");
    pos += write_u64(buf, pos, total as u64);
    pos += write_str(buf, pos, "\nused:  ");
    pos += write_u64(buf, pos, used as u64);
    pos += write_str(buf, pos, "\nfree:  ");
    pos += write_u64(buf, pos, free as u64);
    pos += write_str(buf, pos, "\n");
    pos
}

fn write_str(buf: &mut [u8; 192], pos: usize, s: &str) -> usize {
    let b = s.as_bytes();
    let l = b.len().min(192usize.saturating_sub(pos));
    buf[pos..pos + l].copy_from_slice(&b[..l]);
    l
}

fn write_u64(buf: &mut [u8; 192], pos: usize, val: u64) -> usize {
    if val == 0 {
        if pos < 192 {
            buf[pos] = b'0';
            return 1;
        }
        return 0;
    }
    let mut tmp = [0u8; 20];
    let mut v = val;
    let mut i = 0;
    while v > 0 {
        tmp[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    let l = i.min(192usize.saturating_sub(pos));
    for j in 0..l {
        buf[pos + j] = tmp[i - 1 - j];
    }
    l
}

pub const PROC_ENTRIES: &[&str] = &[
    "version", "uptime", "meminfo", "mounts", "cpuinfo", "stat", "heap", "diskstats",
];

/// Linux /proc/diskstats analogue, one line per active block device:
/// name kind ios sectors_read sectors_written sectors_discarded errors retries avg_latency_us
fn format_diskstats(buf: &mut [u8; 1024]) -> usize {
    use core::fmt::Write;

    struct W<'a> {
        buf: &'a mut [u8; 1024],
        pos: usize,
    }
    impl core::fmt::Write for W<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let bytes = s.as_bytes();
            let n = bytes.len().min(1024 - self.pos);
            self.buf[self.pos..self.pos + n].copy_from_slice(&bytes[..n]);
            self.pos += n;
            Ok(())
        }
    }

    let mut w = W { buf, pos: 0 };
    for id in 0..crate::vfs::types::MAX_BLOCK_DEVICES as u8 {
        let Some(info) = crate::block::info(id) else { continue };
        if info.total_sectors == 0 && info.model_len == 0 {
            continue;
        }
        let Some(st) = crate::block::dev_stats(id) else { continue };
        let kind = match st.kind {
            crate::block::DevKind::Ata       => "ata",
            crate::block::DevKind::VirtioBlk => "virtio",
            crate::block::DevKind::Ahci      => "ahci",
            crate::block::DevKind::Nvme      => "nvme",
        };
        let _ = writeln!(
            w,
            "blk{} {} {} {} {} {} {} {} {}",
            id, kind, st.ios, st.sectors_read, st.sectors_written,
            st.sectors_discarded, st.io_errors, st.retries, st.avg_io_us
        );
    }
    w.pos
}
