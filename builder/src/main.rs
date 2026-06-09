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

fn build_ldmiku(root: &Path, low_ram: bool) {
    let ldmiku_dir = root.join("ld-miku");
    if !ldmiku_dir.exists() {
        panic!("[!] ld-miku/ not found at {} — run builder from miku-os/builder/", ldmiku_dir.display());
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

fn build_libmiku(root: &Path, low_ram: bool) {
    let libmiku_dir = root.join("libmiku");
    if !libmiku_dir.exists() {
        panic!("[!] libmiku/ not found at {} - run builder from miku-os/builder/", libmiku_dir.display());
    }

    println!("\nBuilding libmiku.so  (src/lib/libmiku/)...");

    let rustflags = [
        "-C relocation-model=pic",
        "-C link-arg=-pie",
        "-C link-arg=-z",  "-C link-arg=noexecstack",
        "-C link-arg=-z",  "-C link-arg=now",
        "-C link-arg=--no-dynamic-linker",
        "-C link-arg=--export-dynamic",
        "-C link-arg=--hash-style=both",
        "-C no-redzone=y",
    ].join(" ");

    let mut cmd = Command::new("cargo");
    cmd.current_dir(&libmiku_dir)
        .env("RUSTFLAGS", &rustflags)
        .arg("+nightly")
        .arg("build")
        .arg("--release")
        .arg("--target").arg("x86_64-miku-lib.json")
        .arg("-Z").arg("json-target-spec")
        .arg("-Z").arg("build-std=core")
        .arg("-Z").arg("build-std-features=compiler-builtins-mem");

    if low_ram { cmd.arg("--jobs").arg("1"); }

    if !cmd.status().expect("cargo build libmiku failed").success() {
        panic!("libmiku compilation failed");
    }
    println!("[ok] libmiku.so built");

    let bin_src = libmiku_dir.join("target/x86_64-miku-lib/release/libmiku");
    let bin_dst = root.join("src/lib/libmiku/libmiku.so");
    if !bin_src.exists() {
        panic!("libmiku binary not found at {}", bin_src.display());
    }
    fs::copy(&bin_src, &bin_dst)
        .unwrap_or_else(|e| panic!("Cannot copy libmiku.so: {}", e));
    println!("[ok] libmiku.so → src/lib/libmiku/libmiku.so ({} KB)",
        fs::metadata(&bin_dst).unwrap().len() / 1024);
}

fn create_iso(root: &Path) {
    let out_dir  = root.join("miku-os");
    fs::create_dir_all(&out_dir).unwrap();

    let iso_root = root.join("iso_root");
    if iso_root.exists() { fs::remove_dir_all(&iso_root).unwrap(); }
    fs::create_dir_all(iso_root.join("boot/grub")).unwrap();

    let kernel_src = root.join("target/x86_64-unknown-none/debug/miku-os-release");
    let kernel_dst = iso_root.join("boot/kernel.elf");
    fs::copy(&kernel_src, &kernel_dst)
        .unwrap_or_else(|e| panic!("Cannot copy kernel: {}", e));

    let grub_cfg_src = root.join("grub.cfg");
    let grub_cfg_dst = iso_root.join("boot/grub/grub.cfg");
    let cfg = fs::read_to_string(&grub_cfg_src)
        .unwrap_or_else(|e| panic!("Cannot read grub.cfg: {}", e));
    let mut new_cfg = String::from("set timeout=-1\n");
    for line in cfg.lines() {
        let t = line.trim();
        if !t.starts_with("set timeout=") && !t.starts_with("timeout=") {
            new_cfg.push_str(line);
            new_cfg.push('\n');
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

/// Stage src/nvidia/gtx1650/tu116/ into a Linux /lib/firmware-style tree
/// Returns the staging root holding lib/firmware/nvidia/tu116/...
fn stage_firmware_tree(root: &Path) -> Option<PathBuf> {
    let src_tu116 = root.join("src/nvidia/gtx1650/tu116");
    if !src_tu116.exists() {
        println!("[!] {} not found - skipping firmware staging", src_tu116.display());
        return None;
    }
    let staging = root.join("target/fw_root");
    if staging.exists() {
        fs::remove_dir_all(&staging).ok();
    }
    let dst = staging.join("lib/firmware/nvidia/tu116");
    copy_dir_recursive(&src_tu116, &dst);
    Some(staging)
}

fn provision_root_disk(root: &Path, disk_path: &Path, size_mb: u32) {
    let staging = match stage_firmware_tree(root) {
        Some(s) => s,
        None => return,
    };
    if !check_mke2fs() {
        return;
    }
    let staged_kb = dir_size(&staging) / 1024;

    if !disk_path.exists() {
        println!("\nCreating root disk disk.img ({} MB) with /lib/firmware...", size_mb);
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
        println!("\nRefreshing /lib/firmware on existing disk.img...");
        inject_firmware_debugfs(disk_path, &staging);
        println!("[ok] /lib/firmware refreshed on disk.img ({} KB)", staged_kb);
    }
}

/// Refresh the staged tree into an existing ext image via a debugfs script:
/// mkdir every directory (pre-order, parents first), then for each file rm any
/// stale copy and write the fresh one. debugfs continues past "already exists"
/// / "not found" errors, so the script is idempotent
fn inject_firmware_debugfs(img: &Path, staging: &Path) {
    if Command::new("debugfs").arg("-V").output().map(|o| o.status.success()).unwrap_or(false) == false {
        println!("[!] debugfs not found - cannot refresh firmware on existing disk.img");
        return;
    }
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_tree(staging, "", &mut dirs, &mut files);

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
    build_libmiku(&root, low_ram);
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
    ];

    if cfg.data_mb > 0 && data_path.exists() {
        args.push("-drive".into());
        args.push(format!("file={},format=raw,if=none,id=disk1,cache=unsafe,aio=threads",
            data_path.display()));
        args.push("-device".into());
        args.push("ide-hd,drive=disk1,bus=ide.1,unit=1,rotation_rate=1".into());
        println!("[*] data.img attached as drive 2");
    }

    if Command::new("qemu-system-x86_64")
        .args(["-enable-kvm", "-version"]).output()
        .map(|o| o.status.success()).unwrap_or(false)
    {
        args.push("-enable-kvm".into());
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
