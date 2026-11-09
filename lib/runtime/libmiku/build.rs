// The ten shared libraries: each [[bin]] becomes a <name>.so. Libraries call each
// other only through C symbols, resolved by ld-miku.
//
// Validates libs.list, generates the shims into OUT_DIR, and builds a link stub
// per library so each bin gets DT_NEEDED entries for its dependencies.

use std::env;
use std::fs;
use std::path::PathBuf;

use miku_buildgen::{libs, shim, stubs};

// Forms of `use` a shim cannot generate, one file per module in shim/manual/
fn manual(root: &std::path::Path, module: &str) -> Option<String> {
    fs::read_to_string(root.join("shim/manual").join(format!("{module}.rs"))).ok()
}

// The modules a root includes through `shim!(x);`
fn shims_of(root: &std::path::Path, lib: &str) -> Vec<String> {
    let src = fs::read_to_string(libs::dir(root, lib).join("main.rs")).unwrap();
    src.lines()
        .filter_map(|l| l.trim().strip_prefix("shim!(")?.strip_suffix(");"))
        .map(str::to_string)
        .collect()
}

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=libs.list");
    println!("cargo:rerun-if-changed=shim");

    let list = libs::read(&root);
    libs::validate(&list, &root);
    for l in &list {
        println!("cargo:rerun-if-changed={}", libs::dir(&root, &l.name).display());
    }

    let shims = out_dir.join("shim");
    fs::create_dir_all(&shims).unwrap();
    for l in &list {
        for m in shims_of(&root, &l.name) {
            // No module of its own: a shim written by hand whole
            let text = if m == "sync" {
                manual(&root, &m).expect("shim/manual/sync.rs")
            } else {
                shim::generate(&m, &libs::module_source(&list, &root, &m), manual(&root, &m).as_deref())
            };
            fs::write(shims.join(format!("{m}.rs")), text).unwrap();
        }
    }

    stubs::build_all(&root, &list, &out_dir);

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    for l in &list {
        let lib = &l.name;
        for dep in &l.deps {
            println!("cargo:rustc-link-arg-bin={lib}=-l{dep}");
        }
        // libc_miku exports POSIX names, so it keeps full --export-dynamic;
        // others expose only `miku_*`, letting --gc-sections drop unused code
        if lib == "libc_miku" {
            println!("cargo:rustc-link-arg-bin={lib}=--export-dynamic");
        } else {
            println!("cargo:rustc-link-arg-bin={lib}=--export-dynamic-symbol=miku_*");
            println!("cargo:rustc-link-arg-bin={lib}=--export-dynamic-symbol=__miku_*");
            println!("cargo:rustc-link-arg-bin={lib}=--gc-sections");
        }
    }
}
