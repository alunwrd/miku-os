//! Compiles `c/stdc.c` through `cc`, which finds `lib/sdk/c/miku-cc` via
//! `CC_x86_64_unknown_miku`

fn main() {
    println!("cargo:rerun-if-changed=c/stdc.c");
    cc::Build::new().file("c/stdc.c").opt_level(2).warnings(true).compile("stdc");
}
