//! The environment.
//!
//! `libmiku` keeps it as a growable array of heap-allocated `"KEY=VALUE"` C
//! strings, the shape `execve` wants, and `miku_environ()` hands that array
//! over. `sys::pal::init` seeds it from the initial stack; the loader fills in
//! no `environ` symbol.

use crate::ffi::{CStr, OsStr, OsString};
use crate::io;
use crate::os::miku::ffi::{OsStrExt, OsStringExt};
use crate::sys::pal::abi;
use crate::{fmt, vec};

pub struct Env {
    iter: vec::IntoIter<(OsString, OsString)>,
}

impl fmt::Debug for Env {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter.as_slice()).finish()
    }
}

impl Iterator for Env {
    type Item = (OsString, OsString);
    fn next(&mut self) -> Option<(OsString, OsString)> {
        self.iter.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

/// A snapshot. The array `miku_environ` returns is the storage, valid until the
/// next `setenv`, so everything is copied
pub fn env() -> Env {
    let mut out: Vec<(OsString, OsString)> = Vec::new();
    let environ = abi::miku_environ();
    if !environ.is_null() {
        let mut i = 0usize;
        loop {
            // SAFETY: the array is NUL-terminated, as `execve` requires and
            // `libmiku` maintains
            let e = unsafe { *environ.add(i) };
            if e.is_null() {
                break;
            }
            i += 1;
            let bytes = unsafe { CStr::from_ptr(e.cast()) }.to_bytes();
            if let Some(eq) = bytes.iter().position(|&c| c == b'=') {
                if eq == 0 {
                    continue; // an entry with no key is not an entry
                }
                out.push((
                    OsString::from_vec(bytes[..eq].to_vec()),
                    OsString::from_vec(bytes[eq + 1..].to_vec()),
                ));
            }
        }
    }
    Env { iter: out.into_iter() }
}

/// A NUL-terminated copy for the kernel and `libmiku`
fn cstr(s: &OsStr) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.iter().any(|&c| c == 0) {
        return None;
    }
    let mut v = Vec::with_capacity(b.len() + 1);
    v.extend_from_slice(b);
    v.push(0);
    Some(v)
}

pub fn getenv(k: &OsStr) -> Option<OsString> {
    let k = cstr(k)?;
    let p = abi::miku_getenv(k.as_ptr());
    if p.is_null() {
        return None;
    }
    // Copied out at once. The pointer into the stored entry stays valid only
    // until that variable is next set
    let bytes = unsafe { CStr::from_ptr(p.cast()) }.to_bytes().to_vec();
    Some(OsString::from_vec(bytes))
}

/// # Safety
/// As `std::env::set_var`: not safe to call while another thread may be
/// reading the environment.
pub unsafe fn setenv(k: &OsStr, v: &OsStr) -> io::Result<()> {
    let (k, v) = match (cstr(k), cstr(v)) {
        (Some(k), Some(v)) => (k, v),
        _ => {
            return Err(io::const_error!(
                io::ErrorKind::InvalidInput,
                "environment variable name or value contains a NUL byte",
            ));
        }
    };
    if abi::miku_setenv(k.as_ptr(), v.as_ptr()) {
        Ok(())
    } else {
        Err(io::const_error!(io::ErrorKind::InvalidInput, "could not set the variable"))
    }
}

/// # Safety
/// As `setenv`.
pub unsafe fn unsetenv(k: &OsStr) -> io::Result<()> {
    let Some(k) = cstr(k) else {
        return Err(io::const_error!(
            io::ErrorKind::InvalidInput,
            "environment variable name contains a NUL byte",
        ));
    };
    // Removing an absent variable is not a failure, as POSIX says
    abi::miku_unsetenv(k.as_ptr());
    Ok(())
}
