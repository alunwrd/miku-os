// Volumes judged by dosfstools and mtools
//
// `format.rs` is this code read back by its own arithmetic. These are the
// readers a firmware resembles.
//
// Skipped when the tools are not installed.

use fatfs::{format, MemDisk};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const LABEL: &[u8; 11] = b"MIKUOS ESP ";

fn have(tool: &str, flag: &str) -> bool {
    Command::new(tool).arg(flag).output().is_ok()
}

fn dump(image: &[u8], name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("fatfs-test-{name}-{}.img", std::process::id()));
    let mut f = std::fs::File::create(&path).expect("cannot create image file");
    f.write_all(image).expect("cannot write image");
    path
}

/// Removes the image unless FATFS_KEEP_IMAGES is set
fn cleanup(path: &Path) {
    if std::env::var_os("FATFS_KEEP_IMAGES").is_some() {
        eprintln!("kept image: {}", path.display());
        return;
    }
    let _ = std::fs::remove_file(path);
}

/// `fsck.fat -n`: checks, changes nothing. 0 is a clean volume
fn fsck(path: &Path) -> (i32, String) {
    let out = Command::new("fsck.fat").arg("-n").arg(path).output().expect("fsck.fat did not run");
    let mut report = String::from_utf8_lossy(&out.stdout).into_owned();
    report.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), report)
}

fn payload(len: usize) -> Vec<u8> {
    (0..len as u32).map(|i| (i ^ i >> 7) as u8).collect()
}

/// A volume shaped like the installer's, of `sectors` sectors
fn esp(sectors: u64, loader: &[u8]) -> Vec<u8> {
    let mut disk = MemDisk::new(sectors as usize);
    let mut vol = format(&mut disk, 0, sectors, LABEL, 0xC0FF_EE00).unwrap();
    let efi = vol.mkdir(vol.root(), "EFI").unwrap();
    let boot = vol.mkdir(efi, "BOOT").unwrap();
    vol.write_file(boot, "BOOTX64.EFI", loader).unwrap();
    vol.write_file(vol.root(), "EMPTY", &[]).unwrap();
    vol.finish().unwrap();
    disk.image_flushed().to_vec()
}

#[test]
fn fsck_fat_accepts_the_volume() {
    if !have("fsck.fat", "--help") {
        eprintln!("dosfstools not installed; skipping");
        return;
    }
    // One sector a cluster, eight, and the smallest FAT32 there is
    for (name, sectors) in [("esp", 204_800u64), ("spc8", 620_000), ("min", 66_601)] {
        let path = dump(&esp(sectors, &payload(851_968)), name);
        let (code, report) = fsck(&path);
        cleanup(&path);
        assert_eq!(code, 0, "fsck.fat rejects the {name} volume (exit {code}):\n{report}");
    }
}

#[test]
fn fsck_fat_accepts_a_full_volume() {
    if !have("fsck.fat", "--help") {
        eprintln!("dosfstools not installed; skipping");
        return;
    }
    const SECTORS: u64 = 66_601;
    let mut disk = MemDisk::new(SECTORS as usize);
    let mut vol = format(&mut disk, 0, SECTORS, LABEL, 1).unwrap();
    let clusters = vol.geometry().clusters as usize;
    vol.write_file(vol.root(), "FILL", &payload((clusters - 1) * 512)).unwrap();
    vol.finish().unwrap();
    let path = dump(disk.image_flushed(), "full");
    let (code, report) = fsck(&path);
    cleanup(&path);
    assert_eq!(code, 0, "fsck.fat rejects a full volume (exit {code}):\n{report}");
}

#[test]
fn mtools_reads_the_loader_back() {
    if !have("mdir", "-V") || !have("mcopy", "-V") {
        eprintln!("mtools not installed - skipping");
        return;
    }
    let loader = payload(851_968);
    let path = dump(&esp(204_800, &loader), "mtools");
    // A stray ~/.mtoolsrc must not decide what the image is
    let mtools = |tool: &str| {
        let mut c = Command::new(tool);
        c.env("MTOOLSRC", "/dev/null").env("MTOOLS_SKIP_CHECK", "0");
        c
    };

    let listing = mtools("mdir").arg("-i").arg(&path).arg("::/EFI/BOOT").output().expect("mdir did not run");
    let text = String::from_utf8_lossy(&listing.stdout).into_owned();
    let errors = String::from_utf8_lossy(&listing.stderr).into_owned();

    let mut out = std::env::temp_dir();
    out.push(format!("fatfs-test-loader-{}.efi", std::process::id()));
    let copy = mtools("mcopy")
        .arg("-n")
        .arg("-i")
        .arg(&path)
        .arg("::/EFI/BOOT/BOOTX64.EFI")
        .arg(&out)
        .output()
        .expect("mcopy did not run");
    let read = std::fs::read(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    cleanup(&path);

    assert!(listing.status.success(), "mdir failed:\n{text}{errors}");
    assert!(text.contains("BOOTX64  EFI"), "mdir does not list the loader:\n{text}");
    assert!(text.contains("851968"), "mdir shows another size:\n{text}");
    assert!(copy.status.success(), "mcopy failed: {}", String::from_utf8_lossy(&copy.stderr));
    assert!(read == loader, "mcopy read back {} bytes that differ from the {} written", read.len(), loader.len());
}
