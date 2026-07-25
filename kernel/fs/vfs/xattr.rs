use crate::fs::vfs::types::*;

const XATTR_NAME_LEN: usize = 16;
const XATTR_VAL_LEN: usize = 32;

#[derive(Clone, Copy)]
pub struct Xattr {
    pub name: [u8; XATTR_NAME_LEN],
    pub name_len: u8,
    pub value: [u8; XATTR_VAL_LEN],
    pub value_len: u8,
    pub active: bool,
}

impl Xattr {
    pub const fn empty() -> Self {
        Self {
            name: [0; XATTR_NAME_LEN],
            name_len: 0,
            value: [0; XATTR_VAL_LEN],
            value_len: 0,
            active: false,
        }
    }

    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }

    pub fn name_matches(&self, name: &str) -> bool {
        let bytes = name.as_bytes();
        self.name_len as usize == bytes.len() && &self.name[..self.name_len as usize] == bytes
    }
}

pub struct XattrStore {
    pub attrs: [Xattr; MAX_XATTRS_PER_NODE],
}

impl XattrStore {
    pub const fn new() -> Self {
        Self {
            attrs: [Xattr::empty(); MAX_XATTRS_PER_NODE],
        }
    }

    pub fn set(&mut self, name: &str, value: &[u8]) -> VfsResult<()> {
        if name.len() > XATTR_NAME_LEN {
            return Err(VfsError::NameTooLong);
        }
        if value.len() > XATTR_VAL_LEN {
            return Err(VfsError::XattrTooLarge);
        }

        for attr in self.attrs.iter_mut() {
            if attr.active && attr.name_matches(name) {
                let vlen = value.len();
                attr.value[..vlen].copy_from_slice(value);
                attr.value_len = vlen as u8;
                return Ok(());
            }
        }

        for attr in self.attrs.iter_mut() {
            if !attr.active {
                let nb = name.as_bytes();
                let nlen = nb.len();
                attr.name[..nlen].copy_from_slice(nb);
                attr.name_len = nlen as u8;
                let vlen = value.len();
                attr.value[..vlen].copy_from_slice(value);
                attr.value_len = vlen as u8;
                attr.active = true;
                return Ok(());
            }
        }

        Err(VfsError::NoSpace)
    }

    pub fn get(&self, name: &str) -> VfsResult<&[u8]> {
        for attr in &self.attrs {
            if attr.active && attr.name_matches(name) {
                return Ok(&attr.value[..attr.value_len as usize]);
            }
        }
        Err(VfsError::NotFound)
    }

    pub fn remove(&mut self, name: &str) -> VfsResult<()> {
        for attr in self.attrs.iter_mut() {
            if attr.active && attr.name_matches(name) {
                *attr = Xattr::empty();
                return Ok(());
            }
        }
        Err(VfsError::NotFound)
    }

    pub fn list_names(&self, buf: &mut [[u8; XATTR_NAME_LEN]], lens: &mut [u8]) -> usize {
        let mut count = 0;
        for attr in &self.attrs {
            if attr.active && count < buf.len() {
                buf[count][..attr.name_len as usize]
                    .copy_from_slice(&attr.name[..attr.name_len as usize]);
                lens[count] = attr.name_len;
                count += 1;
            }
        }
        count
    }

    pub fn count(&self) -> usize {
        self.attrs.iter().filter(|a| a.active).count()
    }

    pub fn clear(&mut self) {
        for attr in self.attrs.iter_mut() {
            *attr = Xattr::empty();
        }
    }
}
