use crate::fs::vfs::types::*;

#[derive(Clone, Copy)]
pub struct SecurityLabel {
    pub vnode_id: InodeId,
    pub label: [u8; 16],
    pub label_len: u8,
    pub active: bool,
}

impl SecurityLabel {
    pub const fn empty() -> Self {
        Self {
            vnode_id: INVALID_ID,
            label: [0; 16],
            label_len: 0,
            active: false,
        }
    }
}

pub struct SecurityManager {
    pub labels: [SecurityLabel; MAX_SECURITY_LABELS],
    pub enforcing: bool,
}

impl SecurityManager {
    pub const fn new() -> Self {
        Self {
            labels: [SecurityLabel::empty(); MAX_SECURITY_LABELS],
            enforcing: false,
        }
    }

    pub fn set_label(&mut self, vnode_id: InodeId, label: &[u8]) -> VfsResult<()> {
        if label.len() > 16 {
            return Err(VfsError::InvalidArgument);
        }

        for sl in self.labels.iter_mut() {
            if sl.active && sl.vnode_id == vnode_id {
                let len = label.len();
                sl.label[..len].copy_from_slice(label);
                sl.label_len = len as u8;
                return Ok(());
            }
        }

        for sl in self.labels.iter_mut() {
            if !sl.active {
                sl.vnode_id = vnode_id;
                let len = label.len();
                sl.label[..len].copy_from_slice(label);
                sl.label_len = len as u8;
                sl.active = true;
                return Ok(());
            }
        }

        Err(VfsError::NoSpace)
    }

    pub fn get_label(&self, vnode_id: InodeId) -> Option<&[u8]> {
        for sl in &self.labels {
            if sl.active && sl.vnode_id == vnode_id {
                return Some(&sl.label[..sl.label_len as usize]);
            }
        }
        None
    }

    pub fn remove_label(&mut self, vnode_id: InodeId) {
        for sl in self.labels.iter_mut() {
            if sl.active && sl.vnode_id == vnode_id {
                *sl = SecurityLabel::empty();
            }
        }
    }

    pub fn check_access(
        &self,
        vnode_id: InodeId,
        cred: &Credentials,
        _access: crate::fs::vfs::permissions::AccessMode,
    ) -> bool {
        if !self.enforcing {
            return true;
        }
        if cred.is_root() {
            return true;
        }
        match self.get_label(vnode_id) {
            Some(label) => {
                if label == b"restricted" {
                    return false;
                }
                true
            }
            None => true,
        }
    }
}
