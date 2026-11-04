use crate::fs::vfs::types::*;
use core::sync::atomic::{AtomicU64, Ordering};

pub struct ProcFs;

impl ProcFs {
    pub const fn new() -> Self {
        Self
    }
}

static WALL_CLOCK: AtomicU64 = AtomicU64::new(0);

/// Timer ticks since the machine started: the timer interrupt's own counter
pub fn uptime_ticks() -> u64 {
    crate::arch::x86_64::interrupts::get_tick()
}

/// Ticks a second, as the timer is programmed. `tick()` is called from the
/// timer interrupt, so this is the divisor that turns its count into seconds.
/// Before calibration finishes the answer is 250, the rate the timer is set to
pub fn tick_hz() -> u64 {
    match crate::arch::x86_64::apic::timer_hz() as u64 {
        0 => 250,
        hz => hz,
    }
}

/// Forwards to kcore::clock, which anchors the epoch to a tick
pub fn set_wall_clock(unix_secs: u64) {
    crate::kcore::clock::set_epoch(unix_secs, "ntp");
    WALL_CLOCK.store(unix_secs, Ordering::Relaxed);
}

pub fn wall_clock() -> u64 {
    crate::kcore::clock::now_unix()
}

/// What the VFS says about its caches, taken under its lock by the caller
pub struct VfsNumbers {
    pub vnodes: usize,
    pub fs_lookups: u64,
    pub negative_hits: u64,
    pub complete_misses: u64,
    pub icache: usize,
}

pub fn proc_read(name: &str, buf: &mut [u8], vfs: &VfsNumbers) -> VfsResult<usize> {
    // Two entries format into buffers of their own
    if name == "net" {
        let mut big = [0u8; 512];
        let len = format_net(&mut big);
        let to_copy = len.min(buf.len());
        buf[..to_copy].copy_from_slice(&big[..to_copy]);
        return Ok(to_copy);
    }
    if name == "slabinfo" {
        let mut big = [0u8; 8192];
        let len = format_slabinfo(&mut big);
        let to_copy = len.min(buf.len());
        buf[..to_copy].copy_from_slice(&big[..to_copy]);
        return Ok(to_copy);
    }
    if name == "lockstat" {
        let mut big = [0u8; 8192];
        let len = format_lockstat(&mut big);
        let to_copy = len.min(buf.len());
        buf[..to_copy].copy_from_slice(&big[..to_copy]);
        return Ok(to_copy);
    }
    if name == "diskstats" {
        let mut big = [0u8; 1024];
        let len = format_diskstats(&mut big);
        let to_copy = len.min(buf.len());
        buf[..to_copy].copy_from_slice(&big[..to_copy]);
        return Ok(to_copy);
    }

    let mut tmp = [0u8; PROC_BUF];
    let len = match name {
        "version" => {
            let s = concat!("MikuOS v", env!("CARGO_PKG_VERSION"), " (x86_64)\nbuilt with love <3\n").as_bytes();
            let l = s.len().min(PROC_BUF);
            tmp[..l].copy_from_slice(&s[..l]);
            l
        }
        "uptime" => format_uptime(&mut tmp),
        "meminfo" => format_meminfo(&mut tmp, vfs.vnodes, crate::fs::vfs::pages::tmpfs_limit()),
        "mounts" => format_mounts(&mut tmp),
        "cpuinfo" => {
            let s = b"arch: x86_64\nvendor: unknown\nfeatures: vfs tmpfs devfs procfs ext2 ext3 ext4\n";
            let l = s.len().min(PROC_BUF);
            tmp[..l].copy_from_slice(&s[..l]);
            l
        }
        "stat" => format_stat(&mut tmp, vfs),
        "heap" => format_heap(&mut tmp),
        "clock" => format_clock(&mut tmp),
        _ => return Err(VfsError::NotFound),
    };

    let to_copy = len.min(buf.len());
    buf[..to_copy].copy_from_slice(&tmp[..to_copy]);
    Ok(to_copy)
}

/// Enough for the constant lines, a mount table's worth, and a `cpuN` line in
/// `stat` for each of sixty-four processors
const PROC_BUF: usize = 4096;

/// `/proc/uptime`, in Linux's format: seconds up and seconds idle, summed over
/// the processors, to the hundredth. Quickshell configs read the first number
/// with `parseFloat`
fn format_uptime(buf: &mut [u8; PROC_BUF]) -> usize {
    let hz = tick_hz();
    let idle: u64 = (0..crate::arch::x86_64::percpu::cpu_count())
        .map(|c| crate::sched::cpu_ticks(c).2)
        .sum();
    let mut pos = 0;
    pos += write_seconds(buf, pos, uptime_ticks(), hz);
    pos += write_str(buf, pos, " ");
    pos += write_seconds(buf, pos, idle, hz);
    pos += write_str(buf, pos, "\n");
    pos
}

/// `ticks` as seconds with two decimals: `1234.56`
fn write_seconds(buf: &mut [u8; PROC_BUF], pos: usize, ticks: u64, hz: u64) -> usize {
    let hz = hz.max(1);
    let mut n = write_u64(buf, pos, ticks / hz);
    n += write_str(buf, pos + n, ".");
    let cent = (ticks % hz) * 100 / hz;
    if cent < 10 {
        n += write_str(buf, pos + n, "0");
    }
    n += write_u64(buf, pos + n, cent);
    n
}

/// `Key:` padded to sixteen columns and a number right-aligned in eight more,
/// the layout of Linux's `/proc/meminfo`. Read by people with `cat` and by
/// programs with `Key: *(\d+)`
fn write_kb_line(buf: &mut [u8; PROC_BUF], pos: usize, key: &str, kb: u64) -> usize {
    let mut n = write_str(buf, pos, key);
    n += write_str(buf, pos + n, ":");
    let width = {
        let (mut w, mut v) = (1, kb / 10);
        while v > 0 { w += 1; v /= 10; }
        w
    };
    let pad = (16usize.saturating_sub(key.len() + 1) + 8).saturating_sub(width).max(1);
    for _ in 0..pad {
        n += write_str(buf, pos + n, " ");
    }
    n += write_u64(buf, pos + n, kb);
    n += write_str(buf, pos + n, " kB\n");
    n
}

fn format_meminfo(
    buf: &mut [u8; PROC_BUF],
    vnode_used: usize,
    pages_total: usize,
) -> usize {
    let heap_used = crate::mm::heap::used();
    let heap_free = crate::mm::heap::free();
    let heap_total = crate::mm::heap::total();

    // The machine's memory, in frames of 4 KiB. The heap below is the kernel's
    // own allocator
    let (frames_used, frames_total) = crate::mm::pmm::stats();

    // Linux's names for the first five: `free`, `top` and quickshell memory
    // meters read them (`MemTotal: *(\d+)`). Available equals free. The swap
    // lines are zero but present: a meter that divides by `SwapTotal` checks
    // for zero, not for a missing line
    let total = (frames_total as u64) * 4;
    let free = total.saturating_sub((frames_used as u64) * 4);
    let mut pos = 0;
    pos += write_kb_line(buf, pos, "MemTotal", total);
    pos += write_kb_line(buf, pos, "MemFree", free);
    pos += write_kb_line(buf, pos, "MemAvailable", free);
    pos += write_kb_line(buf, pos, "SwapTotal", 0);
    pos += write_kb_line(buf, pos, "SwapFree", 0);
    // The kernel heap in Linux's terms (`/proc/slabinfo` has it by cache):
    // slabs, the part of them its shrinker can give back now, and the mapped
    // area's pages
    let slab = crate::mm::heap::slab_bytes() as u64 / 1024;
    let reclaimable = ((crate::mm::heap::SHRINKER.count)() as u64 * 4).min(slab);
    pos += write_kb_line(buf, pos, "Slab", slab);
    pos += write_kb_line(buf, pos, "SReclaimable", reclaimable);
    pos += write_kb_line(buf, pos, "SUnreclaim", slab - reclaimable);
    pos += write_kb_line(buf, pos, "VmallocUsed", crate::mm::vmap::mapped_pages() as u64 * 4);
    // Every task's kernel stack, and the deepest one went: of the tasks gone,
    // and of the live ones too with `stackdepth` (`mm::kstack`)
    let (stacks, deepest) = crate::mm::kstack::stats();
    pos += write_kb_line(buf, pos, "KernelStack", stacks as u64 / 1024);
    pos += write_kb_line(buf, pos, "KernelStackMax", deepest.div_ceil(1024) as u64);
    // MikuOS's own lines. Live vnodes: the table has no ceiling
    pos += write_str(buf, pos, "Vnodes:         ");
    pos += write_u64(buf, pos, vnode_used as u64);
    pos += write_str(buf, pos, "\nTmpfsPages:     ");
    pos += write_u64(buf, pos, pages_total as u64);
    pos += write_str(buf, pos, "\nKernelHeap:     ");
    pos += write_u64(buf, pos, heap_used as u64);
    pos += write_str(buf, pos, "/");
    pos += write_u64(buf, pos, heap_total as u64);
    pos += write_str(buf, pos, " (");
    pos += write_u64(buf, pos, heap_free as u64);
    pos += write_str(buf, pos, " free)\n");
    pos
}

/// `/proc/mounts`: what the mount table holds, each with the flavour its own
/// superblock describes and whether it is writable
fn format_mounts(buf: &mut [u8; PROC_BUF]) -> usize {
    let mut pos = 0;
    pos += write_str(buf, pos, "tmpfs on / type tmpfs (rw)\n");
    pos += write_str(buf, pos, "devfs on /dev type devfs (rw)\n");
    pos += write_str(buf, pos, "procfs on /proc type procfs (ro)\n");

    let mounts = crate::fs::ext::mount::mounted_slots_snapshot();
    for entry in mounts.iter().flatten() {
        let (slot, vnode, tag) = *entry;
        if pos + 96 > PROC_BUF {
            break;
        }
        let mut path = [0u8; 64];
        let plen = crate::fs::vfs::core::with_vfs_ro(|v| v.abs_path_of(vnode as usize, &mut path));
        pos += write_str(buf, pos, "/dev/blk");
        pos += write_u64(
            buf,
            pos,
            crate::fs::ext::mount::slot_drive(slot).unwrap_or(0) as u64,
        );
        pos += write_str(buf, pos, " on ");
        if plen > 0 {
            pos += write_str(
                buf,
                pos,
                core::str::from_utf8(&path[..plen]).unwrap_or("?"),
            );
        } else {
            pos += write_str(buf, pos, "?");
        }
        pos += write_str(buf, pos, " type ");
        pos += write_str(buf, pos, tag);
        pos += write_str(
            buf,
            pos,
            if crate::fs::ext::mount::slot_read_only(slot) { " (ro)\n" } else { " (rw)\n" },
        );
    }
    pos
}

/// `/proc/stat`: Linux's `cpu` lines first, then this machine's own
///
/// `cpu  user nice system idle iowait irq softirq steal guest guest_nice`, summed and then per processor, in timer ticks (`hz:` below says how many a second). Only user, system and idle are counted: the tick knows whether it interrupted ring 3, the kernel or the idle task. The summed line is the file's first, where a CPU meter looks: end4's is `^cpu\s+(\d+)...` on the whole text, with no multiline flag
fn format_stat(buf: &mut [u8; PROC_BUF], vfs: &VfsNumbers) -> usize {
    let mut pos = 0;
    let cpus = crate::arch::x86_64::percpu::cpu_count().max(1);
    let (mut u, mut sy, mut id) = (0, 0, 0);
    for c in 0..cpus {
        let (a, b, i) = crate::sched::cpu_ticks(c);
        u += a;
        sy += b;
        id += i;
    }
    pos += write_cpu_line(buf, pos, None, (u, sy, id));
    for c in 0..cpus {
        if pos + 96 > PROC_BUF {
            break;
        }
        pos += write_cpu_line(buf, pos, Some(c), crate::sched::cpu_ticks(c));
    }
    let ticks = uptime_ticks();
    pos += write_str(buf, pos, "ticks: ");
    pos += write_u64(buf, pos, ticks);
    pos += write_str(buf, pos, "\nvnodes: ");
    // Handed in: this runs under the VFS lock (`file_ops`), and taking it again
    // here would deadlock
    pos += write_u64(buf, pos, vfs.vnodes as u64);
    // Lookups that asked a filesystem, misses a negative entry answered, misses
    // a complete directory answered, files in the icache
    // (`dev/vfs-dentries-inodes.md`, "Stage 5")
    pos += write_str(buf, pos, "\ndcache: ");
    for (i, v) in [vfs.fs_lookups, vfs.negative_hits, vfs.complete_misses, vfs.icache as u64].into_iter().enumerate() {
        if i > 0 {
            pos += write_str(buf, pos, " ");
        }
        pos += write_u64(buf, pos, v);
    }
    // The disk page cache: hits, misses, fills, hits answered with no VFS lock
    // (`dev/vfs-locking.md`, "4b"), and for write-back the pages dirty now,
    // dirtied and cleaned (`dev/fs-architecture.md`)
    pos += write_str(buf, pos, "\nfilecache: ");
    let fc = {
        let c = crate::fs::vfs::filecache::lock();
        [
            c.hits.load(core::sync::atomic::Ordering::Relaxed),
            c.misses.load(core::sync::atomic::Ordering::Relaxed),
            c.fills,
            crate::fs::vfs::filecache::UNLOCKED_HITS.load(core::sync::atomic::Ordering::Relaxed),
            crate::fs::vfs::filecache::DIRTY_PAGES.load(core::sync::atomic::Ordering::Relaxed) as u64,
            c.dirtied,
            c.cleaned,
        ]
    };
    for (i, v) in fc.into_iter().enumerate() {
        if i > 0 {
            pos += write_str(buf, pos, " ");
        }
        pos += write_u64(buf, pos, v);
    }
    pos += write_str(buf, pos, "\nmax_pages: ");
    pos += write_u64(buf, pos, crate::fs::vfs::pages::tmpfs_limit() as u64);
    pos += write_str(buf, pos, "\nmax_fds: ");
    pos += write_u64(buf, pos, MAX_OPEN_FILES as u64);
    pos += write_str(buf, pos, "\nheap_kb: ");
    pos += write_u64(buf, pos, (crate::mm::heap::total() / 1024) as u64);
    pos += write_str(buf, pos, "\nprocs: ");
    pos += write_u64(buf, pos, crate::sched::get_stats().len() as u64);
    pos += write_str(buf, pos, "\nhz: ");
    pos += write_u64(buf, pos, tick_hz());
    pos += write_str(buf, pos, "\n");
    pos
}

/// `cpu  1 0 2 3 0 0 0 0 0 0`, or `cpu4 ...` for one processor
fn write_cpu_line(buf: &mut [u8; PROC_BUF], pos: usize, cpu: Option<usize>,
                  (user, system, idle): (u64, u64, u64)) -> usize {
    let mut n = write_str(buf, pos, "cpu");
    match cpu {
        Some(c) => n += write_u64(buf, pos + n, c as u64),
        None => n += write_str(buf, pos + n, " "),
    }
    for v in [user, 0, system, idle, 0, 0, 0, 0, 0, 0] {
        n += write_str(buf, pos + n, " ");
        n += write_u64(buf, pos + n, v);
    }
    n += write_str(buf, pos + n, "\n");
    n
}

/// What the machine thinks the time is
///
/// The kernel knows the time zone (the installer stores it). `clock_gettime`
/// returns UTC and there is no `/etc/localtime`, so ring 3 reads it here. Local
/// time is `unix + tz_min * 60`
fn format_clock(buf: &mut [u8; PROC_BUF]) -> usize {
    let mut pos = 0;
    pos += write_str(buf, pos, "unix: ");
    pos += write_u64(buf, pos, crate::kcore::clock::now_unix());
    pos += write_str(buf, pos, "\ntz_min: ");
    let tz = crate::kcore::clock::tz_offset();
    if tz < 0 {
        pos += write_str(buf, pos, "-");
        pos += write_u64(buf, pos, (-tz) as u64);
    } else {
        pos += write_u64(buf, pos, tz as u64);
    }
    pos += write_str(buf, pos, "\nset: ");
    pos += write_str(buf, pos, if crate::kcore::clock::is_set() { "yes" } else { "no" });
    pos += write_str(buf, pos, "\nsource: ");
    pos += write_str(buf, pos, crate::kcore::clock::source());
    pos += write_str(buf, pos, "\n");
    pos
}

fn format_heap(buf: &mut [u8; PROC_BUF]) -> usize {
    let used = crate::mm::heap::used();
    let free = crate::mm::heap::free();
    let total = crate::mm::heap::total();

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

/// Bytes into the file, as many as fit in the buffer
fn write_str(buf: &mut [u8; PROC_BUF], pos: usize, s: &str) -> usize {
    let b = s.as_bytes();
    let l = b.len().min(PROC_BUF.saturating_sub(pos));
    buf[pos..pos + l].copy_from_slice(&b[..l]);
    l
}

fn write_u64(buf: &mut [u8; PROC_BUF], pos: usize, val: u64) -> usize {
    if val == 0 {
        if pos < PROC_BUF {
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
    let l = i.min(PROC_BUF.saturating_sub(pos));
    for j in 0..l {
        buf[pos + j] = tmp[i - 1 - j];
    }
    l
}

pub const PROC_ENTRIES: &[&str] = &[
    "version", "uptime", "meminfo", "mounts", "cpuinfo", "stat", "heap", "diskstats",
    "net", "clock", "slabinfo", "lockstat",
];

/// `/proc/lockstat`: what each sleeping lock has cost (`kcore::sched_lock`,
/// `dev/kernel-core.md`, "Lock statistics")
fn format_lockstat(buf: &mut [u8; 8192]) -> usize {
    struct W<'a> {
        buf: &'a mut [u8; 8192],
        pos: usize,
    }
    impl core::fmt::Write for W<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let b = s.as_bytes();
            let n = b.len().min(self.buf.len() - self.pos);
            self.buf[self.pos..self.pos + n].copy_from_slice(&b[..n]);
            self.pos += n;
            Ok(())
        }
    }
    let mut w = W { buf, pos: 0 };
    crate::kcore::sched_lock::report(&mut w);
    use core::fmt::Write;
    use core::sync::atomic::Ordering;
    let _ = writeln!(
        w,
        "# calls that gave way to a filesystem's lock {}, found it taken and waited under the VFS lock {}",
        crate::fs::vfs::core::GAVE_WAY.load(Ordering::Relaxed),
        crate::fs::vfs::core::WAITED_UNDER.load(Ordering::Relaxed)
    );
    for (what, c) in ["reads", "writes"].iter().zip(crate::io::block::IO_UNDER_VFS.iter()) {
        let _ = writeln!(
            w,
            "# disk {} under the VFS lock {}, {} us in all, the longest {} us",
            what,
            c[0].load(Ordering::Relaxed),
            c[1].load(Ordering::Relaxed) / 1000,
            c[2].load(Ordering::Relaxed) / 1000
        );
    }
    w.pos
}

/// `/proc/slabinfo`, in Linux's version 2.1 columns, for `slabtop` and the
/// like. There are no tunables to report: a magazine's size is fixed
/// (`mm::heap::magazine`)
fn format_slabinfo(buf: &mut [u8; 8192]) -> usize {
    use core::fmt::Write;
    struct W<'a> {
        buf: &'a mut [u8; 8192],
        pos: usize,
    }
    impl Write for W<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let b = s.as_bytes();
            let n = b.len().min(self.buf.len() - self.pos);
            self.buf[self.pos..self.pos + n].copy_from_slice(&b[..n]);
            self.pos += n;
            Ok(())
        }
    }
    let mut w = W { buf, pos: 0 };
    let _ = w.write_str("slabinfo - version: 2.1\n");
    let _ = w.write_str("# name            <active_objs> <num_objs> <objsize> <objperslab> <pagesperslab> \
                         : tunables <limit> <batchcount> <sharedfactor> : slabdata <active_slabs> <num_slabs> <sharedavail>\n");
    crate::mm::heap::each_cache(|c| {
        let _ = write!(w, "{:<17} {:>6} {:>6} {:>6} {:>4} {:>4} : tunables {:>4} {:>4} {:>4} : slabdata {:>6} {:>6} {:>6}\n",
                       c.name, c.active, c.objects, c.objsize, c.per_slab, c.pages_per_slab,
                       0, 0, 0, c.active_slabs, c.slabs, 0);
    });
    w.pos
}

/// The network stack's state, for ring 3: `net::get_ip` and its neighbours as
/// one text file
fn format_net(buf: &mut [u8; 512]) -> usize {
    use core::fmt::Write;

    struct W<'a> {
        buf: &'a mut [u8; 512],
        pos: usize,
    }
    impl<'a> Write for W<'a> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let b = s.as_bytes();
            let n = b.len().min(self.buf.len() - self.pos);
            self.buf[self.pos..self.pos + n].copy_from_slice(&b[..n]);
            self.pos += n;
            Ok(())
        }
    }

    let ip = crate::net::get_ip();
    let dns = crate::net::get_dns();
    let mac = crate::net::get_mac();
    let mut w = W { buf, pos: 0 };
    let _ = write!(w, "link: {}\n", if crate::net::is_ready() { "up" } else { "down" });
    let _ = write!(w, "ip: {}.{}.{}.{}\n", ip[0], ip[1], ip[2], ip[3]);
    let _ = write!(w, "dns: {}.{}.{}.{}\n", dns[0], dns[1], dns[2], dns[3]);
    let _ = write!(w, "mac: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n",
                   mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

    // Who the card belongs to, and the handoff state. A program opening
    // /dev/net0 needs to know it is allowed to, and lost frames need "nobody is
    // reading" told from "the reader offered a buffer too small"
    let raw = crate::net::raw::enabled();
    let _ = write!(w, "owner: {}\n", if raw { "/dev/net0" } else { "kernel" });
    if raw {
        let (rd, wr, drop, under) = crate::net::raw::stats();
        let _ = write!(w, "raw: read={} written={} dropped={} undersized={}\n",
                       rd, wr, drop, under);
    }
    w.pos
}

/// Linux /proc/diskstats analogue, one line per active block device:
/// name kind ios sectors_read sectors_written sectors_discarded errors retries
/// avg_latency_us state
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
    for id in 0..crate::fs::vfs::types::MAX_BLOCK_DEVICES as u8 {
        let Some(info) = crate::io::block::info(id) else { continue };
        if info.total_sectors == 0 && info.model_len == 0 {
            continue;
        }
        let Some(st) = crate::io::block::dev_stats(id) else { continue };
        let kind = match st.kind {
            crate::io::block::DevKind::Ata       => "ata",
            crate::io::block::DevKind::VirtioBlk => "virtio",
            crate::io::block::DevKind::Ahci      => "ahci",
            crate::io::block::DevKind::Nvme      => "nvme",
            crate::io::block::DevKind::Ramdisk   => "ramdisk",
        };
        let state = match st.state {
            crate::io::block::DevState::Online   => "online",
            crate::io::block::DevState::Degraded => "degraded",
            crate::io::block::DevState::Offline  => "offline",
        };
        let _ = writeln!(
            w,
            "blk{} {} {} {} {} {} {} {} {} {}",
            id, kind, st.ios, st.sectors_read, st.sectors_written,
            st.sectors_discarded, st.io_errors, st.retries, st.avg_io_us, state
        );
    }
    w.pos
}
