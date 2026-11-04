// The system font, rasterized once for everything linking this library

use std::path::PathBuf;

#[path = "fontgen.rs"]
mod fontgen;

#[path = "icongen.rs"]
mod icongen;

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    // The repository's one copy of the assets, reached from this crate
    let assets = manifest.join("../../assets");

    let mono = assets.join("JetBrainsMono-Regular.ttf");
    fontgen::generate(&mono, &out_dir);
    // The interface faces. A proportional TTF at 'assets/ui-regular.ttf' and
    // 'assets/ui-bold.ttf' is picked up; without one the mono face is used at
    // the same sizes
    fontgen::generate_ui(
        &assets.join("ui-regular.ttf"),
        &assets.join("ui-bold.ttf"),
        &mono,
        &out_dir,
    );

    // The interface's pictures, from their published paths
    icongen::generate(&assets.join("icons-lucide"), &out_dir);

    println!("cargo:rerun-if-changed=fontgen.rs");
    println!("cargo:rerun-if-changed=icongen.rs");
    // The inputs too: a different TTF must rebuild the glyph tables, which
    // cargo cannot infer from the generator's source
    println!("cargo:rerun-if-changed={}", assets.display());
    println!("cargo:rerun-if-changed=build.rs");
}
