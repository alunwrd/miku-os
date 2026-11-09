// Link stubs and declarations for MikuOS programs
//
// A stub .so per library in ../../runtime/libmiku, with a matching soname, so a
// program gets one DT_NEEDED per library and ld-miku resolves the real symbols
// at load time. `cargo:rustc-link-*` lines apply to final binaries, so program
// crates get linkage by depending on `miku`.
//
// The `extern` blocks come from the libraries' own signatures.
//
// `x86_64-miku-app.json` passes `-T../lib/sdk/miku/linker.ld`, resolved against
// the crate, so program crates must sit one level below the root.

use std::env;
use std::fs;
use std::path::PathBuf;

use miku_buildgen::{decls, libs, stubs};

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../../runtime/libmiku");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", root.join("libs.list").display());

    let list = libs::read(&root);
    for l in &list {
        println!("cargo:rerun-if-changed={}", libs::dir(&root, &l.name).display());
    }

    fs::write(out_dir.join("decls.rs"), decls::generate(&root, &list)).unwrap();
    stubs::build_all(&root, &list, &out_dir);

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    for l in &list {
        println!("cargo:rustc-link-lib=dylib={}", l.name);
    }

    // Copy at a fixed path, `stubs/`, named by the `std` target spec, since `std`
    // programs do not depend on this crate
    let stubs_dir = manifest.join("stubs");
    let _ = fs::create_dir_all(&stubs_dir);
    for l in &list {
        let name = format!("lib{}.so", l.name);
        let _ = fs::copy(out_dir.join(&name), stubs_dir.join(&name));
    }
}
