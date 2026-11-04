//! Directory listing and configuration files, which `Root` does not provide

use alloc::string::String;
use alloc::vec::Vec;

use mikupkg::conf::layers::Files;

/// A directory's kind, as a listing gives it
pub const KIND_DIR: u8 = miku::KIND_DIRECTORY;

/// Directory entries with kinds, following no symlinks (`miku::read_dir`)
pub fn read_dir(path: &str) -> Vec<(String, u8)> {
    miku::read_dir(path)
}

/// The configuration's files, as the layering wants to see them.
pub struct SysFiles;

impl Files for SysFiles {
    fn list(&self, dir: &str) -> Vec<String> {
        read_dir(dir)
            .into_iter()
            .filter(|(_, kind)| *kind != KIND_DIR)
            .map(|(name, _)| name)
            .collect()
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let mut c = Vec::with_capacity(path.len() + 1);
        c.extend_from_slice(path.as_bytes());
        c.push(0);
        let mut size: usize = 0;
        let ptr = unsafe { miku::miku_read_file(c.as_ptr(), &mut size) };
        if ptr.is_null() {
            return None;
        }
        let slice = unsafe { core::slice::from_raw_parts(ptr, size) };
        Some(slice.to_vec())
    }
}
