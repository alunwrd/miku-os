use crate::shell::SHELL;
use crate::vfs::with_vfs_ro;
use crate::{allocator, console, cprint, cprintln, print, print_info, print_success, println};

pub fn cmd_echo(text: &str) {
    if !text.is_empty() {
        println!("{}", text);
    }
}

pub fn cmd_info() {
    let (vn, mn) = with_vfs_ro(|v| (v.total_vnodes(), v.total_mounts()));
    let ticks = crate::vfs::procfs::uptime_ticks();
    let total_secs = ticks / 1000;
    let hours = total_secs / 3600;
    let mins  = (total_secs % 3600) / 60;
    let secs  = total_secs % 60;
    let heap_used  = allocator::used();
    let heap_total = allocator::HEAP_SIZE;

    let (pmm_used, pmm_total) = crate::pmm::stats();

    let usable_ram_kb = pmm_total * 4;
    let used_ram_kb   = pmm_used * 4 + heap_used / 1024;
    let free_ram_kb   = usable_ram_kb.saturating_sub(used_ram_kb);

    cprintln!(57, 197, 187,  "  MikuOS v0.2.0");
    cprintln!(230, 240, 240, "  VNodes: {}/{}", vn, crate::vfs::MAX_VNODES);
    cprintln!(230, 240, 240, "  Mounts: {}", mn);
    cprintln!(230, 240, 240, "  Heap:   {} / {} KB", heap_used / 1024, heap_total / 1024);
    cprintln!(230, 240, 240, "  RAM:    {} / {} MB", used_ram_kb / 1024, usable_ram_kb / 1024);
    cprintln!(230, 240, 240, "  Usable: {} MB  Free: {} MB", usable_ram_kb / 1024, free_ram_kb / 1024);

    if crate::swap::swap_is_active() {
        let stotal = crate::swap::swap_total_kb();
        let sused  = crate::swap::swap_used_kb();
        let sfree  = crate::swap::swap_free_kb();
        cprintln!(255, 200, 80, "  Swap:   {} / {} KB  free: {} KB", sused, stotal, sfree);
    } else {
        cprintln!(128, 140, 140, "  Swap:   inactive");
    }

    cprintln!(120, 140, 140, "  Uptime: {}h {}m {}s", hours, mins, secs);
}

pub fn cmd_memmap() {
    use crate::grub;

    cprintln!(57, 197, 187, "  Physical Memory Map");
    cprintln!(57, 197, 187, "  {:18}  {:18}  {}  Size", "Base", "Length", "Type    ");

    let mmap = match grub::memory_map() {
        Some(m) => m,
        None => {
            crate::print_error!("  memory map not available");
            return;
        }
    };

    let mut total_usable: u64 = 0;
    let mut total_all:    u64 = 0;

    for entry in mmap {
        let base       = entry.base();
        let length     = entry.length();
        let mem_type   = entry.mem_type();
        let type_str   = grub::mmap_type_str(mem_type);
        let (r, g, b)  = grub::mmap_type_color(mem_type);
        let mb = length / (1024 * 1024);
        let kb = (length % (1024 * 1024)) / 1024;

        crate::cprintln!(r, g, b,
            "  {:#018x}  {:#018x}  {}  {}MB {}KB",
            base, length, type_str, mb, kb
        );

        if mem_type == grub::MMAP_USABLE {
            total_usable += length;
        }
        total_all += length;
    }

    cprintln!(100, 220, 150, "  Total USABLE: {} MB", total_usable / 1024 / 1024);
    cprintln!(230, 240, 240, "  Total ALL:    {} MB", total_all    / 1024 / 1024);
}

pub fn cmd_heap() {
    let used = allocator::used();
    let free = allocator::free();
    let total = allocator::HEAP_SIZE;
    cprintln!(57, 197, 187, "  Heap Allocator");
    println!("  Total:  {} bytes ({} KB)", total, total / 1024);
    println!("  Used:   {} bytes ({} KB)", used, used / 1024);
    println!("  Free:   {} bytes ({} KB)", free, free / 1024);
    let pct = if total > 0 { (used * 100) / total } else { 0 };
    println!("  Usage:  {}%", pct);
    if pct > 80 {
        cprintln!(220, 220, 100, "  warning: heap usage high");
    }
}

pub fn cmd_poweroff() {
    crate::serial_println!("[kern] poweroff requested");
    cprintln!(220, 200, 80, "  shutting down...");

    // Sync filesystems before stopping services
    sync_filesystems();

    // Graceful shutdown via mikuD (stops all services in order)
    if crate::mikud::is_running() {
        crate::mikud::poweroff();
    } else {
        crate::power::shutdown();
    }
}

pub fn cmd_reboot() {
    crate::serial_println!("[kern] reboot requested");
    cprintln!(220, 200, 80, "  rebooting...");

    sync_filesystems();

    if crate::mikud::is_running() {
        crate::mikud::reboot();
    } else {
        crate::power::reboot();
    }
}

fn sync_filesystems() {
    if crate::commands::ext2_cmds::is_ext2_ready() {
        crate::commands::ext2_cmds::with_ext2_pub(|fs| {
            if fs.has_dirty_data() {
                let _ = fs.periodic_sync();
                crate::serial_println!("[kern] filesystem synced");
            }
        });
    }
}

pub fn cmd_help() {
    cprintln!(57, 197, 187, "  VFS Commands:");
    cprintln!(128, 222, 217, "  ls cd pwd mkdir touch cat write rm rmdir mv stat");
    cprintln!(128, 222, 217, "  ln -s <target> <n>   readlink   chmod <mode> <path>");
    cprintln!(128, 222, 217, "  df info mount umount echo clear help history heap");
    cprintln!(128, 222, 217, "  rm -rf <path>");

    cprintln!(57, 197, 187, "  Disk / Partitions:");
    cprintln!(128, 222, 217, "  gpt <drive>             show GPT table");
    cprintln!(128, 222, 217, "  gpt.init <drive>        initialize GPT");
    cprintln!(128, 222, 217, "  gpt.add <d> <type> <MB> add partition (fs|swap)");
    cprintln!(128, 222, 217, "  gpt.del <drive> <part>  delete partition");
    cprintln!(128, 222, 217, "  mkswap <drive> <part>   format swap");
    cprintln!(128, 222, 217, "  swapon <drive> <part>   enable swap");
    cprintln!(128, 222, 217, "  swapoff                 disable swap");
    cprintln!(128, 222, 217, "  swapinfo                swap info");
    cprintln!(128, 222, 217, "  mkswap.raw <d> <lba> <mb>  format raw swap");
    cprintln!(128, 222, 217, "  swapon.raw <d> <lba> <sec> activate raw swap");
    cprintln!(128, 222, 217, "  swapon.auto              scan & activate swap");
    cprintln!(128, 222, 217, "  fs.list              show all mounted filesystems");
    cprintln!(128, 222, 217, "  fs.select <0|1>      switch active mount slot");
    cprintln!(128, 222, 217, "  fs.umount [0|1]      unmount slot (default: active)");

    cprintln!(57, 197, 187, "  Ext2 Commands:");
    cprintln!(128, 222, 217, "  ext2mount                mount ext2 disk");
    cprintln!(128, 222, 217, "  ext2info                 filesystem info");
    cprintln!(128, 222, 217, "  ext2ls [path]            list directory");
    cprintln!(128, 222, 217, "  ext2cat <path>           show file");
    cprintln!(128, 222, 217, "  ext2stat <path>          inode info");
    cprintln!(128, 222, 217, "  ext2write <path> <text>  write file");
    cprintln!(128, 222, 217, "  ext2append <path> <text> append to file");
    cprintln!(128, 222, 217, "  ext2mkdir <path>         create dir");
    cprintln!(128, 222, 217, "  ext2rm <path>            delete file");
    cprintln!(128, 222, 217, "  ext2rm -rf <path>        recursive delete");
    cprintln!(128, 222, 217, "  ext2rmdir <path>         delete empty dir");
    cprintln!(128, 222, 217, "  ext2mv <path> <newname>  rename");
    cprintln!(128, 222, 217, "  ext2cp <src> <dst>       copy file");
    cprintln!(128, 222, 217, "  ext2ln -s <tgt> <n>      symlink");
    cprintln!(128, 222, 217, "  ext2chmod <mode> <path>  change mode");
    cprintln!(128, 222, 217, "  ext2chown <u> <g> <path> change owner");
    cprintln!(128, 222, 217, "  ext2du [path]            disk usage");
    cprintln!(128, 222, 217, "  ext2tree [path]          directory tree");
    cprintln!(128, 222, 217, "  ext2fsck                 check filesystem");
    cprintln!(128, 222, 217, "  ext2cache                cache statistics");
    cprintln!(128, 222, 217, "  ext2cacheflush           flush block cache");

    cprintln!(57, 197, 187, "  Mount:");
    cprintln!(128, 222, 217, "  mount ext2 <path>        mount ext2 at path");
    cprintln!(128, 222, 217, "  umount <path>            unmount");

    cprintln!(57, 197, 187, "  Ext3 Commands:");
    cprintln!(128, 222, 217, "  ext3mkjournal            create journal");
    cprintln!(128, 222, 217, "  ext3info                 journal info");
    cprintln!(128, 222, 217, "  ext3journal              show transactions");
    cprintln!(128, 222, 217, "  ext3recover              replay journal");
    cprintln!(128, 222, 217, "  ext3clean                mark journal clean");

    cprintln!(57, 197, 187, "  Network:");
    cprintln!(128, 222, 217, "  dhcp                     get ip via dhcp");
    cprintln!(128, 222, 217, "  ping <ip> [count]        ping host (ctrl+c to stop)");
    cprintln!(128, 222, 217, "  fetch <host> [port]      tcp connect + GET request");
    cprintln!(128, 222, 217, "  net status                adapter info");
    cprintln!(128, 222, 217, "  net pci                   list pci nics");
    cprintln!(128, 222, 217, "  net poll                  receive packets");
    cprintln!(128, 222, 217, "  net arp                   show arp table");
    cprintln!(128, 222, 217, "  net dns [<ip>]            show/set dns server");
    cprintln!(128, 222, 217, "  net ip <ip> <gw> <mask>  set ip manually");
    cprintln!(128, 222, 217, "  traceroute <host>        trace route to host");
    cprintln!(128, 222, 217, "  net send <ip> <port> <m> send udp packet");

    cprintln!(57, 197, 187, "  mikuD (init daemon):");
    cprintln!(128, 222, 217, "  sv list|status|start|stop|restart|reload <name>");
    cprintln!(128, 222, 217, "  sv enable|disable|mask|unmask|force-stop <name>");
    cprintln!(128, 222, 217, "  sv journal [name]        show event log");
    cprintln!(128, 222, 217, "  sv target [name]         show/set target");
    cprintln!(128, 222, 217, "  sv isolate <target>      switch target");
    cprintln!(128, 222, 217, "  sv analyze               boot timing");
    cprintln!(128, 222, 217, "  sv tree|rdeps <name>     dependency info");
    cprintln!(128, 222, 217, "  sv load|scan             load unit files");
    cprintln!(128, 222, 217, "  sv timer|socket          manage units");

    cprintln!(57, 197, 187, "  System:");
    cprintln!(128, 222, 217, "  ps                      thread list (CPU/RAM/stack)");
    cprintln!(128, 222, 217, "  top                      realtime process monitor");
    cprintln!(128, 222, 217, "  nice <pid> <1-20>       change priority");
    cprintln!(128, 222, 217, "  affinity <pid> <mask>   set CPU affinity");
    cprintln!(128, 222, 217, "  kill <pid>              kill thread");
    cprintln!(128, 222, 217, "  heap                     heap allocator info");
    cprintln!(128, 222, 217, "  memmap                 physical memory map");
    cprintln!(128, 222, 217, "  reboot                   restart system (graceful)");
    cprintln!(128, 222, 217, "  poweroff                 shutdown system (graceful)");
    println!("  exec <path>     - load and run ELF binary");
}

pub fn cmd_clear() {
    console::clear_screen();
}

pub fn cmd_history() {
    let sh = SHELL.lock();
    if sh.history_count == 0 {
        cprintln!(120, 140, 140, "  (empty)");
        return;
    }
    let start = if sh.history_count > 16 { sh.history_count - 16 } else { 0 };
    for i in start..sh.history_count {
        let idx = i % 16;
        let entry = &sh.history[idx];
        let s = unsafe { core::str::from_utf8_unchecked(&entry.buf[..entry.len]) };
        cprint!(120, 140, 140, "  {}: ", i + 1);
        cprintln!(230, 240, 240, "{}", s);
    }
}

pub fn cmd_ps() {
    let stats    = crate::scheduler::get_stats();
    let total_sw = crate::scheduler::total_switches();
    let now      = crate::interrupts::get_tick();
    let uptime_s = now / 1000;

    cprintln!(57, 197, 187,
        "  {:>4}  {:<12}  {:>2}  {:>3}  {:>5}  {:>6}  {:>6}  {:>5}  {:>6}",
        "PID", "NAME", "ST", "PRI", "CPU%", "UP(s)", "STK-U", "STK-A", "CTX-IN"
    );

    for s in &stats {
        let ci = s.cpu_pct_x10 / 10;
        let cf = s.cpu_pct_x10 % 10;
        let up = s.uptime_ticks / 1000;

        let (r, g, b) = match s.state {
            "R" => (100, 220, 150),
            "S" => (128, 222, 217),
            "Z" => (220, 220, 100),
            "X" => (180,  80,  80),
            _   => (180, 140, 220),
        };

        let sp = if s.stack_alloc_kb > 0 { s.stack_used_kb * 100 / s.stack_alloc_kb } else { 0 };
        let (sr, sg, sb) = if sp >= 80 { (220, 80, 80) } else if sp >= 50 { (220, 200, 80) } else { (100, 200, 150) };

        cprint!(200, 200, 200, "  {:>4}  {:<12}  ", s.pid, s.name);
        cprint!(r, g, b, "{:>2}", s.state);
        cprint!(200, 200, 200, "  {:>3}  {:>2}.{}%  {:>6}  ", s.priority, ci, cf, up);
        cprint!(sr, sg, sb, "{:>4}K ", s.stack_used_kb);
        cprint!(160, 160, 160, "{:>4}K  ", s.stack_alloc_kb);
        cprint!(200, 200, 200, "{:>6}", s.switch_in);
        if s.cpu_mask != u64::MAX { cprint!(180, 140, 220, "  cpu={:#x}", s.cpu_mask); }
        crate::println!();
    }

    let (pu, pt) = crate::pmm::stats();
    let hk = crate::allocator::used() / 1024;
    cprintln!(100, 100, 100,
        "  threads={} sw={} uptime={}s  ram={}/{}MB  heap={}KB",
        stats.len(), total_sw, uptime_s, pu * 4 / 1024, pt * 4 / 1024, hk
    );
}

pub fn cmd_top() {
    use crate::net::CTRL_C;
    use core::sync::atomic::Ordering;

    CTRL_C.store(false, Ordering::SeqCst);
    x86_64::instructions::interrupts::enable();
    cprintln!(57, 197, 187, "  top - Ctrl+C to exit");

    loop {
        if CTRL_C.load(Ordering::SeqCst) { break; }

        let stats    = crate::scheduler::get_stats();
        let total_sw = crate::scheduler::total_switches();
        let now      = crate::interrupts::get_tick();
        let uptime_s = now / 1000;
        let (pu, pt) = crate::pmm::stats();
        let hk       = crate::allocator::used() / 1024;

        cprintln!(57, 197, 187,
            "  uptime={}s  ram={}/{}MB  heap={}KB  threads={}  sw={}",
            uptime_s, pu * 4 / 1024, pt * 4 / 1024, hk, stats.len(), total_sw
        );
        cprintln!(57, 197, 187,
            "  {:>4}  {:<12}  {:>2}  {:>3}  {:>6}  {:>5}  {:>6}",
            "PID", "NAME", "ST", "PRI", "CPU%", "STK-U", "UP(s)"
        );

        let mut sorted = stats.clone();
        sorted.sort_by(|a, b| b.cpu_pct_x10.cmp(&a.cpu_pct_x10));

        for s in &sorted {
            let ci = s.cpu_pct_x10 / 10;
            let cf = s.cpu_pct_x10 % 10;
            let up = s.uptime_ticks / 1000;
            let (r, g, b) = if s.cpu_pct_x10 > 100 { (220, 120, 80) }
                            else if s.state == "R"  { (100, 220, 150) }
                            else                    { (128, 222, 217) };
            cprint!(200, 200, 200, "  {:>4}  {:<12}  ", s.pid, s.name);
            cprint!(r, g, b, "{:>2}", s.state);
            cprint!(200, 200, 200, "  {:>3}  {:>2}.{}%  {:>4}K  {:>6}",
                s.priority, ci, cf, s.stack_used_kb, up);
            crate::println!();
        }
        crate::println!();

        for _ in 0..100u32 {
            if CTRL_C.load(Ordering::SeqCst) { break; }
            crate::scheduler::sleep(10);
        }
    }
}

pub fn cmd_swaptest() {
    use crate::swap;

    if !swap::swap_is_active() {
        crate::print_error!("  swap is not active - run swapon first");
        return;
    }

    let hhdm = crate::grub::hhdm();

    let n = 256usize;
    cprintln!(57, 197, 187, "  swaptest: testing {} pages of swap I/O...", n);

    let drive_idx = swap::swap_drive_idx();
    let mut drive = match drive_idx {
        0 => crate::ata::AtaDrive::primary(),
        1 => crate::ata::AtaDrive::primary_slave(),
        2 => crate::ata::AtaDrive::secondary(),
        _ => crate::ata::AtaDrive::secondary_slave(),
    };

    let mut frames: alloc::vec::Vec<(u64, u32, u8)> = alloc::vec::Vec::new();

    cprintln!(128, 222, 217, "  Phase 1: allocating and swapping out {} pages...", n);
    for i in 0..n {
        let phys = match crate::pmm::alloc_frame() {
            Some(p) => p,
            None    => { cprintln!(220,180,80,"  alloc failed at {}",i); break; }
        };

        let pattern = ((i & 0xFF) as u8) ^ 0xA5;
        unsafe { core::ptr::write_bytes((phys + hhdm) as *mut u8, pattern, 4096); }

        match swap::swap_out_internal(phys, &mut drive) {
            Ok(slot) => {
                unsafe { core::ptr::write_bytes((phys + hhdm) as *mut u8, 0xDE, 4096); }
                frames.push((phys, slot, pattern));
            }
            Err(e) => {
                crate::pmm::free_frame(phys);
                cprintln!(220,100,80,"  swap_out failed at page {}: {:?}", i, e);
                break;
            }
        }
    }

    cprintln!(100, 220, 150, "  swapped out {} pages. swap used: {} KB",
        frames.len(), swap::swap_used_kb());

    cprintln!(128, 222, 217, "  Phase 2: swapping in and verifying...");
    let mut pass = 0usize;
    let mut fail = 0usize;

    for &(phys, slot, pattern) in frames.iter() {
        match swap::swap_in_internal(slot, phys, &mut drive) {
            Ok(()) => {
                let mut ok = true;
                unsafe {
                    let ptr = (phys + hhdm) as *const u8;
                    for j in 0..4096usize {
                        if *ptr.add(j) != pattern { ok = false; break; }
                    }
                }
                if ok { pass += 1; }
                else {
                    fail += 1;
                    if fail <= 3 {
                        crate::print_error!("  data mismatch: phys={:#x} slot={} pattern={:#x}",
                            phys, slot, pattern);
                    }
                }
            }
            Err(e) => {
                fail += 1;
                if fail <= 3 { crate::print_error!("  swap_in failed: {:?}", e); }
            }
        }
    }

    if fail == 0 {
        cprintln!(100, 220, 150, "  pass: {}/{} pages correct! Swap I/O works", pass, frames.len());
    } else {
        crate::print_error!("  fail: {}/{} pages corrupted", fail, frames.len());
    }

    for &(phys, _, _) in frames.iter() {
        crate::pmm::free_frame(phys);
    }

    let swap_final = swap::swap_used_kb();
    cprintln!(128, 222, 217, "  swap after free: {} KB (should be 0)", swap_final);
    if swap_final == 0 {
        cprintln!(100, 220, 150, "  Full swap cycle: pass");
    }
}

pub fn cmd_nice(pid_str: &str, prio_str: &str) {
    let pid = match parse_u64(pid_str) {
        Some(v) => v,
        None => { crate::print_error!("Usage: nice <pid> <1-20>"); return; }
    };
    let prio = match parse_u64(prio_str) {
        Some(v) if v >= 1 && v <= 20 => v as u8,
        _ => { crate::print_error!("priority must be 1-20"); return; }
    };
    crate::scheduler::set_priority(pid, prio);
    cprintln!(100, 220, 150, "  pid={} priority={}", pid, prio);
}

pub fn cmd_affinity(pid_str: &str, mask_str: &str) {
    let pid = match parse_u64(pid_str) {
        Some(v) => v,
        None => { crate::print_error!("Usage: affinity <pid> <hex_mask>"); return; }
    };
    let mask = parse_hex(mask_str).unwrap_or(u64::MAX);
    crate::scheduler::set_affinity(pid, mask);
    cprintln!(100, 220, 150, "  pid={} affinity={:#018x}", pid, mask);
}

fn parse_u64(s: &str) -> Option<u64> {
    let mut v: u64 = 0;
    if s.is_empty() { return None; }
    for b in s.bytes() {
        if b < b'0' || b > b'9' { return None; }
        v = v.checked_mul(10)?.checked_add((b - b'0') as u64)?;
    }
    Some(v)
}

fn parse_hex(s: &str) -> Option<u64> {
    let s = s.trim_start_matches("0x").trim_start_matches("0X");
    let mut v: u64 = 0;
    if s.is_empty() { return None; }
    for b in s.bytes() {
        let digit = match b {
            b'0'..=b'9' => (b - b'0') as u64,
            b'a'..=b'f' => (b - b'a' + 10) as u64,
            b'A'..=b'F' => (b - b'A' + 10) as u64,
            _ => return None,
        };
        v = v.checked_shl(4)?.checked_add(digit)?;
    }
    Some(v)
}

pub fn cmd_ldconfig(_args: &str) {
    crate::solib::ldconfig();
    let (count, bytes) = crate::solib::stats();
    crate::println!("ldconfig: {} libraries cached ({} KB)", count, bytes / 1024);
}

pub fn cmd_ldd(_path: &str) {
    let libs = crate::solib::list();
    if libs.is_empty() {
        crate::println!("  no cached libraries");
        return;
    }
    for (name, size, loads, shared) in &libs {
        let tag = if *shared { "shared" } else { "data" };
        crate::println!("  {} ({} bytes, loaded {} times, {})", name, size, loads, tag);
    }
}
