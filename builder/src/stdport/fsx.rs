//! Tree copying like `rsync -a [--delete]`. Keeps mtimes and skips unchanged
//! files, since cargo fingerprints path dependencies by mtime

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Makes `dst` match `src`. With `delete`, extra files go, except top-level
/// names in `keep`, which are neither copied nor deleted
pub fn mirror(src: &Path, dst: &Path, delete: bool, keep: &[&str]) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    mirror_dir(src, dst, delete, keep, true)
}

fn mirror_dir(src: &Path, dst: &Path, delete: bool, keep: &[&str], top: bool) -> io::Result<()> {
    let kept = |n: &OsString| top && keep.iter().any(|k| n.as_os_str() == *k);
    let mut seen: HashSet<OsString> = HashSet::new();
    for e in fs::read_dir(src)? {
        let e = e?;
        let name = e.file_name();
        if kept(&name) { continue; }
        seen.insert(name.clone());
        let s = e.path();
        let d = dst.join(&name);
        let ft = e.file_type()?;
        if ft.is_symlink() {
            let target = fs::read_link(&s)?;
            if fs::read_link(&d).ok().as_ref() != Some(&target) {
                remove_any(&d)?;
                std::os::unix::fs::symlink(&target, &d)?;
            }
        } else if ft.is_dir() {
            if let Ok(m) = d.symlink_metadata() {
                if !m.is_dir() { remove_any(&d)?; }
            }
            fs::create_dir_all(&d)?;
            mirror_dir(&s, &d, delete, keep, false)?;
        } else {
            copy_if_changed(&s, &d)?;
        }
    }
    if delete {
        for e in fs::read_dir(dst)? {
            let e = e?;
            let name = e.file_name();
            if !seen.contains(&name) && !kept(&name) {
                remove_any(&e.path())?;
            }
        }
    }
    Ok(())
}

/// Same size and mtime means the same file
fn copy_if_changed(s: &Path, d: &Path) -> io::Result<()> {
    let sm = fs::metadata(s)?;
    if let Ok(dm) = d.symlink_metadata() {
        if dm.is_file() && dm.len() == sm.len() && dm.modified().ok() == sm.modified().ok() {
            return Ok(());
        }
        // Replaces rather than writes through: the old file may be read-only, a
        // directory, or a link
        remove_any(d)?;
    }
    fs::copy(s, d)?;
    if let Ok(t) = sm.modified() {
        // futimens on a descriptor opened for reading is enough for the owner
        fs::File::open(d)?.set_modified(t)?;
    }
    Ok(())
}

pub fn remove_any(p: &Path) -> io::Result<()> {
    match p.symlink_metadata() {
        Ok(m) if m.is_dir() => fs::remove_dir_all(p),
        Ok(_) => fs::remove_file(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Every regular file under `dir`, sorted by path, for hashing a tree
pub fn files_sorted(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(t) if t.is_file() => out.push(p),
                _ => {}
            }
        }
    }
    out.sort();
    out
}

/// SHA-256 of paths and contents, as hex, using the package format's
/// implementation
pub fn hash_files(files: &[PathBuf]) -> String {
    let mut all = Vec::new();
    for f in files {
        all.extend_from_slice(f.to_string_lossy().as_bytes());
        all.push(0);
        if let Ok(b) = fs::read(f) {
            all.extend_from_slice(&mikupkg::format::sha256(&b));
        }
    }
    mikupkg::format::sha256(&all).iter().map(|b| format!("{b:02x}")).collect()
}

/// Writes `body` only if it differs, keeping the mtime otherwise
pub fn write_if_changed(path: &Path, body: &str) -> io::Result<()> {
    if fs::read_to_string(path).ok().as_deref() == Some(body) {
        return Ok(());
    }
    if let Some(p) = path.parent() { fs::create_dir_all(p)?; }
    fs::write(path, body)
}
