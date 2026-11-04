use std::path::{Path, PathBuf};

// Artwork decoded and scaled, and GPU programs compiled, at build time, since
// the kernel has neither decoder nor compiler
#[path = "lib/buildgen/imagegen.rs"]
mod imagegen;

#[path = "lib/buildgen/shadergen.rs"]
mod shadergen;

// Wallpapers are generated as files, for selection in a config
#[path = "lib/buildgen/qoigen.rs"]
mod qoigen;

// The trust store is staged at /etc/ssl/certs/ca-bundle.pem and parsed by
// `lib/net` at run time

// The font is built by `libmay`'s build script

/// GB206 is Blackwell, compute capability 12.0
/// (`nvidia-smi --query-gpu=compute_cap`)
const GPU_ARCH: &str = "sm_120";

/// Width of the greeter's round portrait, the same as the installer's mascot
const GREETER_ART: u32 = 260;

fn main() {
    generate_miku_libs_table();

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    imagegen::generate(Path::new("docs/miku.png"), &out_dir, "mascot", 260, true);

    // Beside the target directory, since OUT_DIR has a hash outside tools do
    // not know
    qoigen::generate(
        Path::new("assets/wallpapers"),
        &[Path::new("docs/miku.png")],
        Path::new("target/wallpapers"),
        qoigen::WALLPAPER, "wallpaper");

    // Launcher icons, through the same converter at icon size
    qoigen::generate(
        Path::new("assets/icons"),
        &[],
        Path::new("target/icons"),
        qoigen::ICON, "icon");

    // Greeter portrait, cut to a circle here. A file, since the config can name
    // another
    qoigen::generate_round(
        Path::new("docs/miku.png"),
        Path::new("target/art"), "miku", GREETER_ART);

    let programs = shadergen::generate(
        Path::new("kernel/drivers/gpu/nvidia/shaders"), &out_dir, GPU_ARCH);
    for p in &programs {
        println!("cargo:warning=gpu program '{}': {} bytes, {} registers, \
                  {} bytes shared, bank0 {:#x}", p.name, p.code.len(), p.regs, p.shared, p.const0);
    }

    println!("cargo:rerun-if-changed=lib/buildgen/imagegen.rs");
    println!("cargo:rerun-if-changed=lib/buildgen/shadergen.rs");
    println!("cargo:rerun-if-changed=lib/buildgen/qoigen.rs");
    // Reruns on input changes
    println!("cargo:rerun-if-changed=docs/miku.png");
    println!("cargo:rerun-if-changed=build.rs");
}

// Generates the MIKU_LIBS preload table (src/ldso.rs) from the shared library
// manifest
fn generate_miku_libs_table() {
    let list = Path::new("lib/mikulibs/libs.list");
    let src = std::fs::read_to_string(list)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", list.display()));

    let mut output = String::from("pub static MIKU_LIBS: &[MikuLib] = &[\n");
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let name = line.split(':').next().unwrap().trim();
        output.push_str(&format!(
            "    MikuLib {{ name: \"{name}.so\", bytes: include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/lib/mikulibs/libs/{name}.so\")) }},\n"
        ));
    }
    output.push_str("];\n");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    std::fs::write(Path::new(&out_dir).join("miku_libs.rs"), output).unwrap();
    println!("cargo:rerun-if-changed=lib/mikulibs/libs.list");
}

