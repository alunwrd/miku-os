// Compiles GPU programs at build time
//
// `nvcc -cubin` gives an ELF64 with the program in `.text.<name>` and constant
// bank 0's template in `.nv.constant0.<name>`. Parsed by hand.
//
// Three values go with each program for the launch descriptor: registers per
// thread and shared memory per block from `cuobjdump -res-usage`, and the size
// of constant bank 0.
//
// With no CUDA toolkit, the build has no GPU programs and reports them
// unavailable.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What the launch descriptor has to be told about a program
pub struct Program {
    pub name: String,
    pub code: Vec<u8>,
    /// Registers per thread. Too few rejects the launch; too many reduces
    /// blocks per multiprocessor
    pub regs: u32,
    /// Shared memory a block asks for, in bytes
    pub shared: u32,
    /// Constant bank 0: driver-filled head, then parameters at `param_base`
    pub const0: u32,
}

/// Parameter offset in constant bank 0, fixed by the compiler's ABI: a kernel's
/// first parameter loads from `c[0x0][0x380]`, and below it is the
/// driver-filled head. `generate` verifies every kernel's bank is at least this
/// long
pub const PARAM_BASE: u32 = 0x380;

/// Compiles every `.cu` in `dir` for `arch`, or returns empty without the
/// toolkit
pub fn generate(dir: &Path, out_dir: &Path, arch: &str) -> Vec<Program> {
    println!("cargo:rerun-if-changed={}", dir.display());

    let nvcc = match find_tool("nvcc") {
        Some(p) => p,
        None => {
            println!("cargo:warning=no nvcc - building without GPU programs (blur, shadows)");
            write_table(out_dir, &[], arch, false);
            return Vec::new();
        }
    };

    let mut sources: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "cu").unwrap_or(false))
            .collect(),
        Err(_) => Vec::new(),
    };
    sources.sort();

    let mut programs = Vec::new();
    for src in &sources {
        match compile(&nvcc, src, out_dir, arch) {
            Ok(mut p) => programs.append(&mut p),
            Err(e) => panic!("shader {}: {}", src.display(), e),
        }
    }
    write_table(out_dir, &programs, arch, true);
    programs
}

fn find_tool(name: &str) -> Option<PathBuf> {
    // Toolkit search path, including /opt/cuda
    for base in ["", "/opt/cuda/bin/", "/usr/local/cuda/bin/"] {
        let p = PathBuf::from(format!("{}{}", base, name));
        let found = if base.is_empty() {
            Command::new(name).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
        } else {
            p.exists()
        };
        if found {
            return Some(if base.is_empty() { PathBuf::from(name) } else { p });
        }
    }
    None
}

/// Flags from a file's first line: `// nvcc: --fmad=false`. Public for the host
/// check of `paint.cu`
pub fn file_flags(src: &Path) -> Vec<String> {
    let first = std::fs::read_to_string(src).ok()
        .and_then(|t| t.lines().next().map(str::to_string))
        .unwrap_or_default();
    first.strip_prefix("// nvcc:")
        .map(|f| f.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

fn compile(nvcc: &Path, src: &Path, out_dir: &Path, arch: &str) -> Result<Vec<Program>, String> {
    let stem = src.file_stem().unwrap().to_string_lossy().to_string();
    let cubin = out_dir.join(format!("{}.cubin", stem));

    let out = Command::new(nvcc)
        .args(["-arch", arch, "-cubin"])
        .args(file_flags(src))
        .arg("-o")
        .arg(&cubin)
        .arg(src)
        .output()
        .map_err(|e| format!("nvcc did not run: {}", e))?;
    if !out.status.success() {
        return Err(format!("nvcc failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }

    let bytes = std::fs::read(&cubin).map_err(|e| format!("{}", e))?;

    // Keeps the cubin as a file for `gpu_program`. `target/gpu` is staged to
    // `/etc/gpu`
    let staged = Path::new("target/gpu");
    if std::fs::create_dir_all(staged).is_ok() {
        let _ = std::fs::copy(&cubin, staged.join(format!("{}.cubin", stem)));
    }
    let usage = res_usage(&cubin);
    let mut programs = Vec::new();
    for (name, code, const0) in sections(&bytes)? {
        let Some(u) = usage.iter().find(|u| u.name == name) else {
            return Err(format!("kernel '{}': cuobjdump -res-usage did not say what it uses", name));
        };
        let (regs, shared) = (u.regs, u.shared);
        // Rejects what kernel launches do not provide (`compute.rs`,
        // `build_qmd`): stack, local memory, and constant banks past the first,
        // where nvcc puts jump tables
        if u.stack != 0 || u.local != 0 {
            return Err(format!("kernel '{}' needs a stack ({} bytes, {} local) - an array indexed by a \
                                number worked out at run time, or a call; the launches set up none", name, u.stack, u.local));
        }
        if u.const2 != 0 {
            return Err(format!("kernel '{}' reads constant bank 2 ({} bytes) - a jump table, from a switch or \
                                an if chain over close values; the launches bind bank 0 only", name, u.const2));
        }
        if const0 < PARAM_BASE {
            return Err(format!(
                "kernel '{}': constant bank 0 is {:#x}, shorter than the parameter base {:#x} -                  the ABI this was built against is not the one assumed here",
                name, const0, PARAM_BASE));
        }
        programs.push(Program { name, code, regs, shared, const0 });
    }
    if programs.is_empty() {
        return Err("no kernels in the cubin".to_string());
    }
    Ok(programs)
}

/// What `cuobjdump -res-usage` says one kernel uses
struct Usage {
    name: String,
    regs: u32,
    shared: u32,
    stack: u32,
    local: u32,
    /// Bytes of constant bank 2
    const2: u32,
}

/// `cuobjdump -res-usage`, parsed for the two descriptor values and the three
/// unsupported ones
fn res_usage(cubin: &Path) -> Vec<Usage> {
    let Some(tool) = find_tool("cuobjdump") else { return Vec::new() };
    let Ok(out) = Command::new(tool).arg("-res-usage").arg(cubin).output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut found = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Function ") {
            current = Some(rest.trim_end_matches(':').to_string());
            continue;
        }
        if let (Some(name), true) = (current.clone(), line.starts_with("REG:")) {
            let field = |key: &str| -> u32 {
                line.split_whitespace()
                    .find_map(|t| t.strip_prefix(key))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0)
            };
            found.push(Usage {
                name,
                regs: field("REG:"),
                shared: field("SHARED:"),
                stack: field("STACK:"),
                local: field("LOCAL:"),
                const2: field("CONSTANT[2]:"),
            });
            current = None;
        }
    }
    found
}

/// Cubin reader, shared with `kernel/drivers/gpu/nvidia/cubin.rs`
#[path = "cubin.rs"]
mod cubin;

/// Pull `(name, code, const0_size)` out of the cubin for every kernel in it
fn sections(elf: &[u8]) -> Result<Vec<(String, Vec<u8>, u32)>, String> {
    cubin::kernels(elf)
        .map(|ks| ks.into_iter().map(|k| (k.name, k.code, k.const0)).collect())
        .map_err(|e| e.why().to_string())
}

/// Emit the table the kernel includes
fn write_table(out_dir: &Path, programs: &[Program], arch: &str, have_toolkit: bool) {
    let mut s = String::new();
    s.push_str("// Generated by src/lib/gpu/shadergen.rs - do not edit\n\n");
    s.push_str(&format!("pub const ARCH: &str = \"{}\";\n", arch));
    s.push_str(&format!("pub const AVAILABLE: bool = {};\n", have_toolkit && !programs.is_empty()));
    s.push_str(&format!("pub const PARAM_BASE: u32 = {:#x};\n\n", PARAM_BASE));

    s.push_str("/// A program, and everything the launch descriptor has to know about it\n");
    s.push_str("pub struct Program {\n    pub name: &'static str,\n    \
                pub code: &'static [u8],\n    pub regs: u32,\n    \
                pub shared: u32,\n    pub const0: u32,\n}\n\n");

    s.push_str(&format!("pub static PROGRAMS: [Program; {}] = [\n", programs.len()));
    for p in programs {
        s.push_str(&format!(
            "    Program {{ name: \"{}\", regs: {}, shared: {}, const0: {:#x}, code: &[\n",
            p.name, p.regs, p.shared, p.const0));
        for chunk in p.code.chunks(16) {
            s.push_str("        ");
            for b in chunk {
                s.push_str(&format!("{:#04x}, ", b));
            }
            s.push('\n');
        }
        s.push_str("    ] },\n");
    }
    s.push_str("];\n\n");

    s.push_str("/// Find a program by the name it has in its source\n");
    s.push_str("pub fn program(name: &str) -> Option<&'static Program> {\n    \
                PROGRAMS.iter().find(|p| p.name == name)\n}\n");

    std::fs::write(out_dir.join("gpu_shaders.rs"), s).expect("writing gpu_shaders.rs");
}
