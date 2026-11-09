//! The working directory, and the places a program is told about.

use crate::ffi::{CStr, OsString};
use crate::io;
use crate::os::miku::ffi::{OsStrExt, OsStringExt};
use crate::path::{Path, PathBuf};
use crate::sys::pal::{abi, cvt};

// `PATH` and its kin, split and joined at `:` as on every unix
const PATH_SEPARATOR: u8 = b':';

// Upstream's unix shape: `impl Iterator` would need `'a` live on drop
// (rust-lang/rust#146045)
pub type SplitPaths<'a> = crate::iter::Map<
    crate::slice::Split<'a, u8, impl FnMut(&u8) -> bool + 'static>,
    impl FnMut(&[u8]) -> PathBuf + 'static,
>;

#[define_opaque(SplitPaths)]
pub fn split_paths(unparsed: &crate::ffi::OsStr) -> SplitPaths<'_> {
    fn is_separator(&b: &u8) -> bool {
        b == PATH_SEPARATOR
    }

    fn into_pathbuf(part: &[u8]) -> PathBuf {
        PathBuf::from(crate::ffi::OsStr::from_bytes(part))
    }

    unparsed.as_bytes().split(is_separator).map(into_pathbuf)
}

#[derive(Debug)]
pub struct JoinPathsError;

pub fn join_paths<I, T>(paths: I) -> Result<OsString, JoinPathsError>
where
    I: Iterator<Item = T>,
    T: AsRef<crate::ffi::OsStr>,
{
    let mut joined = Vec::new();

    for (i, path) in paths.enumerate() {
        let path = path.as_ref().as_bytes();
        if i > 0 {
            joined.push(PATH_SEPARATOR)
        }
        if path.contains(&PATH_SEPARATOR) {
            return Err(JoinPathsError);
        }
        joined.extend_from_slice(path);
    }
    Ok(OsString::from_vec(joined))
}

impl crate::fmt::Display for JoinPathsError {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        write!(f, "path segment contains separator `{}`", char::from(PATH_SEPARATOR))
    }
}

impl crate::error::Error for JoinPathsError {}

pub fn getcwd() -> io::Result<PathBuf> {
    // Generous for a working directory; a path too long for it is an error
    let mut buf = vec![0u8; 1024];
    let p = abi::miku_getcwd(buf.as_mut_ptr(), buf.len());
    if p.is_null() {
        return Err(io::const_error!(io::ErrorKind::Uncategorized, "getcwd failed"));
    }
    let bytes = unsafe { CStr::from_ptr(buf.as_ptr().cast()) }.to_bytes().to_vec();
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

pub fn chdir(p: &Path) -> io::Result<()> {
    let b = p.as_os_str().as_bytes();
    if b.iter().any(|&c| c == 0) {
        return Err(io::const_error!(io::ErrorKind::InvalidInput, "path contains a NUL byte"));
    }
    let mut c = Vec::with_capacity(b.len() + 1);
    c.extend_from_slice(b);
    c.push(0);
    cvt(abi::miku_chdir(c.as_ptr()))?;
    Ok(())
}

/// Where scratch files go: `TMPDIR`, or `/tmp`
pub fn temp_dir() -> PathBuf {
    crate::env::var_os("TMPDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// The running program's own path, from `proc_path`(105)
pub fn current_exe() -> io::Result<PathBuf> {
    let mut buf = vec![0u8; 1024];
    let n = cvt(abi::miku_proc_path(0, buf.as_mut_ptr(), buf.len() as u64))?;
    buf.truncate(n as usize);
    Ok(PathBuf::from(OsString::from_vec(buf)))
}

/// `HOME`, which the kernel puts in a program's environment (`DEFAULT_ENV`)
pub fn home_dir() -> Option<PathBuf> {
    crate::env::var_os("HOME").filter(|s| !s.is_empty()).map(PathBuf::from)
}
