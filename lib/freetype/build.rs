// FreeType 2.14.3, the modules a TrueType or CFF face needs, and `shim.c`
//
// `vendor/` is the release tarball, unchanged, cut to those modules (see
// `README.md`). What lib/freetype configures differently is in `config/`. Built
// at -O2 whatever the profile: aosh builds at -Oz, and every glyph goes through
// this.
//
// For MikuOS, the builder's `std` step points `cc` at `lib/c/miku-cc`; on the
// host it is the host's compiler.

const FT: &str = "vendor/freetype-2.14.3";

const SOURCES: &[&str] = &[
    "base/ftsystem.c",
    "base/ftinit.c",
    "base/ftdebug.c",
    "base/ftbase.c",
    "base/ftbbox.c",
    "base/ftbitmap.c",
    "base/ftmm.c",
    "base/ftsynth.c",
    "autofit/autofit.c",
    "truetype/truetype.c",
    "cff/cff.c",
    "psaux/psaux.c",
    "pshinter/pshinter.c",
    "psnames/psnames.c",
    "sfnt/sfnt.c",
    "smooth/smooth.c",
];

fn main() {
    let mut b = cc::Build::new();
    b.include("config")
        .include(format!("{FT}/include"))
        .define("FT2_BUILD_LIBRARY", None)
        .define("FT_CONFIG_OPTIONS_H", "<miku_ftoption.h>")
        .define("FT_CONFIG_MODULES_H", "<miku_ftmodule.h>")
        .opt_level(2)
        .warnings(false);
    for s in SOURCES {
        b.file(format!("{FT}/src/{s}"));
    }
    b.file("src/shim.c");
    b.compile("freetype");
    println!("cargo:rerun-if-changed=config");
    println!("cargo:rerun-if-changed=src/shim.c");
    println!("cargo:rerun-if-changed={FT}");
}
