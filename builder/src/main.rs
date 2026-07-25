use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
    time::Duration,
};

fn ask_user(prompt: &str, timeout_secs: u64) -> bool {
    print!("{}", prompt);
    io::stdout().flush().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_ok() {
            let _ = tx.send(input.trim().to_lowercase());
        }
    });
    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(input) => input == "y" || input == "yes",
        Err(_) => { println!("Auto: N"); false }
    }
}

fn ask_mb(prompt: &str, default_mb: u32, timeout_secs: u64) -> u32 {
    print!("{}", prompt);
    io::stdout().flush().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_ok() {
            let _ = tx.send(input.trim().to_string());
        }
    });
    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(ref s) if s.is_empty() => { println!("Auto: {} MB", default_mb); default_mb }
        Ok(s) => s.parse::<u32>().unwrap_or_else(|_| {
            println!("Invalid, using {} MB", default_mb);
            default_mb
        }),
        Err(_) => { println!("Auto: {} MB", default_mb); default_mb }
    }
}

fn parse_meminfo(content: &str, field: &str) -> u64 {
    content.lines()
        .find(|l| l.starts_with(field))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
}

fn detect_qemu_ram() -> String {
    let content   = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let total_mb  = parse_meminfo(&content, "MemTotal:") / 1024;
    let free_mb   = parse_meminfo(&content, "MemFree:")  / 1024;
    let buffers   = parse_meminfo(&content, "Buffers:")  / 1024;
    let cached    = parse_meminfo(&content, "Cached:")   / 1024;
    let phys_free = free_mb + buffers + cached;
    let target_mb = ((phys_free as f64 * 0.8) as u64).min(total_mb).max(512);
    let ram = format!("{}M", target_mb);
    println!("[*] Host RAM: {} MB  Phys free: {} MB  → QEMU gets: {}", total_mb, phys_free, ram);
    ram
}

fn check_grub_mkrescue() {
    let ok = Command::new("grub-mkrescue")
        .arg("--version").output()
        .map(|o| o.status.success()).unwrap_or(false);
    if !ok { panic!("grub-mkrescue not found"); }
    println!("[ok] grub-mkrescue found");
}

fn build_kernel(root: &Path, low_ram: bool) {
    println!("\nBuilding kernel...");
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root)
        .arg("build")
        // The ISO used to ship a debug build. Unoptimized kernel code has far
        // fatter stack frames (a single 0.5 MiB VFS temporary was enough to
        // run off the 512 KiB boot stack), and every release image was slower
        // and ~6x larger than it needed to be
        .arg("--release")
        .arg("-p").arg("miku-os-release")
        .arg("--target").arg("x86_64-unknown-none")
        .arg("-Z").arg("build-std=core,compiler_builtins,alloc")
        .arg("-Z").arg("build-std-features=compiler-builtins-mem");

    // Firmware is no longer embedded into the kernel image; it is staged onto
    // the firmware.img store (see build_firmware_image) and read on demand.

    let mut rustflags =
        "-C relocation-model=static -C link-arg=-Tlinker.ld -C link-arg=--no-dynamic-linker"
            .to_string();
    if low_ram {
        cmd.arg("--jobs").arg("1");
        rustflags.push_str(" -C codegen-units=1");
    }
    cmd.env("RUSTFLAGS", &rustflags);

    if !cmd.status().expect("cargo build failed").success() {
        panic!("Kernel compilation failed");
    }
    println!("[ok] Kernel built");
}


/// Userspace programs (src/lib/userspace). These were never built by the
/// builder, so ring-3 binaries only ever existed if someone ran build.sh by
/// hand - which is why the syscall ABI had no real users. Binaries land in
/// /bin on the root filesystem.
fn build_userspace(root: &Path, low_ram: bool) -> Vec<(String, PathBuf)> {
    let us_dir = root.join("src/lib/userspace");
    if !us_dir.exists() {
        println!("[!] src/lib/userspace not found - skipping userspace programs");
        return Vec::new();
    }

    println!("\nBuilding userspace programs  (src/lib/userspace/)...");

    let rustflags = [
        "-C relocation-model=pic",
        "-C link-arg=-pie",
        "-C link-arg=-z",  "-C link-arg=noexecstack",
        "-C no-redzone=y",
    ].join(" ");

    let mut cmd = Command::new("cargo");
    cmd.current_dir(&us_dir)
        .env("RUSTFLAGS", &rustflags)
        .arg("+nightly")
        .arg("build")
        .arg("--release")
        .arg("--target").arg("x86_64-miku-app.json")
        .arg("-Z").arg("json-target-spec")
        .arg("-Z").arg("build-std=core")
        .arg("-Z").arg("build-std-features=compiler-builtins-mem");
    for b in USERSPACE_BINS {
        cmd.arg("--bin").arg(b);
    }
    if low_ram { cmd.arg("--jobs").arg("1"); }

    if !cmd.status().expect("cargo build userspace failed").success() {
        println!("[!] userspace build failed - continuing without ring-3 programs");
        return Vec::new();
    }

    let bin_dir = us_dir.join("target/x86_64-miku-app/release");
    let mut out = Vec::new();
    for b in USERSPACE_BINS {
        let p = bin_dir.join(b);
        if p.exists() {
            println!("[ok] {} ({} KB)", b, fs::metadata(&p).unwrap().len() / 1024);
            out.push((b.to_string(), p));
        } else {
            println!("[!] {} not produced", b);
        }
    }
    out
}

/// Ring-3 programs staged into /bin. msh is the userspace shell; the rest are
/// ABI smoke tests that are handy to have on the image.
const USERSPACE_BINS: &[&str] = &["msh", "hello"];

fn build_ldmiku(root: &Path, low_ram: bool) {
    let ldmiku_dir = root.join("ld-miku");
    if !ldmiku_dir.exists() {
        panic!("[!] ld-miku/ not found at {} - run builder from miku-os/builder/", ldmiku_dir.display());
    }

    println!("\nBuilding ld-miku.so  (src/lib/ld_miku/)...");

    let ld_script = ldmiku_dir.join("ld_link.ld");

    let rustflags = [
        "-C relocation-model=pic",
        "-C link-arg=-pie",
        "-C link-arg=-z",  "-C link-arg=noexecstack",
        "-C link-arg=-z",  "-C link-arg=now",
        "-C link-arg=--no-dynamic-linker",
        &format!("-C link-arg=-T{}", ld_script.display()),
        "-C no-redzone=y",
    ].join(" ");

    let mut cmd = Command::new("cargo");
        cmd.current_dir(&ldmiku_dir)
            .env("RUSTFLAGS", &rustflags)
            .arg("+nightly")
            .arg("build")
            .arg("--release")
            .arg("--target").arg("x86_64-miku-ldso.json")
            .arg("-Z").arg("json-target-spec")
            .arg("-Z").arg("build-std=core")
            .arg("-Z").arg("build-std-features=compiler-builtins-mem");

    if low_ram { cmd.arg("--jobs").arg("1"); }

    if !cmd.status().expect("cargo build ld-miku failed").success() {
        panic!("ld-miku compilation failed");
    }
    println!("[ok] ld-miku.so built");

    let bin_src = root.join("target/x86_64-miku-ldso/release/ld-miku");
    let bin_dst = root.join("src/lib/ld_miku/ld-miku.bin");
    if !bin_src.exists() {
        panic!("ld-miku binary not found at {}", bin_src.display());
    }
    fs::copy(&bin_src, &bin_dst)
        .unwrap_or_else(|e| panic!("Cannot copy ld-miku.bin: {}", e));
    println!("[ok] ld-miku.bin → src/lib/ld_miku/ld-miku.bin ({} KB)",
        fs::metadata(&bin_dst).unwrap().len() / 1024);
}

/// Library names come from the shared manifest src/lib/mikulibs/libs.list
/// (also consumed by mikulibs/build.rs, userspace/build.rs and the kernel
/// root build.rs, which generates the ldso preload table from it).
fn miku_libs(root: &Path) -> Vec<String> {
    let list = root.join("src/lib/mikulibs/libs.list");
    let src = fs::read_to_string(&list)
        .unwrap_or_else(|e| panic!("cannot read {}: {}", list.display(), e));
    src.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split(':').next().unwrap().trim().to_string())
        .collect()
}

fn build_mikulibs(root: &Path, low_ram: bool) {
    let mikulibs_dir = root.join("mikulibs");
    if !mikulibs_dir.exists() {
        panic!("[!] mikulibs/ not found at {} - run builder from miku-os/builder/", mikulibs_dir.display());
    }

    println!("\nBuilding miku libraries  (src/lib/mikulibs/)...");

    let rustflags = [
        "-C relocation-model=pic",
        "-C link-arg=-pie",
        "-C link-arg=-z",  "-C link-arg=noexecstack",
        "-C link-arg=-z",  "-C link-arg=now",
        "-C link-arg=--no-dynamic-linker",
        "-C link-arg=--hash-style=both",
        "-C no-redzone=y",
    ].join(" ");

    let mut cmd = Command::new("cargo");
    cmd.current_dir(&mikulibs_dir)
        .env("RUSTFLAGS", &rustflags)
        .arg("+nightly")
        .arg("build")
        .arg("--release")
        .arg("--target").arg("x86_64-miku-lib.json")
        .arg("-Z").arg("json-target-spec")
        .arg("-Z").arg("build-std=core")
        .arg("-Z").arg("build-std-features=compiler-builtins-mem");

    if low_ram { cmd.arg("--jobs").arg("1"); }

    if !cmd.status().expect("cargo build mikulibs failed").success() {
        panic!("mikulibs compilation failed");
    }
    println!("[ok] miku libraries built");

    let libs_dir = root.join("src/lib/mikulibs/libs");
    fs::create_dir_all(&libs_dir)
        .unwrap_or_else(|e| panic!("Cannot create {}: {}", libs_dir.display(), e));
    for lib in miku_libs(root) {
        let bin_src = mikulibs_dir.join(format!("target/x86_64-miku-lib/release/{}", lib));
        let bin_dst = libs_dir.join(format!("{}.so", lib));
        if !bin_src.exists() {
            panic!("{} binary not found at {}", lib, bin_src.display());
        }
        fs::copy(&bin_src, &bin_dst)
            .unwrap_or_else(|e| panic!("Cannot copy {}.so: {}", lib, e));
        println!("[ok] {}.so → src/lib/mikulibs/libs/{}.so ({} KB)",
            lib, lib, fs::metadata(&bin_dst).unwrap().len() / 1024);
    }
}

fn create_iso(root: &Path) {
    let out_dir  = root.join("miku-os");
    fs::create_dir_all(&out_dir).unwrap();

    let iso_root = root.join("iso_root");
    if iso_root.exists() { fs::remove_dir_all(&iso_root).unwrap(); }
    fs::create_dir_all(iso_root.join("boot/grub")).unwrap();

    let kernel_src = root.join("target/x86_64-unknown-none/release/miku-os-release");
    let kernel_dst = iso_root.join("boot/kernel.elf");
    fs::copy(&kernel_src, &kernel_dst)
        .unwrap_or_else(|e| panic!("Cannot copy kernel: {}", e));

    // Stage /lib/firmware into the ISO as a GRUB module so the boot medium
    // carries firmware on its own (no second disk needed on real hardware).
    let have_fw = build_firmware_module(root, &iso_root);

    let grub_cfg_src = root.join("grub.cfg");
    let grub_cfg_dst = iso_root.join("boot/grub/grub.cfg");
    let cfg = fs::read_to_string(&grub_cfg_src)
        .unwrap_or_else(|e| panic!("Cannot read grub.cfg: {}", e));
    let mut new_cfg = String::from("set timeout=-1\n");
    for line in cfg.lines() {
        let t = line.trim();
        if t.starts_with("set timeout=") || t.starts_with("timeout=") {
            continue;
        }
        new_cfg.push_str(line);
        new_cfg.push('\n');
        // Load the firmware image right after the kernel so GRUB hands it to
        // the kernel as a multiboot2 module tagged "firmware".
        if have_fw && t.starts_with("multiboot2") {
            let indent = &line[..line.len() - line.trim_start().len()];
            new_cfg.push_str(indent);
            new_cfg.push_str("module2 /boot/firmware.img firmware\n");
        }
    }
    fs::write(&grub_cfg_dst, new_cfg)
        .unwrap_or_else(|e| panic!("Cannot write grub.cfg: {}", e));

    let iso_path = out_dir.join("miku-os.iso");
    println!("\nCreating ISO: {}", iso_path.display());
    let status = Command::new("grub-mkrescue")
        .args(["-o", iso_path.to_str().unwrap(), iso_root.to_str().unwrap()])
        .status().expect("grub-mkrescue failed");
    if !status.success() { panic!("grub-mkrescue failed"); }

    println!("[ok] ISO: {}  ({} KB)",
        iso_path.display(),
        fs::metadata(&iso_path).unwrap().len() / 1024);
    fs::remove_dir_all(&iso_root).ok();
}

fn check_mke2fs() -> bool {
    let ok = Command::new("mke2fs")
        .arg("-V").output()
        .map(|o| o.status.success()).unwrap_or(false);
    if ok {
        println!("[ok] mke2fs found");
    } else {
        println!("[!] mke2fs not found - disk.img /lib/firmware will NOT be provisioned (GPU firmware unavailable)");
    }
    ok
}

/// Recursively copy a directory tree, creating destination dirs as needed
fn copy_dir_recursive(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap_or_else(|e| panic!("mkdir {}: {}", dst.display(), e));
    for entry in fs::read_dir(src).unwrap_or_else(|e| panic!("read_dir {}: {}", src.display(), e)) {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to);
        } else {
            fs::copy(&from, &to).unwrap_or_else(|e| panic!("copy {}: {}", from.display(), e));
        }
    }
}

/// Total bytes of every regular file under dir
fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(rd) = fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                total += dir_size(&p);
            } else if let Ok(m) = entry.metadata() {
                total += m.len();
            }
        }
    }
    total
}

/// NVIDIA firmware home: kernel/drivers/gpu/nvidia/firmware/<card>/, one folder per card.
/// Each folder is staged to /lib/firmware/<chip path> (the path the kernel's
/// fwload requests, same layout as linux-firmware). A folder not in this
/// table is staged verbatim to /lib/firmware/nvidia/<folder name>.
const NVIDIA_FW_CARDS: &[(&str, &str)] = &[
    ("gtx1650",   "nvidia/tu116"),   // GTX 1650 / 1660 (TU116/TU117)
    ("rtx5060ti", "nvidia/gb206"),   // RTX 5060 Ti / 5060 (GB206)
];

/// Stage a Linux /lib/firmware-style tree from
/// kernel/drivers/gpu/nvidia/firmware/<card>/ folders. Returns the staging
/// root holding lib/firmware/...
/// Stage everything that belongs on the root image: NVIDIA firmware (when it
/// is present) and the ring-3 programs.
///
/// Returns the staging root and whether any firmware was found. Missing
/// firmware is the normal case for a fresh clone - the blobs are NVIDIA's
/// proprietary microcode and are not kept in git - so it is reported as
/// information, never as a warning, and never fails the build.
fn stage_root_tree(root: &Path) -> Option<(PathBuf, bool)> {
    let fw_home = root.join("kernel/drivers/gpu/nvidia/firmware");
    let staging = root.join("target/fw_root");
    if staging.exists() {
        fs::remove_dir_all(&staging).ok();
    }
    let mut staged_any = false;

    let mut cards: Vec<_> = match fs::read_dir(&fw_home) {
        Ok(rd) => rd.flatten().filter(|e| e.path().is_dir()).collect(),
        Err(_) => Vec::new(),
    };
    cards.sort_by_key(|e| e.file_name());
    let mut have_firmware = false;
    for card in cards {
        let name = card.file_name().into_string().unwrap();
        let dest_rel = NVIDIA_FW_CARDS.iter()
            .find(|(folder, _)| *folder == name)
            .map(|(_, chip)| (*chip).to_string())
            .unwrap_or_else(|| format!("nvidia/{}", name));
        let kb = dir_size(&card.path()) / 1024;
        if kb == 0 { continue; }
        copy_dir_recursive(&card.path(), &staging.join("lib/firmware").join(&dest_rel));
        println!("[ok] firmware {} -> /lib/firmware/{} ({} KB)", name, dest_rel, kb);
        staged_any = true;
        have_firmware = true;
    }

    // /bin: ring-3 programs. Staged into the same tree so both the fresh
    // mke2fs path and the debugfs refresh path pick them up
    let apps = root.join("src/lib/userspace/target/x86_64-miku-app/release");
    if apps.is_dir() {
        for b in USERSPACE_BINS {
            let src = apps.join(b);
            if !src.exists() { continue; }
            let dst_dir = staging.join("bin");
            fs::create_dir_all(&dst_dir).ok();
            if fs::copy(&src, dst_dir.join(b)).is_ok() {
                println!("[ok] {} -> /bin/{}", b, b);
                staged_any = true;
            }
        }
    }

    if !have_firmware {
        // Both the ISO module and the root disk stage from here, so without
        // this the same notice would print twice per build
        static NOTICE: std::sync::Once = std::sync::Once::new();
        NOTICE.call_once(|| {
            println!("[i] no NVIDIA firmware in {} - image will boot without GPU firmware",
                fw_home.display());
            println!("[i]   see {}/README.md to add it", fw_home.display());
        });
    }

    if staged_any { Some((staging, have_firmware)) } else { None }
}

/// Build a compact ext2 image holding /lib/firmware and drop it into the ISO
/// tree as a GRUB multiboot2 module. The kernel exposes the module's RAM span
/// as a block device and mounts /lib/firmware from it (see fwload::init), so a
/// single ISO/USB carries kernel + firmware with no second disk and no
/// QEMU-specific drive layout. Returns true if the module image was created.
fn build_firmware_module(root: &Path, iso_root: &Path) -> bool {
    let (staging, have_firmware) = match stage_root_tree(root) {
        Some(v) => v,
        None => return false,
    };
    // No blobs on this machine: the ISO simply ships without the module and
    // the kernel reports "firmware unavailable". Nothing to warn about
    if !have_firmware {
        return false;
    }
    if !check_mke2fs() {
        println!("[!] mke2fs missing - ISO will NOT carry firmware (GPU firmware unavailable)");
        return false;
    }
    let staged_bytes = dir_size(&staging);
    // staged payload + 30% slack + 8 MiB for ext metadata/inodes, >= 40 MiB
    let size_mb = (((staged_bytes + staged_bytes / 3) / (1024 * 1024)) + 8).max(40) as u32;

    let img = iso_root.join("boot/firmware.img");
    let ok = Command::new("dd")
        .args(["if=/dev/zero",
               &format!("of={}", img.display()),
               "bs=1M", &format!("count={}", size_mb)])
        .status().expect("dd failed").success();
    if !ok { panic!("dd failed for firmware.img"); }

    let ok = Command::new("mke2fs")
        .args([
            "-q", "-F",
            "-t", "ext2",
            "-O", "^resize_inode,^dir_index,^has_journal",
            "-d", staging.to_str().unwrap(),
            img.to_str().unwrap(),
        ])
        .status().expect("mke2fs failed").success();
    if !ok { panic!("mke2fs failed to format firmware.img"); }

    println!("[ok] firmware module: boot/firmware.img ({} MB ext2, {} KB staged)",
        size_mb, staged_bytes / 1024);
    true
}

fn provision_root_disk(root: &Path, disk_path: &Path, size_mb: u32) {
    // Staging carries /bin even when there is no firmware, so the disk is
    // still worth creating - that is where the ring-3 shell comes from
    let (staging, have_firmware) = match stage_root_tree(root) {
        Some(v) => v,
        None => {
            println!("[i] nothing to stage onto disk.img (no firmware, no userspace programs)");
            return;
        }
    };
    if !check_mke2fs() {
        return;
    }
    let staged_kb = dir_size(&staging) / 1024;
    let what = if have_firmware { "/lib/firmware + /bin" } else { "/bin" };

    if !disk_path.exists() {
        println!("\nCreating root disk disk.img ({} MB) with {}...", size_mb, what);
        let ok = Command::new("dd")
            .args(["if=/dev/zero",
                   &format!("of={}", disk_path.display()),
                   "bs=1M", &format!("count={}", size_mb)])
            .status().expect("dd failed").success();
        if !ok { panic!("dd failed for disk.img"); }

        let ok = Command::new("mke2fs")
            .args([
                "-q", "-F",
                "-t", "ext2",
                "-O", "^resize_inode,^dir_index,^has_journal",
                "-d", staging.to_str().unwrap(),
                disk_path.to_str().unwrap(),
            ])
            .status().expect("mke2fs failed").success();
        if !ok { panic!("mke2fs failed to format disk.img"); }
        println!("[ok] disk.img formatted ext2 + firmware staged ({} KB)", staged_kb);
    } else {
        // Only push firmware when the image does not already carry it
        let skip_firmware = !have_firmware || image_has_path(disk_path, "/lib/firmware");
        let what = if skip_firmware { "/bin" } else { "/lib/firmware + /bin" };
        println!("\nRefreshing {} on existing disk.img...", what);
        if inject_firmware_debugfs(disk_path, &staging, skip_firmware) {
            println!("[ok] {} refreshed on disk.img", what);
        } else {
            println!("[i] disk.img not writable by debugfs (MikuOS-formatted ext?); \
                      leaving it as is");
        }
    }
}

/// Refresh the staged tree into an existing ext image via a debugfs script:
/// mkdir every directory (pre-order, parents first), then for each file rm any
/// stale copy and write the fresh one. debugfs continues past "already exists"
/// / "not found" errors, so the script is idempotent

/// True if `path` already exists inside the ext image. Used to avoid
/// rewriting tens of megabytes of firmware on every single build
fn image_has_path(img: &Path, path: &str) -> bool {
    let out = Command::new("debugfs")
        .args(["-R", &format!("stat {}", path), img.to_str().unwrap()])
        .output();
    match out {
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            let txt = String::from_utf8_lossy(&o.stdout);
            !err.contains("File not found")
                && !txt.contains("File not found")
                && !err.contains("Filesystem not open")
        }
        Err(_) => false,
    }
}

fn inject_firmware_debugfs(img: &Path, staging: &Path, skip_firmware: bool) -> bool {
    if Command::new("debugfs").arg("-V").output().map(|o| o.status.success()).unwrap_or(false) == false {
        println!("[i] debugfs not found - leaving disk.img contents as they are");
        return false;
    }
    // debugfs exits 0 even when it cannot open the filesystem, so probe first
    let probe = Command::new("debugfs")
        .args(["-R", "stats -h", img.to_str().unwrap()])
        .output().expect("debugfs probe failed");
    let probe_err = String::from_utf8_lossy(&probe.stderr);
    if probe_err.contains("Filesystem not open") || probe_err.contains("Couldn't find valid filesystem") {
        return false;
    }
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_tree(staging, "", &mut dirs, &mut files);

    // Firmware is written once, when the image does not have it yet. Pushing
    // ~90 MB of unchanged blobs through debugfs on every build is slow and
    // pointless, and it means an ordinary `cargo run` rewrites vendor
    // microcode onto the disk for no reason
    if skip_firmware {
        dirs.retain(|d| !d.starts_with("lib/firmware"));
        files.retain(|(rel, _)| !rel.starts_with("lib/firmware"));
    }
    if files.is_empty() {
        return true;
    }

    let mut script = String::new();
    for d in &dirs {
        script.push_str(&format!("mkdir /{}\n", d));
    }
    for (rel, host) in &files {
        script.push_str(&format!("rm /{}\n", rel));
        script.push_str(&format!("write {} /{}\n", host.display(), rel));
    }

    let script_path = staging.parent().unwrap().join("fw_debugfs.script");
    fs::write(&script_path, &script).expect("write debugfs script");

    let ok = Command::new("debugfs")
        .args(["-w", "-f", script_path.to_str().unwrap(), img.to_str().unwrap()])
        .status().expect("debugfs failed").success();
    if !ok { panic!("debugfs failed to inject firmware into disk.img"); }
    true
}

/// Pre-order walk: relative dir paths (parents before children) and
/// (relative-file-path, absolute-host-path) pairs
fn collect_tree(dir: &Path, prefix: &str, dirs: &mut Vec<String>, files: &mut Vec<(String, PathBuf)>) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().into_string().unwrap();
        let rel = if prefix.is_empty() { name } else { format!("{}/{}", prefix, name) };
        if e.path().is_dir() {
            dirs.push(rel.clone());
            collect_tree(&e.path(), &rel, dirs, files);
        } else {
            files.push((rel, e.path()));
        }
    }
}

fn ensure_disk(path: &Path, size_mb: u32, label: &str) {
    if path.exists() {
        println!("[ok] {} exists ({} MB)", label,
            fs::metadata(path).unwrap().len() / (1024 * 1024));
        return;
    }
    println!("[*] Creating {} disk: {} MB", label, size_mb);
    let ok = Command::new("dd")
        .args(["if=/dev/zero",
               &format!("of={}", path.display()),
               "bs=1M",
               &format!("count={}", size_mb)])
        .status().expect("dd failed").success();
    if !ok { panic!("dd failed for {}", label); }
    println!("[ok] {} disk created", label);
}

struct DiskConfig { main_mb: u32, data_mb: u32 }

impl DiskConfig {
    fn ask(root: &Path) -> Self {
        let main_exists = root.join("miku-os/disk.img").exists();
        let data_exists = root.join("miku-os/data.img").exists();

        if main_exists && data_exists {
            return Self {
                main_mb: (fs::metadata(root.join("miku-os/disk.img")).unwrap().len() / (1024*1024)) as u32,
                data_mb: (fs::metadata(root.join("miku-os/data.img")).unwrap().len() / (1024*1024)) as u32,
            };
        }

        println!("\nDisk Setup");
        println!("  disk.img → drive 1 (ext4 root)");
        println!("  data.img → drive 2 (extra, optional)");

        let main_mb = if main_exists {
            (fs::metadata(root.join("miku-os/disk.img")).unwrap().len() / (1024*1024)) as u32
        } else {
            ask_mb("  disk.img size in MB (default 4096): ", 4096, 30)
        };

        let data_mb = if ask_user("  Create data.img? [y/N]: ", 15) && !data_exists {
            ask_mb("  data.img size in MB (default 2048): ", 2048, 30)
        } else if data_exists {
            (fs::metadata(root.join("miku-os/data.img")).unwrap().len() / (1024*1024)) as u32
        } else { 0 };

        Self { main_mb, data_mb }
    }
}

fn main() {
    println!("MikuOS Builder\n");

    let root = std::env::current_exe()
        .expect("cannot locate builder binary")
        .ancestors()
        .nth(4)
        .expect("unexpected binary location")
        .to_path_buf();
    let low_ram = ask_user("Low RAM mode? [y/N]: ", 10);

    check_grub_mkrescue();
    build_ldmiku(&root, low_ram);
    build_mikulibs(&root, low_ram);
    build_userspace(&root, low_ram);
    build_kernel(&root, low_ram);
    create_iso(&root);

    let cfg       = DiskConfig::ask(&root);
    let disk_path = root.join("miku-os/disk.img");
    let data_path = root.join("miku-os/data.img");

    // disk.img is the persistent root: format + carry /lib/firmware (Linux way)
    provision_root_disk(&root, &disk_path, cfg.main_mb);
    if cfg.data_mb > 0 { ensure_disk(&data_path, cfg.data_mb, "data"); }

    if !ask_user("\nLaunch QEMU? [y/N]: ", 10) { return; }

    let ram      = detect_qemu_ram();
    let iso_path = root.join("miku-os/miku-os.iso");

    // disk.img -> drive 1 (primary slave). The kernel mounts it and grafts its
    // /lib/firmware onto the VFS for on-demand firmware loading
    let mut args: Vec<String> = vec![
        "-boot".into(), "d".into(),
        "-cdrom".into(), iso_path.to_str().unwrap().into(),
        "-drive".into(),
        format!("file={},format=raw,if=none,id=disk0,cache=unsafe,aio=threads",
            disk_path.display()),
        "-device".into(), "ide-hd,drive=disk0,bus=ide.0,unit=1,rotation_rate=1".into(),
        "-serial".into(), "stdio".into(),
        "-display".into(), "gtk".into(),
        "-m".into(), ram,
        // xHCI controller + USB keyboard: exercises the native USB HID
        // stack (the PS/2 path still works through QEMU's emulated i8042)
        "-device".into(), "qemu-xhci,id=xhci".into(),
        "-device".into(), "usb-kbd,bus=xhci.0".into(),
    ];

    if cfg.data_mb > 0 && data_path.exists() {
        args.push("-drive".into());
        args.push(format!("file={},format=raw,if=none,id=disk1,cache=unsafe,aio=threads",
            data_path.display()));
        args.push("-device".into());
        args.push("ide-hd,drive=disk1,bus=ide.1,unit=1,rotation_rate=1".into());
        println!("[*] data.img attached as drive 2");
    }

    // '-enable-kvm -version' succeeds even without KVM (QEMU does not init
    // the accelerator for -version), so probe /dev/kvm itself: it exists and
    // is writable only when the kvm module is loaded and we have access
    let kvm_ok = fs::OpenOptions::new()
        .read(true).write(true)
        .open("/dev/kvm")
        .is_ok();
    if kvm_ok {
        args.push("-enable-kvm".into());
    } else {
        println!("[!] /dev/kvm unavailable (module not loaded, no BIOS SVM/VT-x, or no permissions)");
        println!("    falling back to TCG software emulation - expect it to be slow");
    }

    println!("\n  drive 1 → disk.img ({} MB)", cfg.main_mb);
    if cfg.data_mb > 0 { println!("  drive 2 → data.img ({} MB)", cfg.data_mb); }

    println!("Starting QEMU...");
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    Command::new("qemu-system-x86_64")
        .args(&refs)
        .spawn().expect("QEMU failed")
        .wait().unwrap();
}
