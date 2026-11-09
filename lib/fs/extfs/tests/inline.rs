// Inline data from a real filesystem
//
// A file fitting in its inode has no blocks: `i_block` holds the first sixty
// bytes and the rest is in the extended-attribute area as `system.data`. Made
// by `mke2fs -d`, which inlines small files.

use extfs::{MemDisk, MikuFS};
use std::process::Command;

/// Contents of the nth reference file: n bytes of a pattern
fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

const SIZES: [usize; 4] = [40, 80, 100, 120];

/// An ext4 image with inline_data and small files. `None` when e2fsprogs is not
/// installed
fn inline_image(tag: &str) -> Option<Vec<u8>> {
    if Command::new("mke2fs").arg("-V").output().is_err() {
        eprintln!("mke2fs not installed - skipping");
        return None;
    }
    let mut dir = std::env::temp_dir();
    dir.push(format!("extfs-inline-src-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    for n in SIZES {
        std::fs::write(dir.join(format!("f{n}.bin")), pattern(n)).ok()?;
    }

    let mut path = std::env::temp_dir();
    path.push(format!("extfs-inline-{tag}-{}.img", std::process::id()));
    let ok = Command::new("mke2fs")
        .args([
            "-q", "-F", "-t", "ext4", "-b", "1024", "-I", "256",
            "-O", "inline_data,metadata_csum,extent,^has_journal,^64bit",
        ])
        .arg("-d")
        .arg(&dir)
        .arg(&path)
        .arg("4096")
        .status()
        .ok()?;
    assert!(ok.success(), "mke2fs failed to build the inline_data sample");
    let image = std::fs::read(&path).expect("cannot read the image mke2fs made");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&dir);
    Some(image)
}

#[test]
fn the_sample_really_has_inline_files() {
    let Some(image) = inline_image("present") else { return };
    let mut disk = MemDisk::from_image(image);
    let mut fs = MikuFS::open(&mut disk, 0, false).expect("cannot mount");

    for n in SIZES {
        let name = format!("f{n}.bin");
        let ino = fs
            .ext2_lookup_in_dir(2, &name)
            .unwrap()
            .unwrap_or_else(|| panic!("{name} is missing from the image"));
        let inode = fs.read_inode(ino).unwrap();
        assert!(
            inode.has_inline_data(),
            "{name} is not inline - the sample proves nothing"
        );
        assert_eq!(inode.size(), n as u64);
    }
}

/// The whole file
#[test]
fn an_inline_file_reads_in_full() {
    let Some(image) = inline_image("full") else { return };
    let mut disk = MemDisk::from_image(image);
    let mut fs = MikuFS::open(&mut disk, 0, false).expect("cannot mount");

    for n in SIZES {
        let name = format!("f{n}.bin");
        let ino = fs.ext2_lookup_in_dir(2, &name).unwrap().unwrap();
        let inode = fs.read_inode(ino).unwrap();
        let mut buf = vec![0u8; n + 16];
        let got = fs
            .read_file(&inode, 0, &mut buf)
            .unwrap_or_else(|e| panic!("read {name}: {e:?}"));
        assert_eq!(got, n, "{name} read short");
        assert_eq!(&buf[..n], &pattern(n)[..], "{name} read the wrong bytes");
    }
}

/// Reading from part way in
#[test]
fn an_inline_file_reads_from_an_offset() {
    let Some(image) = inline_image("offset") else { return };
    let mut disk = MemDisk::from_image(image);
    let mut fs = MikuFS::open(&mut disk, 0, false).expect("cannot mount");

    let name = "f120.bin";
    let ino = fs.ext2_lookup_in_dir(2, name).unwrap().unwrap();
    let inode = fs.read_inode(ino).unwrap();
    let want = pattern(120);

    for at in [0usize, 1, 59, 60, 61, 100, 119] {
        let mut buf = vec![0u8; 120];
        let got = fs.read_file(&inode, at as u64, &mut buf).unwrap();
        assert_eq!(got, 120 - at, "short read at offset {at}");
        assert_eq!(&buf[..got], &want[at..], "wrong bytes at offset {at}");
    }
    // And past the end
    let mut buf = [0u8; 16];
    assert_eq!(fs.read_file(&inode, 120, &mut buf).unwrap(), 0);
}

/// Writing moves an inline file to blocks, including the xattr part
#[test]
fn writing_converts_without_losing_the_tail() {
    let Some(image) = inline_image("convert") else { return };
    let mut disk = MemDisk::from_image(image);
    let mut fs = MikuFS::open(&mut disk, 0, false).expect("cannot mount");

    let name = "f120.bin";
    let ino = fs.ext2_lookup_in_dir(2, name).unwrap().unwrap();
    let appended = b"appended past the end of the inode";
    fs.ext4_write_file(ino, appended, 120).expect("write failed");

    let inode = fs.read_inode(ino).unwrap();
    assert!(!inode.has_inline_data(), "the file should have left the inode");
    assert_eq!(inode.size(), 120 + appended.len() as u64);

    let mut buf = vec![0u8; 120 + appended.len()];
    let got = fs.read_file(&inode, 0, &mut buf).unwrap();
    assert_eq!(got, buf.len());
    assert_eq!(&buf[..120], &pattern(120)[..], "the original content was lost");
    assert_eq!(&buf[120..], appended);
}
