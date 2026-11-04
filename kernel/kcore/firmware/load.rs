// On-demand firmware loader, modelled on Linux's request_firmware /
// release_firmware
//
// Firmware is under /lib/firmware, as on Linux, on the store init() mounts (the
// GRUB module), and is read through the normal VFS. init() grafts /lib/firmware
// onto the VFS; request("nvidia/tu116/x") opens /lib/firmware/nvidia/tu116/x.
// The buffer is freed when the caller drops the returned Firmware (the
// release_firmware half), so a blob touched once during GPU bring-up is not
// pinned in RAM

use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::vfs::VfsError;
use crate::fs::vfs::core::with_vfs;

/// VFS mount point for the firmware tree.
const FW_ROOT: &str = "/lib/firmware";

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FwError {
    /// /lib/firmware is not mounted (no root disk, or no firmware on it).
    NotMounted,
    /// The firmware tree is mounted but does not contain the requested path.
    NotFound,
    /// Allocation for the file buffer failed
    TooLarge,
    /// A disk read failed partway through
    Io,
}

/// An owned firmware image. Dropping it releases the memory: the
/// release_firmware half of the request_firmware contract
pub struct Firmware {
    path: String,
    data: Vec<u8>,
}

impl Firmware {
    #[inline]
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.data.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    #[inline]
    pub fn path(&self) -> &str {
        &self.path
    }
}

/// Mount the root disk and graft its /lib/firmware onto the VFS. Non-fatal: if
/// the disk has no ext filesystem or no /lib/firmware, the loader stays offline
/// and every request() returns NotMounted, which the GPU bring-up reports as
/// missing firmware
pub fn init() -> Result<(), &'static str> {
    if !crate::fs::ext::mount::mount_root_disk() {
        crate::serial_println!("[fwload] no ext root disk; firmware unavailable");
        return Ok(());
    }

    // /bin holds the ring-3 programs (the userspace shell among them). Grafted
    // here, where the root filesystem is known to be mounted; otherwise nothing
    // outside /lib/firmware on the disk is reachable through the VFS
    crate::fs::ext::mount::graft_root_dir("bin");

    // /etc, with the desktop's config and the account files. `open` falls back
    // to resolving a path on the mounted ext filesystem directly, but listing
    // needs the graft
    crate::fs::ext::mount::graft_root_dir("etc");

    // /pkg: the record of every installed package, the download cache, the
    // transaction journal. Grafted as /etc is: a package manager reads every
    // record in /pkg/db by listing it, and an empty listing would report
    // nothing installed
    crate::fs::ext::mount::graft_root_dir("pkg");

    prefer_persistent_root();

    // Resolve the firmware directory on the disk itself.
    let dir_ino = crate::fs::ext::mount::with_ext2_pub(|fs| {
        fs.resolve_path(FW_ROOT).ok()
    }).flatten();

    let dir_ino = match dir_ino {
        Some(ino) => ino,
        None => {
            crate::serial_println!(
                "[fwload] {} not present on root disk; firmware unavailable", FW_ROOT
            );
            return Ok(());
        }
    };

    // Graft disk:/lib/firmware onto VFS /lib/firmware (under the tmpfs /lib)
    let ok = with_vfs(|v| v.graft_ext_dir("/lib", "firmware", dir_ino).is_ok());
    if ok {
        crate::serial_println!("[fwload] {} mounted from root disk", FW_ROOT);
    } else {
        crate::serial_println!("[fwload] failed to graft {}", FW_ROOT);
    }
    Ok(())
}

/// True if the firmware tree is mounted and backed by the disk
pub fn available() -> bool {
    with_vfs(|v| {
        v.resolve_path(0, FW_ROOT)
            .map(|id| v.nodes[id].is_ext_backed())
            .unwrap_or(false)
    })
}

fn abs_path(rel: &str) -> String {
    let rel = rel.trim_start_matches('/');
    let mut s = String::with_capacity(FW_ROOT.len() + 1 + rel.len());
    s.push_str(FW_ROOT);
    s.push('/');
    s.push_str(rel);
    s
}

fn map_err(e: VfsError) -> FwError {
    match e {
        VfsError::NotFound | VfsError::NotDirectory | VfsError::IsDirectory => FwError::NotFound,
        VfsError::NoSpace => FwError::TooLarge,
        _ => FwError::Io,
    }
}

/// Stat a firmware file's size without reading its data. Used by diagnostics
/// so listing the bundle does not pull megabytes off disk
pub fn size_of(rel: &str) -> Option<u64> {
    let path = abs_path(rel);
    with_vfs(|v| v.path_size(&path).ok())
}

/// Read a firmware file into an owned buffer (the request_firmware half),
/// through the normal VFS path. Freed when the returned Firmware is dropped
pub fn request(rel: &str) -> Result<Firmware, FwError> {
    let path = abs_path(rel);
    let data = with_vfs(|v| v.read_path(&path)).map_err(map_err)?;
    Ok(Firmware {
        path: String::from(rel),
        data,
    })
}

/// Like request(), but on any failure logs the miss and returns an empty
/// Firmware. Driver call sites keep the shape 'let blob = fw.bytes();' then
/// 'NvfwBinHdr::parse(blob).ok_or(..)?', where an empty slice drives the
/// caller's own missing/invalid firmware error path
pub fn request_or_empty(rel: &str) -> Firmware {
    match request(rel) {
        Ok(fw) => fw,
        Err(e) => {
            crate::serial_println!("[fwload] miss: {}/{} ({:?})", FW_ROOT, rel, e);
            Firmware {
                path: String::from(rel),
                data: Vec::new(),
            }
        }
    }
}


/// Take /bin and /etc from a real disk when the boot line names one.
///
/// The store mounted above is the GRUB module: a RAM disk, read-only, gone at
/// reboot. Right for firmware blobs and wrong for /etc with the accounts:
/// `passwd` would appear to work and lose the new password at the next boot.
///
/// It needs asking for: picking a disk and writing to it is not the kernel's
/// decision. So this happens only with `root=` on the boot line, and the
/// filesystem is writable only with `rootrw` next to it, as `mount_root_disk`
/// understands. Without them /etc is the read-only copy.
///
///     root=0:0 rootrw     /bin and /etc from drive 0, writable
///     root=LABEL=miku     the same by label, read-only
fn prefer_persistent_root() {
    let mut probe = [0u8; 64];
    if crate::arch::x86_64::grub::cmdline_arg("root", &mut probe).is_none() {
        crate::serial_println!(
            "[fwload] /etc is the read-only boot copy; add 'root=<drive>:<lba> rootrw' \
             to keep account changes across a reboot"
        );
        return;
    }

    // mount_root_disk returned early on the module, so the disk named by root=
    // has not been looked at. try_mount_ro/try_mount refuse a device that is
    // not an ext filesystem
    if !crate::fs::ext::mount::mount_root_from_cmdline() {
        crate::serial_println!("[fwload] root= named a disk that could not be mounted; \
                                keeping the boot copy of /etc");
        return;
    }

    // Re-graft from the disk. graft_ext_dir replaces the entry, so the module's
    // graft is superseded, not duplicated; /lib/firmware is left alone, as the
    // blobs are in the module. `pkg` too: a persistent root is the one boot
    // where an installation survives a reboot
    for dir in ["bin", "etc", "pkg"] {
        if crate::fs::ext::mount::graft_root_dir(dir) {
            crate::serial_println!("[fwload] /{} now comes from the disk named by root=", dir);
        }
    }
}
