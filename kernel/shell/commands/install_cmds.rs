//! Putting MikuOS on a disk
//!
//! What an installed disk looks like
//!
//! ```text
//!   LBA 0      the boot sector: 440 bytes of GRUB, then the protective MBR
//!   LBA 2048   partition 1, BIOS boot, 2 MiB: core.img, written raw
//!   LBA 6144   partition 2, "mikuos": /boot, /bin, /lib/firmware
//! ```
//!
//! The BIOS reads sector 0, and sector 0 has to say where the rest of the
//! bootloader is before there is a filesystem to look it up in. So partition 1
//! exists and `CORE_LBA` is a constant: the number was written into the boot
//! sector when the ISO was built. `mbr.bin` and `core.bin` are opaque here: the
//! two numbers that had to be patched were patched by the builder, which has
//! GRUB's tools.
//!
//! Why two commands and no question
//!
//! `install plan` writes nothing and prints the line to type next; `install go`
//! takes the disk's size back as an argument and refuses if it does not match
//! what it reads now. So a disk is never chosen by this command, only
//! confirmed. The shell cannot ask a question anyway: the keyboard thread fills
//! the pending line and the shell thread runs the command, so a command waiting
//! for another line would wait on itself.

use alloc::string::String;
use alloc::vec;

use crate::fs::ext::mount::{
    self, dev_for_idx, with_ext2_slot, MAX_MOUNTS,
};
use crate::fs::gpt::{self, GUID_BIOS_BOOT, GUID_LINUX_FS};
use crate::fs::mkfs::{mkfs, FsType, MkfsError, MkfsParams};
use crate::fs::vfs::types::BlockDevId;
use crate::{cprintln, print_error, print_success, print_warn, println};

/// Where core.img starts on every disk MikuOS installs to.
///
/// This must equal `INSTALL_CORE_LBA` in `builder/src/main.rs`. The builder
/// writes this number into the boot sector; nothing at run time can check it,
/// and a disagreement is a disk that partitions, formats and copies and then
/// does not boot.
const CORE_LBA: u64 = 2048;
/// 2 MiB for a 170 KiB core.img: room for modules a future grub.cfg might want
/// without repartitioning
const BIOS_BOOT_SECTORS: u64 = 4096;
const ROOT_LBA: u64 = CORE_LBA + BIOS_BOOT_SECTORS;
/// GPT's backup header and entry array live in the last 33 sectors; the
/// primary copy costs another one at the front
const GPT_TAIL: u64 = 34;

/// What the root filesystem is called, in the superblock and on the boot line.
/// The generated `root=LABEL=` has to name what `mkfs` wrote
const ROOT_LABEL: &str = "mikuos";

/// What the root is formatted as.
///
/// A machine that installs software writes to its root, so it needs a journal:
/// a package manager can recover its own transaction from its log only if the
/// filesystem under it is still a filesystem afterwards.
///
/// Named once so the plan, the format step and every line printing it say the
/// same
const ROOT_FS: FsType = FsType::Ext4;

const SRC_KERNEL: &str = "/boot/kernel.elf";
const SRC_MBR: &str = "/boot/grub/i386-pc/mbr.bin";
const SRC_CORE: &str = "/boot/grub/i386-pc/core.bin";
/// Copied whole, in this order. Not `/boot`: its three files are placed one at
/// a time, because two of them are written to raw sectors as well
const COPY_TREES: [&str; 2] = ["/bin", "/lib/firmware"];

/// Read and written a chunk at a time: firmware blobs run to tens of megabytes
const CHUNK: usize = 1 << 20;

pub fn cmd_install(args: &str) {
    let mut p = args.split_whitespace();
    match p.next().unwrap_or("") {
        "plan" => plan(p.next()),
        "go" => go(p.next(), p.next()),
        "gui" => crate::setup::start(),
        _ => usage(),
    }
}

// Saying how far along it is
//
// `go` prints a line per step, which is no report for a window: a progress bar
// needs a number while the step runs. So the steps publish where they are, and
// the GUI reads this

pub mod progress {
    use spin::Mutex;

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum Step {
        Idle,
        Partitioning,
        Formatting,
        Copying,
        Bootloader,
        Done,
        Failed,
    }

    impl Step {
        pub fn name(self) -> &'static str {
            match self {
                Step::Idle => "waiting",
                Step::Partitioning => "Partitioning the disk",
                Step::Formatting => "Creating the filesystem",
                Step::Copying => "Copying the system",
                Step::Bootloader => "Making it bootable",
                Step::Done => "Done",
                Step::Failed => "Failed",
            }
        }
    }

    #[derive(Clone, Copy)]
    pub struct State {
        pub step: Step,
        pub bytes_done: u64,
        pub bytes_total: u64,
        /// Set when `step` is `Failed`, so the GUI can say what went wrong
        /// rather than only that something did
        pub error: &'static str,
    }

    static STATE: Mutex<State> = Mutex::new(State {
        step: Step::Idle,
        bytes_done: 0,
        bytes_total: 0,
        error: "",
    });

    pub fn get() -> State {
        *STATE.lock()
    }

    pub fn begin(total: u64) {
        let mut s = STATE.lock();
        s.step = Step::Partitioning;
        s.bytes_done = 0;
        s.bytes_total = total;
        s.error = "";
    }

    pub fn step(step: Step) {
        STATE.lock().step = step;
    }

    pub fn add_bytes(n: u64) {
        STATE.lock().bytes_done += n;
    }

    pub fn fail(why: &'static str) {
        let mut s = STATE.lock();
        s.step = Step::Failed;
        s.error = why;
    }
}

/// The drive an `install go` running on its own thread was told to use.
/// Parked here because a thread entry point takes no arguments
static BACKGROUND: spin::Mutex<(usize, u64)> = spin::Mutex::new((usize::MAX, 0));

/// Runs an install on its own thread.
///
/// The GUI keeps drawing while ninety megabytes are copied, and the progress
/// comes back through `progress::get`
pub fn start_background(drive: usize, size_mb: u64) {
    *BACKGROUND.lock() = (drive, size_mb);
    crate::sched::spawn_named(background_main, "install", 1);
}

fn background_main() -> ! {
    let (drive, mb) = *BACKGROUND.lock();
    if drive != usize::MAX {
        let mut d = [0u8; 8];
        let mut m = [0u8; 24];
        go(Some(fmt_usize(drive, &mut d)), Some(fmt_u64(mb, &mut m)));
    }
    // The work is finished. `kill` marks the thread dead and the next schedule
    // never comes back here: the loop is what the type signature requires, not
    // a busy wait
    crate::sched::kill(crate::sched::current_pid());
    loop {
        crate::sched::yield_now();
    }
}

/// Decimal into a caller's buffer, because `go` parses its arguments as the
/// shell hands them over, and this caller has no command line
fn fmt_usize(mut v: usize, buf: &mut [u8; 8]) -> &str {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 || i == 0 {
            break;
        }
    }
    core::str::from_utf8(&buf[i..]).unwrap_or("0")
}

fn fmt_u64(mut v: u64, buf: &mut [u8; 24]) -> &str {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 || i == 0 {
            break;
        }
    }
    core::str::from_utf8(&buf[i..]).unwrap_or("0")
}

fn usage() {
    println!("Usage: install plan <drive>       what it would do, writing nothing");
    println!("       install go <drive> <MB>    do it - the MB is the size 'plan' printed");
    println!("  drives 0-3 are ATA, 4-7 PCI/NVMe/virtio-blk ('disks' lists them)");
    println!("  MikuOS installs for a BIOS machine. UEFI is not supported: grub's");
    println!("  multiboot2 faults under edk2, and the BIOS path is the one that works.");
}

// Looking, which is all `plan` does

struct Target {
    drive: usize,
    dev: BlockDevId,
    sectors: u64,
}

impl Target {
    fn mb(&self) -> u64 {
        self.sectors / 2048
    }
}

/// Identify the disk without writing to it. `None` has already said why
fn look_at(drive: Option<&str>) -> Option<Target> {
    let Some(drive) = drive.and_then(|s| s.parse::<usize>().ok()).filter(|&d| d <= 7) else {
        print_error!("  install: name a drive, 0-7 ('disks' lists them)");
        return None;
    };
    let dev = dev_for_idx(drive);
    let sectors = gpt::gpt_probe_sectors(dev);
    if sectors == 0 {
        print_error!("  install: drive {} does not answer", drive);
        return None;
    }
    Some(Target { drive, dev, sectors })
}

/// One disk, as a window has to show it: a name, a size, and what is on it
///
/// The same three sources `install plan` prints from: the list a person picks
/// from and the disk written to have to be described the same way
pub struct DriveSummary {
    pub drive: usize,
    pub sectors: u64,
    pub model: [u8; 40],
    pub model_len: u8,
    /// How many partitions are on it now, and `false` when there is no GPT,
    /// which is not an error
    pub partitions: u8,
    pub has_gpt: bool,
    /// True when this is the medium the running system booted from. Installing
    /// onto it would erase what is doing the installing
    pub is_source: bool,
    /// What the disk is attached by, the second thing a person reads after its
    /// name
    pub kind: crate::io::block::DevKind,
    /// Sectors already inside a partition. What is left could be used without
    /// destroying anything
    pub used_sectors: u64,
}

impl DriveSummary {
    pub fn mb(&self) -> u64 {
        self.sectors / 2048
    }

    /// Megabytes outside every partition. A disk with no partition table is
    /// entirely free
    pub fn free_mb(&self) -> u64 {
        self.sectors.saturating_sub(self.used_sectors) / 2048
    }

    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.model[..self.model_len as usize]).unwrap_or("")
    }

    /// The bus, as a person names it. Nothing here knows whether a disk spins,
    /// so an ATA disk is "SATA" and nothing claims to be an SSD it cannot see
    pub fn bus(&self) -> &'static str {
        use crate::io::block::DevKind;
        match self.kind {
            DevKind::Nvme => "NVMe SSD",
            DevKind::Ahci | DevKind::Ata => "SATA",
            DevKind::VirtioBlk => "virtio",
            DevKind::Ramdisk => "RAM",
        }
    }
}

/// Every drive that answers. Reads; writes nothing
pub fn drives() -> alloc::vec::Vec<DriveSummary> {
    let mut out = alloc::vec::Vec::new();
    for drive in 0..8usize {
        let dev = dev_for_idx(drive);
        let sectors = gpt::gpt_probe_sectors(dev);
        if sectors == 0 {
            continue;
        }
        let (model, model_len) = match crate::io::block::info(dev) {
            Some(i) => (i.model, i.model_len),
            None => ([0u8; 40], 0),
        };
        let (has_gpt, partitions, used_sectors) = match gpt::gpt_read(dev) {
            Ok(t) => (
                true,
                t.entries.iter().filter(|e| e.is_used()).count() as u8,
                t.entries.iter().filter(|e| e.is_used()).map(|e| e.size_sectors()).sum(),
            ),
            Err(_) => (false, 0, 0),
        };
        let kind = crate::io::block::dev_stats(dev)
            .map(|s| s.kind)
            .unwrap_or(crate::io::block::DevKind::Ata);
        // The medium is whichever mount carries the bootloader, and its drive
        // number is what the mount table says it is
        let is_source = source_slot()
            .and_then(mount::slot_drive)
            .is_some_and(|d| d == drive);
        out.push(DriveSummary {
            drive,
            sectors,
            model,
            model_len,
            partitions,
            has_gpt,
            is_source,
            kind,
            used_sectors,
        });
    }
    out
}

/// The slot the running system came from: the one with a bootloader in it
///
/// Found by looking, since something else may have been mounted by then and the
/// active slot is no longer the medium
fn source_slot() -> Option<usize> {
    (0..MAX_MOUNTS).find(|&s| {
        with_ext2_slot(s, |fs| fs.resolve_path(SRC_MBR).is_ok()).unwrap_or(false)
    })
}

/// Size of one file on the source, in bytes
fn file_size(slot: usize, path: &str) -> Option<u64> {
    with_ext2_slot(slot, |fs| {
        let ino = fs.resolve_path(path).ok()?;
        Some(fs.read_inode(ino).ok()?.size())
    })
    .flatten()
}

fn tree_size(slot: usize, path: &str) -> Option<(u32, u64)> {
    with_ext2_slot(slot, |fs| {
        let ino = fs.resolve_path(path).ok()?;
        fs.ext2_dir_size(ino).ok()
    })
    .flatten()
}

fn plan(drive: Option<&str>) {
    let Some(t) = look_at(drive) else { return };
    let Some(src) = source_slot() else {
        print_error!("  install: this medium carries no bootloader to write");
        println!("    ({} is missing - the ISO was built without GRUB's i386-pc parts)", SRC_MBR);
        return;
    };

    println!();
    cprintln!(128, 222, 217, "  MikuOS installs for a BIOS machine, not UEFI.");
    println!();

    cprintln!(57, 197, 187, "  target: drive {} - {} MB ({} sectors)",
              t.drive, t.mb(), t.sectors);
    match gpt::gpt_read(t.dev) {
        Ok(tbl) => {
            let used = tbl.entries.iter().filter(|e| e.is_used()).count();
            if used == 0 {
                println!("    a GPT with nothing in it");
            } else {
                println!("    a GPT with {} partition(s), all of which would go:", used);
                for (i, e) in tbl.entries.iter().enumerate().filter(|(_, e)| e.is_used()) {
                    let mut name = [0u8; 36];
                    let n = e.name_str(&mut name);
                    println!("      {}  {:<10} {:>7} MB  {}",
                             i + 1, e.type_name(), e.size_mb(),
                             core::str::from_utf8(&name[..n]).unwrap_or(""));
                }
            }
        }
        Err(_) => println!("    no GPT - whatever is on it now is not something this can read"),
    }

    let needed = ROOT_LBA + GPT_TAIL + 2048;
    if t.sectors < needed {
        println!();
        print_error!("  install: drive {} is too small - {} sectors, and the layout \
                      needs at least {}", t.drive, t.sectors, needed);
        return;
    }

    let core_sectors = file_size(src, SRC_CORE).unwrap_or(0).div_ceil(512);
    let root_sectors = t.sectors - ROOT_LBA - GPT_TAIL;

    println!();
    cprintln!(57, 197, 187, "  what would be written:");
    println!("    LBA {:<10} the boot sector, 440 bytes of it", 0);
    println!("    LBA {:<10} core.bin, {} sectors   (partition 1, BIOS boot, {} MB)",
             CORE_LBA, core_sectors, BIOS_BOOT_SECTORS / 2048);
    println!("    LBA {:<10} {} \"{}\", {} MB   (partition 2, the root)",
             ROOT_LBA, ROOT_FS.name(), ROOT_LABEL, root_sectors / 2048);

    println!();
    cprintln!(57, 197, 187, "  what would be copied:");
    let mut total = 0u64;
    if let Some(b) = file_size(src, SRC_KERNEL) {
        println!("    {:<18} {:>8} KB", SRC_KERNEL, b / 1024);
        total += b;
    }
    for tree in COPY_TREES {
        match tree_size(src, tree) {
            Some((files, bytes)) => {
                println!("    {:<18} {:>8} KB  in {} files", tree, bytes / 1024, files);
                total += bytes;
            }
            None => println!("    {:<18} {:>8}", tree, "not here"),
        }
    }
    println!("    {:<18} {:>8} MB", "in all", total / (1024 * 1024));

    // The installed disk is found at boot by its label, so a second volume with
    // that name would leave the new disk unbootable. Knowable now, before
    // anything is written
    let clashes: alloc::vec::Vec<_> = mount::scan_volumes()
        .into_iter()
        .filter(|p| p.label_str() == ROOT_LABEL && p.drive != t.drive)
        .collect();
    if !clashes.is_empty() {
        println!();
        print_warn!("  something else is already labelled '{}':", ROOT_LABEL);
        for p in &clashes {
            println!("    drive {}:{}", p.drive, p.start_lba);
        }
        println!("  with two of them the boot line cannot tell which is which. Either");
        println!("  unplug the other one or boot the 'root by number' menu entry.");
    }

    println!();
    cprintln!(255, 80, 80, "  !! everything on drive {} would be erased !!", t.drive);
    cprintln!(128, 222, 217, "  to do it:  install go {} {}", t.drive, t.mb());
    print_warn!("  (plan: nothing was written)");
}

// Doing it

fn go(drive: Option<&str>, size_mb: Option<&str>) {
    let Some(t) = look_at(drive) else { return };
    let Some(want_mb) = size_mb.and_then(|s| s.parse::<u64>().ok()) else {
        print_error!("  install: say the size 'install plan {}' printed, so that the \
                      disk being erased is one somebody looked at", t.drive);
        return;
    };
    if want_mb != t.mb() {
        print_error!("  install: drive {} is {} MB, not {} MB - refusing",
                     t.drive, t.mb(), want_mb);
        println!("    run 'install plan {}' and use the line it prints", t.drive);
        return;
    }
    let Some(src) = source_slot() else {
        print_error!("  install: this medium carries no bootloader to write");
        return;
    };
    let needed = ROOT_LBA + GPT_TAIL + 2048;
    if t.sectors < needed {
        print_error!("  install: drive {} is too small", t.drive);
        return;
    }

    // Reads the two blobs before anything is destroyed: if the medium cannot
    // produce them there is no point partitioning a disk that cannot be made
    // bootable
    let Some(mbr) = read_whole(src, SRC_MBR) else {
        print_error!("  install: cannot read {}", SRC_MBR);
        return;
    };
    let Some(core) = read_whole(src, SRC_CORE) else {
        print_error!("  install: cannot read {}", SRC_CORE);
        return;
    };
    if mbr.len() != 440 || core.is_empty() || core.len() % 512 != 0 {
        print_error!("  install: the bootloader blobs are {} and {} bytes, which is \
                      not what they should be", mbr.len(), core.len());
        return;
    }
    if (CORE_LBA + (core.len() / 512) as u64) > ROOT_LBA {
        print_error!("  install: core.bin is {} sectors and the boot partition holds {}",
                     core.len() / 512, BIOS_BOOT_SECTORS);
        return;
    }

    println!();
    cprintln!(255, 80, 80, "  erasing drive {} ({} MB)", t.drive, t.mb());
    // What the copy will amount to, so a progress bar has a denominator before
    // the first byte moves
    let payload = file_size(src, SRC_KERNEL).unwrap_or(0)
        + COPY_TREES
            .iter()
            .filter_map(|t| tree_size(src, t))
            .map(|(_, bytes)| bytes)
            .sum::<u64>();
    progress::begin(payload);
    mount::invalidate_drive_mounts(t.drive, 0);

    // 1. the table
    if let Err(e) = gpt::gpt_init(t.dev, t.sectors) {
        print_error!("  install: could not write a GPT: {:?}", e);
        progress::fail("could not write a partition table");
        return;
    }
    let seed = t.sectors as u32;
    if let Err(e) = gpt::gpt_add_partition_at(
        t.dev, GUID_BIOS_BOOT, CORE_LBA, BIOS_BOOT_SECTORS, "BIOS boot", seed)
    {
        print_error!("  install: could not make the boot partition: {:?}", e);
        progress::fail("could not make the boot partition");
        return;
    }
    let root_sectors = t.sectors - ROOT_LBA - GPT_TAIL;
    if let Err(e) = gpt::gpt_add_partition_at(
        t.dev, GUID_LINUX_FS, ROOT_LBA, root_sectors, "mikuos", seed ^ 0x5A5A)
    {
        print_error!("  install: could not make the root partition: {:?}", e);
        progress::fail("could not make the root partition");
        return;
    }
    print_success!("  partitioned: BIOS boot at {}, root at {} ({} MB)",
                   CORE_LBA, ROOT_LBA, root_sectors / 2048);

    // 2. the filesystem: see ROOT_FS
    progress::step(progress::Step::Formatting);
    let mut params = MkfsParams::new(ROOT_FS, t.drive);
    params.start_lba = ROOT_LBA as u32;
    params.total_sectors = root_sectors.min(u32::MAX as u64) as u32;
    params.label = [0u8; 16];
    params.label[..ROOT_LABEL.len()].copy_from_slice(ROOT_LABEL.as_bytes());
    match mkfs(crate::fs::host::KernelHost::new(t.dev), &params) {
        Ok(r) => {
            // What was made, from the parameters it was made with
            print_success!("  formatted: {}, {} blocks of {} bytes, {} inodes",
                           params.fs_type.name(),
                           r.total_blocks, r.block_size, r.total_inodes);
            // A filesystem quietly covering a fraction of the partition still
            // mounts and works, so nothing else would notice
            let fs_mb   = (r.total_blocks as u64 * r.block_size as u64) / (1024 * 1024);
            let part_mb = root_sectors / 2048;
            if fs_mb + 8 < part_mb {
                print_error!(
                    "  install: the filesystem covers {} MB of a {} MB partition - the rest is unreachable",
                    fs_mb, part_mb
                );
            }
        }
        Err(MkfsError::Io(e)) => {
            print_error!("  install: I/O error while formatting: {:?}", e);
            print_error!("  the disk is now in no state to be mounted");
            progress::fail("the disk failed while it was being formatted");
            return;
        }
        Err(e) => {
            print_error!("  install: could not format: {:?}", e);
            progress::fail("could not create the filesystem");
            return;
        }
    }

    // 3. somewhere to put the files
    if !mount::try_mount(t.drive, ROOT_LBA as u32) {
        print_error!("  install: the new filesystem would not mount");
        progress::fail("the new filesystem would not mount");
        return;
    }
    let dst = mount::active_slot_index();
    if dst == src {
        print_error!("  install: the target mounted onto the medium's own slot");
        progress::fail("the target mounted onto the medium's own slot");
        return;
    }

    // 4. the files
    progress::step(progress::Step::Copying);
    let mut copied = 0u64;
    match copy_boot(src, dst) {
        Ok(n) => copied += n,
        Err(e) => { fail_after_mount(dst, e); return; }
    }
    for tree in COPY_TREES {
        match copy_tree(src, dst, tree) {
            Ok(n) => {
                copied += n;
                print_success!("  copied {} ({} KB)", tree, n / 1024);
            }
            Err(e) => { fail_after_mount(dst, e); return; }
        }
    }

    // 5. the boot line, which is the one file this writes rather than copies
    progress::step(progress::Step::Bootloader);
    if let Err(e) = write_grub_cfg(dst, t.drive) {
        fail_after_mount(dst, e);
        return;
    }
    print_success!("  wrote /boot/grub/grub.cfg (root=LABEL={}, with {}:{} as the fallback)",
                   ROOT_LABEL, t.drive, ROOT_LBA);

    // 6. the bootloader, into sectors no filesystem owns
    if let Err(e) = write_bootloader(&t, &mbr, &core) {
        fail_after_mount(dst, e);
        return;
    }
    print_success!("  wrote the boot sector and core.bin ({} sectors at LBA {})",
                   core.len() / 512, CORE_LBA);

    let _ = mount::remount_ro(dst);
    mount::unmount_slot(dst);

    println!();
    progress::step(progress::Step::Done);
    print_success!("  MikuOS is on drive {} - {} MB copied", t.drive, copied / (1024 * 1024));
    cprintln!(128, 222, 217, "  take the medium out and boot it. The first menu entry \
                             mounts the root read-only; the second one mounts it writable.");
    cprintln!(128, 222, 217, "  the root is found by its label '{}', so the disk boots \
                              wherever it is plugged in.", ROOT_LABEL);
    print_warn!("  if another disk ever carries that same label, nothing is mounted on \
                 purpose - boot the third menu entry, which names drive {} directly.",
                t.drive);
}

/// Say what went wrong without leaving the target mounted behind
fn fail_after_mount(slot: usize, why: &'static str) {
    print_error!("  install: {}", why);
    progress::fail(why);
    let _ = mount::remount_ro(slot);
    mount::unmount_slot(slot);
    print_error!("  the disk is partitioned and formatted but not bootable");
}

// Copying

fn read_whole(slot: usize, path: &str) -> Option<alloc::vec::Vec<u8>> {
    let size = file_size(slot, path)? as usize;
    let mut out = vec![0u8; size];
    let mut done = 0usize;
    while done < size {
        let n = with_ext2_slot(slot, |fs| {
            let ino = fs.resolve_path(path).ok()?;
            let inode = fs.read_inode(ino).ok()?;
            fs.read_file(&inode, done as u64, &mut out[done..]).ok()
        })
        .flatten()?;
        if n == 0 {
            return None;
        }
        done += n;
    }
    Some(out)
}

/// Every directory on the way to `path`, made if it is not there
fn ensure_dir(slot: usize, path: &str) -> Result<u32, &'static str> {
    let mut ino = 2u32; // the root of an ext filesystem is always inode 2
    for part in path.split('/').filter(|p| !p.is_empty()) {
        let here = with_ext2_slot(slot, |fs| {
            match fs.ext2_lookup_in_dir(ino, part) {
                Ok(Some(found)) => Some(found),
                _ => fs.ext2_create_dir(ino, part, 0o755).ok(),
            }
        })
        .flatten();
        ino = here.ok_or("could not make a directory on the target")?;
    }
    Ok(ino)
}

/// One file, a chunk at a time
///
/// Two locks per chunk rather than one per file: the source and target are
/// different mounts, and `with_ext2` exists to avoid holding both
fn copy_file(
    src: usize, dst: usize, src_path: &str, dst_parent: u32, name: &str,
) -> Result<u64, &'static str> {
    let size = file_size(src, src_path).ok_or("a file on the medium went missing")?;
    let dst_ino = with_ext2_slot(dst, |fs| fs.ext2_create_file(dst_parent, name, 0o755))
        .and_then(|r| r.ok())
        .ok_or("could not create a file on the target")?;

    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    while done < size {
        let want = ((size - done) as usize).min(CHUNK);
        let got = with_ext2_slot(src, |fs| {
            let ino = fs.resolve_path(src_path).ok()?;
            let inode = fs.read_inode(ino).ok()?;
            fs.read_file(&inode, done, &mut buf[..want]).ok()
        })
        .flatten()
        .ok_or("read failed on the medium")?;
        if got == 0 {
            break;
        }
        let put = with_ext2_slot(dst, |fs| fs.ext2_write_file(dst_ino, &buf[..got], done))
            .and_then(|r| r.ok())
            .ok_or("write failed on the target - is the disk full?")?;
        if put != got {
            return Err("a short write on the target - the disk is full");
        }
        done += got as u64;
        // A megabyte at a time for a bar: fine enough to move visibly, coarse
        // enough to cost nothing
        progress::add_bytes(got as u64);
    }
    Ok(done)
}

/// A directory and everything under it
fn copy_tree(src: usize, dst: usize, path: &str) -> Result<u64, &'static str> {
    let Some(src_ino) = with_ext2_slot(src, |fs| fs.resolve_path(path).ok()).flatten() else {
        // Not an error: a machine with no GPU firmware has no /lib/firmware
        println!("    {} is not on this medium - skipped", path);
        return Ok(0);
    };
    let dst_ino = ensure_dir(dst, path)?;
    copy_dir(src, dst, src_ino, dst_ino, 0)
}

fn copy_dir(
    src: usize, dst: usize, src_ino: u32, dst_ino: u32, depth: u32,
) -> Result<u64, &'static str> {
    if depth >= 16 {
        return Err("the tree on the medium is deeper than this will follow");
    }

    // The listing is taken once and released, because copying a child takes
    // the same lock again
    let entries = with_ext2_slot(src, |fs| {
        let inode = fs.read_inode(src_ino).ok()?;
        let mut buf = [const { crate::fs::ext::structs::DirEntry::empty() }; 64];
        let n = fs.read_dir(&inode, &mut buf).ok()?;
        let mut out = alloc::vec::Vec::new();
        for e in buf.iter().take(n) {
            let name = e.name_str();
            if name == "." || name == ".." {
                continue;
            }
            let is_dir = fs.read_inode(e.inode).map(|i| i.is_directory()).unwrap_or(false);
            out.push((String::from(name), e.inode, is_dir));
        }
        Some(out)
    })
    .flatten()
    .ok_or("could not read a directory on the medium")?;

    let mut total = 0u64;
    for (name, ino, is_dir) in entries {
        if is_dir {
            let child = with_ext2_slot(dst, |fs| {
                match fs.ext2_lookup_in_dir(dst_ino, &name) {
                    Ok(Some(found)) => Some(found),
                    _ => fs.ext2_create_dir(dst_ino, &name, 0o755).ok(),
                }
            })
            .flatten()
            .ok_or("could not make a directory on the target")?;
            total += copy_dir(src, dst, ino, child, depth + 1)?;
        } else {
            total += copy_named(src, dst, ino, dst_ino, &name)?;
        }
    }
    Ok(total)
}

/// A file named by its inode on the source rather than by a path
fn copy_named(
    src: usize, dst: usize, src_ino: u32, dst_parent: u32, name: &str,
) -> Result<u64, &'static str> {
    let size = with_ext2_slot(src, |fs| fs.read_inode(src_ino).ok().map(|i| i.size()))
        .flatten()
        .ok_or("a file on the medium went missing")?;
    let dst_ino = with_ext2_slot(dst, |fs| fs.ext2_create_file(dst_parent, name, 0o755))
        .and_then(|r| r.ok())
        .ok_or("could not create a file on the target")?;

    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    while done < size {
        let want = ((size - done) as usize).min(CHUNK);
        let got = with_ext2_slot(src, |fs| {
            let inode = fs.read_inode(src_ino).ok()?;
            fs.read_file(&inode, done, &mut buf[..want]).ok()
        })
        .flatten()
        .ok_or("read failed on the medium")?;
        if got == 0 {
            break;
        }
        let put = with_ext2_slot(dst, |fs| fs.ext2_write_file(dst_ino, &buf[..got], done))
            .and_then(|r| r.ok())
            .ok_or("write failed on the target - is the disk full?")?;
        if put != got {
            return Err("a short write on the target - the disk is full");
        }
        done += got as u64;
        // A megabyte at a time for a bar: fine enough to move visibly, coarse
        // enough to cost nothing
        progress::add_bytes(got as u64);
    }
    Ok(done)
}

/// /boot: the kernel, and the two blobs kept as files as well as written raw,
/// so a disk can be made bootable again without the medium
fn copy_boot(src: usize, dst: usize) -> Result<u64, &'static str> {
    let boot = ensure_dir(dst, "/boot")?;
    let n = copy_file(src, dst, SRC_KERNEL, boot, "kernel.elf")?;
    print_success!("  copied {} ({} KB)", SRC_KERNEL, n / 1024);

    let pc = ensure_dir(dst, "/boot/grub/i386-pc")?;
    let a = copy_file(src, dst, SRC_MBR, pc, "mbr.bin")?;
    let b = copy_file(src, dst, SRC_CORE, pc, "core.bin")?;
    Ok(n + a + b)
}

/// Writes the boot menu.
///
/// The first two entries name the root by its label, so the disk boots wherever
/// it is plugged in. The third keeps the number, because a label only works
/// while unique
fn write_grub_cfg(dst: usize, drive: usize) -> Result<(), &'static str> {
    use core::fmt::Write;
    let mut cfg = String::new();
    let _ = write!(
        cfg,
        "set default=0\n\
         set timeout=5\n\
         \n\
         menuentry \"MikuOS\" {{\n\
         \x20   insmod all_video\n\
         \x20   set gfxpayload=keep\n\
         \x20   multiboot2 /boot/kernel.elf root=LABEL={label}\n\
         \x20   boot\n\
         }}\n\
         \n\
         menuentry \"MikuOS (writable root)\" {{\n\
         \x20   insmod all_video\n\
         \x20   set gfxpayload=keep\n\
         \x20   multiboot2 /boot/kernel.elf root=LABEL={label} rootrw\n\
         \x20   boot\n\
         }}\n\
         \n\
         menuentry \"MikuOS (root by number - drive {drive})\" {{\n\
         \x20   insmod all_video\n\
         \x20   set gfxpayload=keep\n\
         \x20   multiboot2 /boot/kernel.elf root={drive}:{lba}\n\
         \x20   boot\n\
         }}\n",
        label = ROOT_LABEL,
        drive = drive,
        lba = ROOT_LBA,
    );

    let grub = ensure_dir(dst, "/boot/grub")?;
    let ino = with_ext2_slot(dst, |fs| fs.ext2_create_file(grub, "grub.cfg", 0o644))
        .and_then(|r| r.ok())
        .ok_or("could not create grub.cfg")?;
    let n = with_ext2_slot(dst, |fs| fs.ext2_write_file(ino, cfg.as_bytes(), 0))
        .and_then(|r| r.ok())
        .ok_or("could not write grub.cfg")?;
    if n != cfg.len() {
        return Err("grub.cfg was written short");
    }
    Ok(())
}

/// The two writes that go to sectors rather than to files
fn write_bootloader(t: &Target, mbr: &[u8], core: &[u8]) -> Result<(), &'static str> {
    // The boot sector is read, not replaced: bytes 440 onwards are the
    // protective partition table that `gpt_init` has just written, and GRUB
    // has no business in them
    let mut sector = [0u8; 512];
    crate::io::block::read(t.dev, 0, 1, &mut sector)
        .map_err(|_| "could not read the boot sector back")?;
    sector[..440].copy_from_slice(mbr);
    crate::io::block::write_sync(t.dev, 0, 1, &sector)
        .map_err(|_| "could not write the boot sector")?;

    crate::io::block::write_sync(t.dev, CORE_LBA, (core.len() / 512) as u32, core)
        .map_err(|_| "could not write core.bin")?;
    Ok(())
}
