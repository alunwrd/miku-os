// On-demand firmware loader, modelled on Linux's request_firmware /
// release_firmware.
//
// Firmware blobs used to be baked into the kernel image with include_bytes!.
// That bloated the ELF (the GSP-RM image alone is ~29 MiB) and pinned every
// byte in RAM for the whole uptime, even though most blobs are touched once
// during GPU bring-up and never again.
//
// Now firmware lives on the persistent root disk under /lib/firmware, exactly
// like Linux, and is read through the normal VFS - the same resolve_path +
// read machinery every other file uses. There is no dedicated "firmware disk"
// and no magic marker: init() mounts the root disk (disk.img) and grafts its
// /lib/firmware directory onto the VFS, after which request("nvidia/tu116/x")
// just opens /lib/firmware/nvidia/tu116/x. The buffer is freed the moment the
// caller drops the returned Firmware (the release_firmware half).

use alloc::string::String;
use alloc::vec::Vec;

use crate::vfs::VfsError;
use crate::vfs::core::with_vfs;

/// VFS mount point for the firmware tree.
const FW_ROOT: &str = "/lib/firmware";

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FwError {
    /// /lib/firmware is not mounted (no root disk, or no firmware on it).
    NotMounted,
    /// The firmware tree is mounted but does not contain the requested path.
    NotFound,
    /// Allocation for the file buffer failed.
    TooLarge,
    /// A disk read failed partway through.
    Io,
}

/// An owned firmware image. Drop it to release the backing memory - this is
/// the release_firmware half of the request_firmware contract.
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
/// the disk has no ext filesystem or no /lib/firmware, the loader stays
/// offline and every request() returns NotMounted, which the GPU bring-up
/// surfaces as missing firmware rather than crashing the boot.
pub fn init() -> Result<(), &'static str> {
    if !crate::commands::ext2_cmds::mount_root_disk() {
        crate::serial_println!("[fwload] no ext root disk; firmware unavailable");
        return Ok(());
    }

    // Resolve the firmware directory on the disk itself.
    let dir_ino = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
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

/// True if the firmware tree is mounted and backed by the disk.
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
/// so listing the bundle does not pull megabytes off disk.
pub fn size_of(rel: &str) -> Option<u64> {
    let path = abs_path(rel);
    with_vfs(|v| v.path_size(&path).ok())
}

/// Read a firmware file into an owned buffer (the request_firmware half),
/// through the normal VFS path. Freed when the returned Firmware is dropped.
pub fn request(rel: &str) -> Result<Firmware, FwError> {
    let path = abs_path(rel);
    let data = with_vfs(|v| v.read_path(&path)).map_err(map_err)?;
    Ok(Firmware {
        path: String::from(rel),
        data,
    })
}

/// Like request(), but on any failure logs the miss and returns an empty
/// Firmware instead of an error. Lets driver call sites keep their existing
/// shape - 'let blob = fw.bytes();' then 'NvfwBinHdr::parse(blob).ok_or(..)?'
/// - where an empty slice naturally drives the caller's own missing/invalid
/// firmware error path.
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
