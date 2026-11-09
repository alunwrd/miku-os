// How the image is made out of packages, and what the packages are
mod pkg;
// Programs built against MikuOS's Rust `std`
mod stdport;

use std::{
    env,
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
        // Release build: debug kernel code has much larger stack frames
        .arg("--release")
        .arg("-p").arg("miku-os-release")
        .arg("--target").arg("x86_64-unknown-none")
        .arg("-Z").arg("build-std=core,compiler_builtins,alloc")
        .arg("-Z").arg("build-std-features=compiler-builtins-mem");

    // Firmware is staged to firmware.img (`build_firmware_image`) and read on
    // demand

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


/// Ring-3 program crates. `system/` is the OS itself, `apps/` what runs on it.
/// Both install to /bin
const PROGRAM_CRATES: &[(&str, &[(&str, &str)], &str)] = &[
    ("system", SYSTEM_BINS, "system programs"),
    ("utils",  UTIL_BINS,   "commands"),
    ("apps",   APP_BINS,    "applications"),
];

/// Whether `stdapps` was requested. Also read while staging, so stale binaries
/// are not shipped
static WITH_STDAPPS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn std_crate_wanted(dir: &str) -> bool {
    dir != STD_CRATE || WITH_STDAPPS.load(std::sync::atomic::Ordering::Relaxed)
}

fn build_userspace(root: &Path, low_ram: bool) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for (dir, bins, what) in PROGRAM_CRATES {
        out.extend(build_program_crate(root, dir, bins, what, low_ram));
    }

    // One overlay for every `std` crate of the image. Crate sources are
    // refetched if cargo cleaned them
    let wanted: Vec<&str> = STD_CRATES.iter().map(|(d, _, _)| *d).filter(|d| std_crate_wanted(d)).collect();
    if !std_crate_wanted(STD_CRATE) {
        println!("\n[i] {STD_CRATE}/ not built (answered N) - the image ships without its programs");
    }
    println!("\nAssembling the std overlay  ({})...", wanted.join(", "));
    let overlay = match stdport::prepare(root, &wanted, true) {
        Ok(o) => o,
        Err(e) => {
            // Without it nothing against `std` builds, and aosh is required
            panic!("the std overlay could not be assembled: {e}");
        }
    };
    for (dir, bins, required) in STD_CRATES {
        if !std_crate_wanted(dir) { continue; }
        out.extend(build_std_crate(root, dir, bins, *required, low_ram, &overlay));
    }
    out
}

/// Builds a crate against Rust's `std`: target `x86_64-unknown-miku`,
/// `-Z build-std=std,panic_abort`, and a patched `library/` from
/// `lib/sdk/rust-std/` (`stdport`). Missing tools are fatal only for a `required`
/// crate
fn build_std_crate(root: &Path, dir: &str, bins: &[(&str, &str)], required: bool, low_ram: bool,
                   overlay: &Path)
    -> Vec<(String, PathBuf)>
{
    let crate_dir = root.join(dir);
    if !crate_dir.exists() {
        if required {
            panic!("{dir}/ is not there - the image would have no {}", bins.iter().map(|b| b.0).collect::<Vec<_>>().join(", "));
        }
        return Vec::new();
    }
    println!("\nBuilding std programs  ({dir}/)...");

    let args: Vec<String> = if low_ram { vec!["--jobs".into(), "1".into()] } else { Vec::new() };
    match stdport::build(root, &crate_dir, overlay, false, args) {
        Ok(()) => {}
        Err(e) if required => panic!("{dir}/ failed to build ({e}) - the image would ship without it"),
        Err(e) => {
            println!("[!] {dir}/ failed to build ({e}) - the image ships without its programs");
            return Vec::new();
        }
    }

    let bin_dir = crate_dir.join("target/x86_64-unknown-miku/release");
    let mut out = Vec::new();
    for (b, _) in bins {
        let p = bin_dir.join(b);
        if p.exists() {
            println!("[ok] {} ({} KB)", b, fs::metadata(&p).unwrap().len() / 1024);
            out.push((b.to_string(), p));
        } else if required {
            panic!("{b} was not produced, and the image cannot do without it");
        } else {
            println!("[!] {} not produced", b);
        }
    }
    out
}

fn build_program_crate(root: &Path, dir: &str, bins: &[(&str, &str)], what: &str, low_ram: bool)
    -> Vec<(String, PathBuf)>
{
    let crate_dir = root.join(dir);
    if !crate_dir.exists() {
        println!("[!] {dir}/ not found - skipping {what}");
        return Vec::new();
    }

    println!("\nBuilding {what}  ({dir}/)...");

    let rustflags = [
        "-C relocation-model=pic",
        "-C link-arg=-pie",
        "-C link-arg=-z",  "-C link-arg=noexecstack",
        "-C no-redzone=y",
        // Frame pointers, used by the kernel for crash report backtraces
        "-C force-frame-pointers=yes",
        // Build id for crash reports and core dumps (20 bytes in a PT_NOTE)
        "-C link-arg=--build-id=sha1",
    ].join(" ");

    let mut cmd = Command::new("cargo");
    // The target spec and linker script are shared with the other program crate
    cmd.current_dir(&crate_dir)
        .env("RUSTFLAGS", &rustflags)
        .arg("+nightly")
        .arg("build")
        .arg("--release")
        .arg("--target").arg("../lib/sdk/miku/x86_64-miku-app.json")
        .arg("-Z").arg("json-target-spec")
        // `alloc` as well as `core`: the `miku` crate declares a global
        // allocator over core_miku's heap
        .arg("-Z").arg("build-std=core,alloc")
        .arg("-Z").arg("build-std-features=compiler-builtins-mem");
    for (b, _) in bins {
        cmd.arg("--bin").arg(b);
    }
    if low_ram { cmd.arg("--jobs").arg("1"); }

    // A failed userspace build fails the image, since an empty `/bin` still
    // boots
    if !cmd.status().expect("cargo build failed").success() {
        panic!("{dir}/ failed to build - the image would ship without those programs");
    }

    let bin_dir = crate_dir.join("target/x86_64-miku-app/release");
    let mut out = Vec::new();
    for (b, _) in bins {
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

/// System programs. The second column is the package
const SYSTEM_BINS: &[(&str, &str)] = &[
    ("maywm", "miku-desktop"), ("mayterm", "miku-desktop"),
    // The shell (dev/terminal-architecture.md). The kernel's is the emergency
    // console
    ("msh", "miku-base"),
    // Users and privilege: mikudod changes identity, mikudo is its unprivileged
    // client
    ("mikudo", "miku-system"), ("mikudod", "miku-system"), ("login", "miku-system"),
    ("passwd", "miku-system"), ("id", "miku-system"), ("su", "miku-system"),
    ("useradd", "miku-system"),
    // Package tool
    ("mikupkg", "miku-base"),
    // Root half of the package tool
    ("mikupkg-apply", "miku-base"),
    // Greeter and session program
    ("maygreet", "miku-desktop"), ("maysession", "miku-desktop"),
];

/// Display demo and ABI smoke tests.
/// Commands moved from the kernel shell to ring 3. The kernel keeps what needs
/// ring 0: block layer, `dd`, `mount`, `mkfs`, `nvidia`, selftests
const UTIL_BINS: &[(&str, &str)] = &[
    // files
    ("ls", "miku-base"), ("cat", "miku-base"), ("cp", "miku-base"), ("mv", "miku-base"),
    ("rm", "miku-base"), ("mkdir", "miku-base"), ("rmdir", "miku-base"), ("ln", "miku-base"),
    ("readlink", "miku-base"), ("touch", "miku-base"), ("stat", "miku-base"),
    ("chmod", "miku-base"), ("chown", "miku-base"), ("du", "miku-base"), ("df", "miku-base"),
    ("tree", "miku-base"),
    // text
    ("head", "miku-base"), ("tail", "miku-base"), ("wc", "miku-base"), ("grep", "miku-base"),
    ("sort", "miku-base"), ("uniq", "miku-base"), ("tee", "miku-base"), ("echo", "miku-base"),
    // the machine
    ("ps", "miku-base"), ("kill", "miku-base"), ("sleep", "miku-base"), ("date", "miku-base"),
    ("uptime", "miku-base"), ("uname", "miku-base"), ("free", "miku-base"),
    ("clear", "miku-base"), ("which", "miku-base"), ("env", "miku-base"),
    // Kernel commands forwarded from ring 3 (utils/src/mikuctl.rs)
    ("mikuctl", "miku-base"),
];

/// Programs built by their own subcommand:
/// `(name in /bin, package, unstripped binary)`. Packaged and stripped if
/// present, otherwise skipped with a message
const EXTERNAL_BINS: &[(&str, &str, &str)] = &[
    // `cd builder && cargo run --release -- zed build -p zed`
    ("zed", "zed", "target/zed-miku/target/x86_64-unknown-miku/release/zed"),
];

/// Crate directory of the `std` programs. One level below the root, since the
/// target spec passes `-T../lib/sdk/miku/linker.ld`
const STD_CRATE: &str = "stdapps";

/// Every `std` crate and its programs, and whether the image requires it
const STD_CRATES: &[(&str, &[(&str, &str)], bool)] = &[
    (STD_CRATE, STD_BINS, false),
    ("aosh", AOSH_BINS, true),
];

/// QML desktop shell (`dev/aosh-architecture.md`). Required
const AOSH_BINS: &[(&str, &str)] = &[
    ("aosh", "miku-desktop"),
];

/// Programs built against `std`, and their packages (`dev/std-architecture.md`)
const STD_BINS: &[(&str, &str)] = &[
    ("stdhello", "miku-demos"),
    ("stdthreads", "miku-demos"),
    ("stdfs", "miku-demos"),
    ("stdbench", "miku-demos"),
    ("stdproc", "miku-demos"),
    ("stdpoll", "miku-demos"),
    ("stdasync", "miku-demos"),
    ("stdtokio", "miku-demos"),
    ("stdc", "miku-demos"),
    ("stdsignal", "miku-demos"),
    ("stdwatch", "miku-demos"),
    ("stddns", "miku-demos"),
    ("stdpty", "miku-demos"),
    ("stdremote", "miku-demos"),
];

const APP_BINS: &[(&str, &str)] = &[
    // Demos and ABI smoke tests
    ("hello", "miku-demos"), ("maydemo", "miku-demos"), ("gpudemo", "miku-demos"),
    ("polltest", "miku-demos"), ("abitest", "miku-demos"), ("crashtest", "miku-demos"),
    ("threadtest", "miku-demos"), ("testfull", "miku-demos"), ("mmaptest", "miku-demos"),
    ("envtest", "miku-demos"), ("sigtest", "miku-demos"),
    // Network tools
    ("wget", "miku-net"), ("curl", "miku-net"), ("host", "miku-net"),
    ("ntpdate", "miku-net"), ("ping", "miku-net"), ("traceroute", "miku-net"),
];

fn build_ldmiku(root: &Path, low_ram: bool) {
    let ldmiku_dir = root.join("lib/runtime/ld-miku");
    if !ldmiku_dir.exists() {
        panic!("[!] lib/runtime/ld-miku/ not found at {} - run builder from miku-os/builder/", ldmiku_dir.display());
    }

    println!("\nBuilding ld-miku.so  (lib/runtime/ld-miku/)...");

    let ld_script = ldmiku_dir.join("ld_link.ld");

    let rustflags = [
        "-C relocation-model=pic",
        "-C link-arg=-pie",
        "-C link-arg=-z",  "-C link-arg=noexecstack",
        "-C link-arg=-z",  "-C link-arg=now",
        "-C link-arg=--no-dynamic-linker",
        &format!("-C link-arg=-T{}", ld_script.display()),
        "-C no-redzone=y",
        // Frame pointers, used by the kernel for crash report backtraces
        "-C force-frame-pointers=yes",
        "-C link-arg=--build-id=sha1",
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
    let bin_dst = root.join("lib/runtime/ld-miku/ld-miku.bin");
    if !bin_src.exists() {
        panic!("ld-miku binary not found at {}", bin_src.display());
    }
    fs::copy(&bin_src, &bin_dst)
        .unwrap_or_else(|e| panic!("Cannot copy ld-miku.bin: {}", e));
    println!("[ok] ld-miku.bin → lib/runtime/ld-miku/ld-miku.bin ({} KB)",
        fs::metadata(&bin_dst).unwrap().len() / 1024);
}

/// vDSO (lib/runtime/vdso/): one page of code mapped into every process for
/// `clock_gettime`. Built like ld-miku and embedded with `include_bytes!`,
/// except `-shared` instead of `-pie` and `--hash-style=sysv`, since DT_HASH's
/// chain count gives the symbol count
fn build_vdso(root: &Path, low_ram: bool) {
    let vdso_dir = root.join("lib/runtime/vdso");
    if !vdso_dir.exists() {
        panic!("[!] lib/runtime/vdso/ not found at {} - run builder from miku-os/builder/", vdso_dir.display());
    }

    println!("\nBuilding vdso  (lib/runtime/vdso/)...");

    let ld_script = vdso_dir.join("vdso.ld");

    let rustflags = [
        "-C relocation-model=pic",
        "-C link-arg=-shared",
        "-C link-arg=-Bsymbolic",
        "-C link-arg=-z",  "-C link-arg=noexecstack",
        "-C link-arg=--hash-style=sysv",
        &format!("-C link-arg=-T{}", ld_script.display()),
        "-C no-redzone=y",
        "-C force-frame-pointers=yes",
    ].join(" ");

    let mut cmd = Command::new("cargo");
        cmd.current_dir(&vdso_dir)
            .env("RUSTFLAGS", &rustflags)
            .arg("+nightly")
            .arg("build")
            .arg("--release")
            .arg("--target").arg("x86_64-miku-vdso.json")
            .arg("-Z").arg("json-target-spec")
            .arg("-Z").arg("build-std=core")
            .arg("-Z").arg("build-std-features=compiler-builtins-mem");

    if low_ram { cmd.arg("--jobs").arg("1"); }

    if !cmd.status().expect("cargo build vdso failed").success() {
        panic!("vdso compilation failed");
    }

    let bin_src = root.join("target/x86_64-miku-vdso/release/vdso");
    let bin_dst = root.join("lib/runtime/vdso/vdso.bin");
    if !bin_src.exists() {
        panic!("vdso binary not found at {}", bin_src.display());
    }
    fs::copy(&bin_src, &bin_dst)
        .unwrap_or_else(|e| panic!("Cannot copy vdso.bin: {}", e));
    println!("[ok] vdso.bin → lib/runtime/vdso/vdso.bin ({} bytes)",
        fs::metadata(&bin_dst).unwrap().len());
}

/// Library names from lib/runtime/libmiku/libs.list
fn miku_libs(root: &Path) -> Vec<String> {
    miku_buildgen::libs::read(&root.join("lib/runtime/libmiku")).into_iter().map(|l| l.name).collect()
}

fn build_libmiku(root: &Path, low_ram: bool) {
    let libmiku_dir = root.join("lib/runtime/libmiku");
    if !libmiku_dir.exists() {
        panic!("[!] lib/runtime/libmiku/ not found at {} - run builder from miku-os/builder/", libmiku_dir.display());
    }

    println!("\nBuilding miku libraries  (lib/runtime/libmiku/)...");

    let rustflags = [
        "-C relocation-model=pic",
        "-C link-arg=-pie",
        "-C link-arg=-z",  "-C link-arg=noexecstack",
        "-C link-arg=-z",  "-C link-arg=now",
        "-C link-arg=--no-dynamic-linker",
        "-C link-arg=--hash-style=both",
        "-C no-redzone=y",
        // Frame pointers, used by the kernel for crash report backtraces
        "-C force-frame-pointers=yes",
        "-C link-arg=--build-id=sha1",
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
    println!("[ok] miku libraries built");

    let libs_dir = root.join("lib/runtime/libmiku/libs");
    fs::create_dir_all(&libs_dir)
        .unwrap_or_else(|e| panic!("Cannot create {}: {}", libs_dir.display(), e));
    for lib in miku_libs(root) {
        let bin_src = libmiku_dir.join(format!("target/x86_64-miku-lib/release/{}", lib));
        let bin_dst = libs_dir.join(format!("{}.so", lib));
        if !bin_src.exists() {
            panic!("{} binary not found at {}", lib, bin_src.display());
        }
        fs::copy(&bin_src, &bin_dst)
            .unwrap_or_else(|e| panic!("Cannot copy {}.so: {}", lib, e));
        println!("[ok] {}.so → lib/runtime/libmiku/libs/{}.so ({} KB)",
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

    // Stages /lib/firmware as a GRUB module, so the boot medium needs no second
    // disk
    let have_fw = build_firmware_module(root, &iso_root);

    let grub_cfg_src = root.join("grub.cfg");
    let grub_cfg_dst = iso_root.join("boot/grub/grub.cfg");
    let cfg = fs::read_to_string(&grub_cfg_src)
        .unwrap_or_else(|e| panic!("Cannot read grub.cfg: {}", e));
    // The timeout comes from `grub.cfg`
    let mut new_cfg = String::new();
    for line in cfg.lines() {
        let t = line.trim();
        new_cfg.push_str(line);
        new_cfg.push('\n');
        // Loads the firmware image after the kernel, as a multiboot2 module
        // tagged "firmware"
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

/// Makes root own everything on the image. `mke2fs -d` copies uid and gid from
/// the build host, usually 1000, which is MikuOS's `miku`. debugfs `sif` fixes
/// this, since `mke2fs` has no option for it
fn chown_image_to_root(img: &Path, staging: &Path) {
    if Command::new("debugfs").arg("-V").output().map(|o| !o.status.success()).unwrap_or(true) {
        println!("[!] debugfs missing - image files will be owned by the build host's uid, \
                  and mikudo will refuse to run");
        return;
    }
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    collect_tree(staging, "", &mut dirs, &mut files);

    let mut script = String::from("sif / uid 0\nsif / gid 0\n");
    for d in &dirs {
        script.push_str(&format!("sif /{d} uid 0\nsif /{d} gid 0\n"));
    }
    for (rel, _) in &files {
        script.push_str(&format!("sif /{rel} uid 0\nsif /{rel} gid 0\n"));
    }

    let script_path = staging.parent().unwrap().join("chown_root.script");
    fs::write(&script_path, &script).expect("write chown script");
    let ok = Command::new("debugfs")
        .args(["-w", "-f", script_path.to_str().unwrap(), img.to_str().unwrap()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        println!("[ok] {} files and {} directories now owned by root", files.len(), dirs.len());
    } else {
        println!("[!] debugfs could not set ownership - mikudo will refuse its own config");
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

/// Files and directories under dir
fn dir_entries(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(rd) = fs::read_dir(dir) {
        for entry in rd.flatten() {
            total += 1;
            let p = entry.path();
            if p.is_dir() {
                total += dir_entries(&p);
            }
        }
    }
    total
}

/// Firmware folders under kernel/drivers/gpu/nvidia/firmware/ and the
/// /lib/firmware path fwload requests. Others stage to
/// /lib/firmware/nvidia/<folder name>
const NVIDIA_FW_CARDS: &[(&str, &str)] = &[
    ("gtx1650",   "nvidia/tu116"),   // GTX 1650 / 1660 (TU116/TU117)
    ("rtx5060ti", "nvidia/gb206"),   // RTX 5060 Ti / 5060 (GB206)
];

/// Firmware folders for this image, and a line saying why. Everything by
/// default, since the build host need not be the target. MikuOS prints the
/// `MIKU_FW=` line for each NVIDIA card found (`nvidia::init`).
///
/// MIKU_FW=all                    everything (default)
/// MIKU_FW=none                   no GPU firmware
/// MIKU_FW=nvidia/gb206           as the kernel prints
/// MIKU_FW=rtx5060ti[,gtx1650]    folder names
fn firmware_folders_wanted() -> (Option<Vec<String>>, String) {
    let Ok(v) = env::var("MIKU_FW") else {
        return (None, "every card (set MIKU_FW to trim - MikuOS prints the line)".into());
    };
    let v = v.trim().to_lowercase();
    match v.as_str() {
        "" | "all" => (None, "MIKU_FW=all".into()),
        "none"     => (Some(Vec::new()), "MIKU_FW=none".into()),
        _ => {
            // Accepts a folder name or the printed /lib/firmware path
            let want: Vec<String> = v
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    NVIDIA_FW_CARDS
                        .iter()
                        .find(|(_, chip)| *chip == s)
                        .map(|(folder, _)| (*folder).to_string())
                        .unwrap_or_else(|| s.to_string())
                })
                .collect();
            let why = format!("MIKU_FW={}", want.join(","));
            (Some(want), why)
        }
    }
}

/// Passes build output to the packager: the program list, the selected
/// firmware, and the image writers' paths. Layout is `pkg.rs`, installation is
/// `mikupkg`'s applier. Returns the staging root and whether firmware was
/// found. Missing firmware is normal, since the blobs are not in git
fn stage_root_tree(root: &Path) -> Option<(PathBuf, bool)> {
    let fw_home = root.join("kernel/drivers/gpu/nvidia/firmware");

    // Selected firmware
    let mut firmware: Vec<(String, PathBuf)> = Vec::new();
    let mut cards: Vec<_> = match fs::read_dir(&fw_home) {
        Ok(rd) => rd.flatten().filter(|e| e.path().is_dir()).collect(),
        Err(_) => Vec::new(),
    };
    cards.sort_by_key(|e| e.file_name());
    let (wanted, why) = firmware_folders_wanted();
    println!("[i] firmware selection: {}", why);
    let mut skipped_kb = 0u64;
    for card in cards {
        let name = card.file_name().into_string().unwrap();
        let dest_rel = NVIDIA_FW_CARDS.iter()
            .find(|(folder, _)| *folder == name)
            .map(|(_, chip)| (*chip).to_string())
            .unwrap_or_else(|| format!("nvidia/{}", name));
        let kb = dir_size(&card.path()) / 1024;
        if kb == 0 { continue; }
        if let Some(w) = &wanted {
            if !w.iter().any(|f| f == &name) {
                println!("[i] firmware {} skipped ({} KB) - not in MIKU_FW", name, kb);
                skipped_kb += kb;
                continue;
            }
        }
        let before = firmware.len();
        collect_files(&card.path(), &format!("lib/firmware/{dest_rel}"), &mut firmware);
        println!("[ok] firmware {} -> /lib/firmware/{} ({} KB, {} files)",
            name, dest_rel, kb, firmware.len() - before);
    }
    if skipped_kb > 0 {
        println!("[i] {} KB of firmware left out of the boot image - unset MIKU_FW to put it back",
            skipped_kb);
    }

    // the programs, each with the package its line in the table names
    let mut programs: Vec<(&'static str, &'static str, PathBuf)> = Vec::new();
    for (dir, bins, _) in PROGRAM_CRATES {
        let out = root.join(dir).join("target/x86_64-miku-app/release");
        if !out.is_dir() {
            panic!("{dir}/ has no build output - /bin would ship without it");
        }
        for (b, package) in *bins {
            let src = out.join(b);
            // A listed program that did not build means a broken image
            if !src.exists() {
                panic!("{dir}/target/.../{b} is missing - is it still a [[bin]] in {dir}/Cargo.toml?");
            }
            programs.push((b, package, src));
        }
    }

    // `std` programs, from their own target directory. Absent is not fatal,
    // since a tree may lack `rust-src`
    for (dir, bins, _) in STD_CRATES {
        if !std_crate_wanted(dir) { continue; }
        let std_out = root.join(dir).join("target/x86_64-unknown-miku/release");
        for (b, package) in bins.iter() {
            let src = std_out.join(b);
            if src.exists() {
                programs.push((b, package, src));
            } else {
                println!("[i] {b} was not built - /bin ships without it");
            }
        }
    }
    // Externally built programs, stripped into a separate directory
    let stripped = root.join("target/image-extern");
    for (b, package, rel) in EXTERNAL_BINS {
        let src = root.join(rel);
        if !src.exists() {
            println!("[i] {b} was not built ({rel}) - /bin ships without it");
            continue;
        }
        fs::create_dir_all(&stripped).expect("target/image-extern");
        let dst = stripped.join(b);
        let fresh = fs::metadata(&dst).and_then(|d| d.modified()).ok()
            >= fs::metadata(&src).and_then(|s| s.modified()).ok();
        if !fresh {
            let ok = Command::new("strip").arg("-o").arg(&dst).arg(&src)
                .status().map(|s| s.success()).unwrap_or(false);
            if !ok {
                panic!("strip {} failed", src.display());
            }
        }
        println!("[ok] {b} ({} MB, package {package})", fs::metadata(&dst).unwrap().len() >> 20);
        programs.push((*b, *package, dst));
    }
    println!("[ok] {} programs to package", programs.len());

    let (staging, have_firmware) =
        pkg::build_image(root, env!("CARGO_PKG_VERSION"), &programs, &firmware);

    // /boot is not a package: the kernel and the two GRUB pieces written to raw
    // sectors
    let kernel_src = root.join("target/x86_64-unknown-none/release/miku-os-release");
    if kernel_src.exists() {
        let boot_dir = staging.join("boot");
        fs::create_dir_all(&boot_dir).ok();
        fs::copy(&kernel_src, boot_dir.join("kernel.elf")).ok();
    }
    build_boot_blobs(&staging);

    if !have_firmware {
        // Both image writers stage from here; this avoids printing the notice
        // twice
        static NOTICE: std::sync::Once = std::sync::Once::new();
        NOTICE.call_once(|| {
            println!("[i] no NVIDIA firmware in {} - image will boot without GPU firmware",
                fw_home.display());
            println!("[i]   see {}/README.md to add it", fw_home.display());
        });
    }

    Some((staging, have_firmware))
}

/// Every regular file under `dir`, as `(prefix/<relative path>, source)`
fn collect_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = match fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        let mut items: Vec<_> = rd.flatten().collect();
        items.sort_by_key(|e| e.file_name());
        for e in items {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let rel = p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
            out.push((format!("{prefix}/{rel}"), p));
        }
    }
}

/// LBA of core.img on an installed disk: partition 1, a BIOS boot partition
const INSTALL_CORE_LBA: u64 = 2048;

/// The sector boot.img jumps to, which `grub-mkimage` leaves set to 1
const BOOT_KERNEL_SECTOR: usize = 0x5c;
/// Block list at the end of core.img's first sector: rest of core.img (u64),
/// sector count (u16), load segment (u16). `grub-mkimage` sets the first to 2
const CORE_BLOCKLIST: usize = 0x1f4;

/// GRUB's BIOS files on this distribution
fn grub_pc_dir() -> Option<PathBuf> {
    ["/usr/lib/grub/i386-pc", "/usr/share/grub/i386-pc", "/lib/grub/i386-pc"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.join("boot.img").is_file())
}

/// Builds the two blobs an install writes raw. `grub-bios-setup` requires its
/// `-d` directory to be on the disk being set up, so the fields are patched
/// here: `grub-mkimage` writes them for core.img right after the boot sector
/// (sector 1 in boot.img, sector 2 in the block list), and they move to the
/// real location. Existing values are checked, failing the build if GRUB
/// changes them
fn build_boot_blobs(staging: &Path) -> bool {
    let Some(pc) = grub_pc_dir() else {
        println!("[i] no GRUB i386-pc parts on this machine - the image will \
                  boot, but 'install' will have no bootloader to write");
        return false;
    };

    let work = staging.parent().unwrap_or(Path::new(".")).join("bootblobs");
    fs::create_dir_all(&work).ok();
    let core_img = work.join("core.img");

    // No device in the prefix: core.img looks for /boot/grub on the second
    // partition of the boot disk. Modules are those needed to read a
    // filesystem, plus all_video for the framebuffer
    let ok = Command::new("grub-mkimage")
        .args([
            "-O", "i386-pc",
            "-p", "(,gpt2)/boot/grub",
            "-o", core_img.to_str().unwrap(),
            "biosdisk", "part_gpt", "ext2", "multiboot2",
            "normal", "configfile", "search", "echo", "ls", "halt", "reboot",
            "all_video", "gfxterm", "video",
        ])
        .status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        println!("[!] grub-mkimage failed - 'install' will have no bootloader to write");
        return false;
    }

    let mut boot = match fs::read(pc.join("boot.img")) { Ok(b) => b, Err(_) => return false };
    let mut core = match fs::read(&core_img)           { Ok(b) => b, Err(_) => return false };
    if boot.len() != 512 || core.len() < 512 {
        println!("[!] GRUB's boot.img is {} bytes and core.img {} - not what this \
                  expects; skipping the bootloader", boot.len(), core.len());
        return false;
    }

    let sectors = core.len().div_ceil(512) as u64;
    let read_u64 = |b: &[u8], at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
    let listed = u16::from_le_bytes(core[CORE_BLOCKLIST + 8..CORE_BLOCKLIST + 10]
                                    .try_into().unwrap()) as u64;

    // Checks the values grub-mkimage wrote before moving them
    if read_u64(&boot, BOOT_KERNEL_SECTOR) != 1
        || read_u64(&core, CORE_BLOCKLIST) != 2
        || listed != sectors - 1
    {
        println!("[!] this GRUB does not lay out boot.img/core.img the way \
                  'install' patches them (sector {}, list {} of {}); skipping \
                  the bootloader",
                 read_u64(&boot, BOOT_KERNEL_SECTOR), read_u64(&core, CORE_BLOCKLIST), listed);
        return false;
    }

    boot[BOOT_KERNEL_SECTOR..BOOT_KERNEL_SECTOR + 8]
        .copy_from_slice(&INSTALL_CORE_LBA.to_le_bytes());
    core[CORE_BLOCKLIST..CORE_BLOCKLIST + 8]
        .copy_from_slice(&(INSTALL_CORE_LBA + 1).to_le_bytes());
    core.resize(sectors as usize * 512, 0);

    // Only the first 440 bytes of the boot sector; the rest is the partition
    // table
    let dst = staging.join("boot/grub/i386-pc");
    fs::create_dir_all(&dst).ok();
    let a = fs::write(dst.join("mbr.bin"), &boot[..440]).is_ok();
    let b = fs::write(dst.join("core.bin"), &core).is_ok();
    if !(a && b) {
        println!("[!] could not stage the bootloader blobs");
        return false;
    }

    println!("[ok] bootloader: mbr.bin (440 B) + core.bin ({} sectors at LBA {})",
             sectors, INSTALL_CORE_LBA);
    true
}

/// Builds a compact ext2 image of /lib/firmware as a GRUB multiboot2 module.
/// The kernel exposes it as a block device and mounts it (`fwload::init`).
/// Returns true if created
fn build_firmware_module(root: &Path, iso_root: &Path) -> bool {
    let (staging, have_firmware) = match stage_root_tree(root) {
        Some(v) => v,
        None => return false,
    };
    // Built whenever anything was staged: it also provides /bin and the kernel
    // and bootloader for `install`
    if !check_mke2fs() {
        println!("[!] mke2fs missing - ISO will NOT carry /lib/firmware, /bin or a \
                  bootloader to install with");
        return false;
    }
    let staged_bytes = dir_size(&staging);
    // Staged size plus 5%, at least 4 MiB, for ext2 metadata
    let slack = (staged_bytes / 20).max(4 * 1024 * 1024);
    let size_mb = (((staged_bytes + slack) / (1024 * 1024)) + 1).max(40) as u32;
    // One inode per 64 KiB, to avoid large empty inode tables, and at least
    // what the tree holds plus the reserved ones
    let inodes = (size_mb as u64 * 16).max(dir_entries(&staging) + 64);

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
            // 4 KiB blocks. mke2fs picks 1 KiB here, and a 200 MB file then
            // needs more indirect blocks and descriptors than the slack
            "-b", "4096",
            "-N", &inodes.to_string(),
            "-d", staging.to_str().unwrap(),
            img.to_str().unwrap(),
        ])
        .status().expect("mke2fs failed").success();
    if !ok { panic!("mke2fs failed to format firmware.img"); }

    chown_image_to_root(&img, &staging);

    println!("[ok] boot module: boot/firmware.img ({} MB ext2, {} KB staged{})",
        size_mb, staged_bytes / 1024,
        if have_firmware { "" } else { ", no GPU firmware" });
    true
}

fn provision_root_disk(root: &Path, disk_path: &Path, size_mb: u32) {
    // Staging holds /bin even with no firmware, so the disk is still created
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

        // ext4 with a journal, since a running machine writes to this image,
        // unlike the read-only module. `^resize_inode` stays because
        // `lib/fs/extfs` has no resize. Otherwise e2fsprogs defaults (extents,
        // dir_index, metadata_csum), covered by
        // `lib/fs/extfs/tests/mke2fs_image.rs`
        let ok = Command::new("mke2fs")
            .args([
                "-q", "-F",
                "-t", "ext4",
                "-O", "^resize_inode",
                "-d", staging.to_str().unwrap(),
                disk_path.to_str().unwrap(),
            ])
            .status().expect("mke2fs failed").success();
        if !ok { panic!("mke2fs failed to format disk.img"); }
        println!("[ok] disk.img formatted ext4 (journalled) + firmware staged ({} KB)", staged_kb);
    } else {
        // Pushes firmware only when the image lacks it
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

/// Refreshes the staged tree into an existing ext image via a debugfs script:
/// mkdir every directory parents first, then rm and write each file. debugfs
/// continues past errors, so the script is idempotent

/// True if `path` exists in the ext image
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
    // debugfs exits 0 even when it cannot open the filesystem, so it is probed
    // first
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

    // Firmware is written once, when the image lacks it
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
        script.push_str(&format!("sif /{d} uid 0\nsif /{d} gid 0\n"));
    }
    for (rel, host) in &files {
        script.push_str(&format!("rm /{}\n", rel));
        script.push_str(&format!("write {} /{}\n", host.display(), rel));
        // debugfs `write` creates files as 0100644, so `sif` sets the staging
        // tree's mode (`apply_etc_modes`). Without it /bin has no execute bit
        // and /etc/shadow is world-readable
        script.push_str(&format!("sif /{} mode 0{:o}\n", rel, host_mode(host)));
        // Owned by root: debugfs `write` takes the uid of whoever runs the
        // build
        script.push_str(&format!("sif /{rel} uid 0\nsif /{rel} gid 0\n"));
    }

    let script_path = staging.parent().unwrap().join("fw_debugfs.script");
    fs::write(&script_path, &script).expect("write debugfs script");

    let ok = Command::new("debugfs")
        .args(["-w", "-f", script_path.to_str().unwrap(), img.to_str().unwrap()])
        .status().expect("debugfs failed").success();
    if !ok { panic!("debugfs failed to inject firmware into disk.img"); }
    true
}

/// Full `st_mode` for debugfs, including file-type bits, from the staging copy
fn host_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    const S_IFREG: u32 = 0o100000;
    let perm = fs::metadata(path).map(|m| m.permissions().mode() & 0o7777).unwrap_or(0o644);
    S_IFREG | perm
}

/// Pre-order walk: relative directory paths (parents first) and (relative file
/// path, absolute host path) pairs
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

/// `cargo run --release -- <subcommand>`: manual `std` port builds
fn subcommand(root: &Path, sub: &str, rest: &[String]) -> i32 {
    let res = match sub {
        // `std <crate-dir> [--check] [cargo args...]`: builds one crate against
        // MikuOS's `std`
        "std" => match rest.first() {
            Some(dir) => {
                let check = rest.get(1).is_some_and(|a| a == "--check");
                let args: Vec<String> = rest.iter().skip(if check { 2 } else { 1 }).cloned().collect();
                stdport::prepare(root, &[dir.as_str()], true)
                    .and_then(|ov| stdport::build(root, &root.join(dir), &ov, check, args))
            }
            None => Err("usage: std <crate-dir> [--check] [cargo args...]".into()),
        },
        // `zed build|check|tree [cargo args...]`
        "zed" => match rest.first() {
            Some(cmd) => stdport::zed(root, cmd, &rest[1..]),
            None => Err("usage: zed build|check|tree [cargo args...]".into()),
        },
        // `overlay`: assembles `target/rust-src-miku` only, needed by host
        // tests such as `cd aosh && cargo test-host`
        "overlay" => stdport::prepare(root, &["stdapps", "aosh"], true).map(|ov| {
            println!("{}", ov.display());
        }),
        _ => Err(format!("unknown subcommand {sub}: std <crate-dir> | zed build|check|tree | overlay")),
    };
    match res {
        Ok(()) => 0,
        Err(e) => { eprintln!("[!] {e}"); 1 }
    }
}

fn main() {
    println!("MikuOS Builder\n");

    // Repository root, from this crate's location
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("builder/ has no parent - the repository layout has changed")
        .to_path_buf();
    // Subcommands: a `std` crate, Zed, or the overlay alone
    let argv: Vec<String> = env::args().skip(1).collect();
    if let Some(sub) = argv.first() {
        std::process::exit(subcommand(&root, sub, &argv[1..]));
    }

    let low_ram = ask_user("Low RAM mode? [y/N]: ", 10);
    // Optional, since their ported crates (tokio, mio, inotify...) may need
    // refetching. aosh is always built
    let with_std = ask_user(
        "Build the std programs (stdapps: stdbench, stdfs, stdtokio...)? Missing crate sources are downloaded [y/N]: ", 10);
    WITH_STDAPPS.store(with_std, std::sync::atomic::Ordering::Relaxed);

    check_grub_mkrescue();
    build_ldmiku(&root, low_ram);
    build_vdso(&root, low_ram);
    build_libmiku(&root, low_ram);
    build_userspace(&root, low_ram);
    build_kernel(&root, low_ram);
    create_iso(&root);

    let cfg       = DiskConfig::ask(&root);
    let disk_path = root.join("miku-os/disk.img");
    let data_path = root.join("miku-os/data.img");

    // Persistent root: formats it and adds /lib/firmware
    provision_root_disk(&root, &disk_path, cfg.main_mb);
    if cfg.data_mb > 0 { ensure_disk(&data_path, cfg.data_mb, "data"); }

    if !ask_user("\nLaunch QEMU? [y/N]: ", 10) { return; }

    let ram      = detect_qemu_ram();
    let iso_path = root.join("miku-os/miku-os.iso");

    // disk.img as drive 1 (primary slave). The kernel mounts it and grafts its
    // /lib/firmware onto the VFS
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
        // xHCI controller and USB keyboard, for the native HID stack (PS/2
        // still works through i8042)
        "-device".into(), "qemu-xhci,id=xhci".into(),
        "-device".into(), "usb-kbd,bus=xhci.0".into(),
        // Tablet for absolute pointing, so the host pointer is not captured.
        // Also exercises report descriptors, since a tablet has no boot
        // protocol
        "-device".into(), "usb-tablet,bus=xhci.0".into(),
    ];

    if cfg.data_mb > 0 && data_path.exists() {
        args.push("-drive".into());
        args.push(format!("file={},format=raw,if=none,id=disk1,cache=unsafe,aio=threads",
            data_path.display()));
        args.push("-device".into());
        args.push("ide-hd,drive=disk1,bus=ide.1,unit=1,rotation_rate=1".into());
        println!("[*] data.img attached as drive 2");
    }

    // `-enable-kvm -version` succeeds without KVM, so /dev/kvm is probed
    // instead
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
