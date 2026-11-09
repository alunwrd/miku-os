//! The export scanner and the declaration check, on a library made in a temp dir

use std::fs;
use std::path::PathBuf;

use miku_buildgen::{check, decls, exports, libs};

fn tree(name: &str, module: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("miku-buildgen-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("core")).unwrap();
    fs::write(root.join("libs.list"), "core_miku:\n").unwrap();
    fs::write(root.join("core/main.rs"), "").unwrap();
    fs::write(root.join("core/io.rs"), module).unwrap();
    root
}

const MODULE: &str = r#"
/// Writes bytes
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_write(fd: u64, buf: *const u8, len: usize) -> i64 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn miku_grid(
    mut r: *mut [u64; 16],
    cb: extern "C" fn(*const u8, *const u8) -> i32,
) {
}

#[no_mangle]
pub unsafe extern "C" fn miku_fmt(fmt: *const u8, mut args: ...) -> i32 {
    0
}
"#;

#[test]
fn signatures_read_from_source() {
    let root = tree("scan", MODULE);
    let e = exports::scan(&root.join("core/io.rs"));
    assert_eq!(e.len(), 3);
    assert_eq!(e[0].name, "miku_write");
    assert_eq!(e[0].docs, ["/// Writes bytes"]);
    assert_eq!(e[0].ret, "i64");
    assert_eq!(e[0].args.len(), 3);
    assert_eq!(e[1].args[0], ("r".to_string(), "*mut [u64; 16]".to_string()));
    assert_eq!(e[1].args.len(), 2);
    assert!(e[2].is_variadic());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn declarations_are_generated_with_their_docs() {
    let root = tree("decls", MODULE);
    let list = libs::read(&root);
    let out = decls::generate(&root, &list);
    assert!(out.contains("/// Writes bytes"), "{out}");
    assert!(out.contains("pub fn miku_write(fd: u64, buf: *const u8, len: usize) -> i64;"), "{out}");
    assert!(out.contains("pub fn miku_fmt(fmt: *const u8, ...) -> i32;"), "{out}");
    let _ = fs::remove_dir_all(root);
}

fn decl_file(root: &std::path::Path, text: &str) -> PathBuf {
    let p = root.join("decl.rs");
    fs::write(&p, text).unwrap();
    p
}

#[test]
fn a_matching_declaration_passes() {
    let root = tree("ok", MODULE);
    let f = decl_file(&root, "unsafe extern \"C\" {\n    pub safe fn miku_write(fd: u64, buf: *const u8, len: usize) -> i64;\n}\n");
    assert!(check::declarations(&root, &[&f]).is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_wrong_argument_count_is_refused() {
    let root = tree("count", MODULE);
    let f = decl_file(&root, "extern \"C\" {\n    pub fn miku_write(fd: u64, buf: *const u8) -> i64;\n}\n");
    let e = check::declarations(&root, &[&f]).unwrap_err();
    assert!(e.contains("miku_write takes 2 arguments, the library's 3"), "{e}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_wrong_width_is_refused() {
    let root = tree("width", MODULE);
    let f = decl_file(&root, "extern \"C\" {\n    pub fn miku_write(fd: c_int, buf: *const u8, len: usize) -> i64;\n}\n");
    let e = check::declarations(&root, &[&f]).unwrap_err();
    assert!(e.contains("argument 0"), "{e}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_cycle_in_the_list_is_refused() {
    let root = tree("cycle", MODULE);
    fs::create_dir_all(root.join("sys")).unwrap();
    fs::write(root.join("sys/main.rs"), "").unwrap();
    fs::write(root.join("libs.list"), "core_miku: sys_miku\nsys_miku: core_miku\n").unwrap();
    let list = libs::read(&root);
    let r = std::panic::catch_unwind(|| libs::validate(&list, &root));
    assert!(r.is_err());
    let _ = fs::remove_dir_all(root);
}
