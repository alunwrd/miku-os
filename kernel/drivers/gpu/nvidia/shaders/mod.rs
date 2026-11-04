// The GPU programs, as the kernel sees them
//
// `lib/buildgen/shadergen.rs` runs in `build.rs`, compiles every `.cu` next to
// this file for the card, and emits the table below: program bytes, registers
// per thread, shared memory, and the length of constant bank 0. The kernel
// loads and launches.
//
// `AVAILABLE` is false on a machine that built without the CUDA toolkit:
// everything needing a program says it is unavailable, as with a missing
// firmware blob

// Generated code: the table's contents follow from which `.cu` files exist, so
// `warn(dead_code)` on the driver subtree has nothing actionable to say
#[allow(dead_code)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/gpu_shaders.rs"));
}
pub use generated::*;

/// One line for the shell, so what got built is visible without a rebuild
pub fn describe() -> alloc::string::String {
    extern crate alloc;
    use alloc::string::String;
    use core::fmt::Write;

    let mut s = String::new();
    if !AVAILABLE {
        let _ = write!(s, "no GPU programs (built without the CUDA toolkit)");
        return s;
    }
    let _ = write!(s, "{} program(s) for {}:", PROGRAMS.len(), ARCH);
    for p in PROGRAMS.iter() {
        let _ = write!(s, " {} ({} bytes, {} regs)", p.name, p.code.len(), p.regs);
    }
    s
}
