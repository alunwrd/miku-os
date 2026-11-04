// Where an ELF image's bytes come from while it is loaded
//
// A source is read by range, so a large program is not held whole in the kernel
// heap during its own exec. The loader asks for what it needs: the headers, the
// notes, `PT_INTERP`, the TLS image, and each `PT_LOAD` run straight into the
// frames it fills. Small reads in sequence (the dynamic table, relocations) go
// through a `Window`, so each is not its own trip through the filesystem.
//
// Two kinds exist. A byte slice, for images already in memory: the embedded
// `ld-miku.so`, the vDSO, `elf_test`'s hostile images, and what the whole-file
// path still reads. And an open file: `FileSource` holds a descriptor for the
// length of the load, which keeps its vnode from being reclaimed, and reads
// through `read_at_vnode`, the path a file-backed `mmap` fills its pages from.

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

pub trait ElfSource {
    /// The image's size in bytes
    fn len(&self) -> usize;

    /// Copy up to `out.len()` bytes starting at `off`; how many there were.
    /// Short only at the end of the image (or on an I/O error, which the
    /// loader treats the same way: bytes it did not get are zero)
    fn read_at(&self, off: usize, out: &mut [u8]) -> usize;
}

impl ElfSource for [u8] {
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn read_at(&self, off: usize, out: &mut [u8]) -> usize {
        if off >= <[u8]>::len(self) {
            return 0;
        }
        let n = out.len().min(<[u8]>::len(self) - off);
        out[..n].copy_from_slice(&self[off..off + n]);
        n
    }
}

impl ElfSource for Vec<u8> {
    fn len(&self) -> usize {
        self.as_slice().len()
    }

    fn read_at(&self, off: usize, out: &mut [u8]) -> usize {
        self.as_slice().read_at(off, out)
    }
}

/// `len` bytes at `off`, owned, for the pieces that are parsed rather than
/// copied into a process (headers, notes, the interpreter's path). `None` if
/// the image does not have them all
pub fn read_vec<S: ElfSource + ?Sized>(src: &S, off: usize, len: usize) -> Option<Vec<u8>> {
    let end = off.checked_add(len)?;
    if end > src.len() {
        return None;
    }
    let mut v = vec![0u8; len];
    if src.read_at(off, &mut v) != len {
        return None;
    }
    Some(v)
}

/// How much one `read_at_vnode` call carries. The VFS lock is held for the
/// length of each call, across the disk read under it, so this bounds how
/// long a large exec keeps every other filesystem call waiting
const FILE_CHUNK: usize = 256 * 1024;

/// A regular file, open for the length of the load
pub struct FileSource {
    fd: usize,
    vid: usize,
    size: usize,
}

impl FileSource {
    /// `path` from the root, opened for reading like any other open: the
    /// permission check, the vnode's reference and, on drop, the close are the
    /// VFS's. `None` if the VFS has no such regular file
    pub fn open(path: &str) -> Option<Self> {
        use crate::fs::vfs::types::{FileMode, OpenFlags};
        crate::fs::vfs::core::with_vfs(|vfs| {
            // Close-on-exec as well as closed on drop: the descriptor sits in
            // the caller's table, and an exec hands that table on
            let flags = OpenFlags(OpenFlags::READ | OpenFlags::CLOEXEC);
            let fd = vfs.open(0, path, flags, FileMode::default_file()).ok()?;
            let backing = vfs.fd_backing(fd);
            match backing {
                Some((vid, size)) if vfs.nodes[vid as usize].is_regular() => {
                    Some(FileSource { fd, vid: vid as usize, size: size as usize })
                }
                _ => {
                    let _ = vfs.close(fd);
                    None
                }
            }
        })
    }
}

impl Drop for FileSource {
    fn drop(&mut self) {
        let fd = self.fd;
        crate::fs::vfs::core::with_vfs(|vfs| {
            let _ = vfs.close(fd);
        });
    }
}

impl ElfSource for FileSource {
    fn len(&self) -> usize {
        self.size
    }

    fn read_at(&self, off: usize, out: &mut [u8]) -> usize {
        if off >= self.size {
            return 0;
        }
        let want = out.len().min(self.size - off);
        let mut done = 0;
        while done < want {
            let n = (want - done).min(FILE_CHUNK);
            let got = match crate::fs::vfs::core::read_at_vnode(
                self.vid,
                (off + done) as u64,
                &mut out[done..done + n],
            ) {
                Ok(g) => g,
                Err(_) => break,
            };
            if got == 0 {
                break;
            }
            done += got;
        }
        done
    }
}

/// Little-endian words read in sequence through a buffer: the dynamic
/// table and the relocation entries, thousands of 8-byte reads that are
/// mostly next to each other
pub struct Window<'a, S: ElfSource + ?Sized> {
    src: &'a S,
    base: usize,
    len: usize,
    buf: Vec<u8>,
}

const WINDOW: usize = 64 * 1024;

impl<'a, S: ElfSource + ?Sized> Window<'a, S> {
    pub fn new(src: &'a S) -> Self {
        Window { src, base: 0, len: 0, buf: vec![0u8; WINDOW] }
    }

    fn bytes<const N: usize>(&mut self, off: usize) -> Option<[u8; N]> {
        let end = off.checked_add(N)?;
        if end > self.src.len() {
            return None;
        }
        if off < self.base || end > self.base + self.len {
            self.base = off;
            self.len = self.src.read_at(off, &mut self.buf);
            if self.len < N {
                return None;
            }
        }
        let at = off - self.base;
        self.buf[at..at + N].try_into().ok()
    }

    pub fn u16_at(&mut self, off: usize) -> Option<u16> {
        self.bytes::<2>(off).map(u16::from_le_bytes)
    }

    pub fn u64_at(&mut self, off: usize) -> Option<u64> {
        self.bytes::<8>(off).map(u64::from_le_bytes)
    }

    pub fn i64_at(&mut self, off: usize) -> Option<i64> {
        self.bytes::<8>(off).map(i64::from_le_bytes)
    }
}
