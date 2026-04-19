use crate::vfs::types::*;

#[derive(Clone, Copy)]
pub struct OpenFile {
    pub vnode_id: InodeId,
    pub flags: OpenFlags,
    pub offset: u64,
    pub active: bool,
}

impl OpenFile {
    pub const fn empty() -> Self {
        Self {
            vnode_id: INVALID_ID,
            flags: OpenFlags::EMPTY,
            offset: 0,
            active: false,
        }
    }

    pub fn readable(&self) -> bool {
        self.flags.readable()
    }
    pub fn writable(&self) -> bool {
        self.flags.writable()
    }
}

pub struct FdTable {
    pub files: [OpenFile; MAX_OPEN_FILES],
}

impl FdTable {
    pub const fn new() -> Self {
        Self {
            files: [OpenFile::empty(); MAX_OPEN_FILES],
        }
    }

    pub fn alloc(&mut self, vnode_id: InodeId, flags: OpenFlags) -> VfsResult<usize> {
        // Keep ordinary VFS fds out of the stdio range reserved by syscalls.
        for i in RESERVED_STDIO_FDS..MAX_OPEN_FILES {
            if !self.files[i].active {
                self.files[i] = OpenFile {
                    vnode_id,
                    flags,
                    offset: 0,
                    active: true,
                };
                return Ok(i);
            }
        }
        Err(VfsError::TooManyOpenFiles)
    }

    pub fn alloc_at(&mut self, fd: usize, vnode_id: InodeId, flags: OpenFlags) -> VfsResult<usize> {
        if fd >= MAX_OPEN_FILES {
            return Err(VfsError::BadFd);
        }
        if self.files[fd].active {
            return Err(VfsError::Busy);
        }
        self.files[fd] = OpenFile {
            vnode_id,
            flags,
            offset: 0,
            active: true,
        };
        Ok(fd)
    }

    pub fn dup(&mut self, old_fd: usize) -> VfsResult<usize> {
        let file = self.get(old_fd)?;
        let vnode_id = file.vnode_id;
        let flags = file.flags;
        let offset = file.offset;
        // Preserve the same reserved range for duplicated descriptors.
        for i in RESERVED_STDIO_FDS..MAX_OPEN_FILES {
            if !self.files[i].active {
                self.files[i] = OpenFile {
                    vnode_id,
                    flags,
                    offset,
                    active: true,
                };
                return Ok(i);
            }
        }
        Err(VfsError::TooManyOpenFiles)
    }

    pub fn get(&self, fd: usize) -> VfsResult<&OpenFile> {
        if fd < MAX_OPEN_FILES && self.files[fd].active {
            Ok(&self.files[fd])
        } else {
            Err(VfsError::BadFd)
        }
    }

    pub fn get_mut(&mut self, fd: usize) -> VfsResult<&mut OpenFile> {
        if fd < MAX_OPEN_FILES && self.files[fd].active {
            Ok(&mut self.files[fd])
        } else {
            Err(VfsError::BadFd)
        }
    }

    pub fn close(&mut self, fd: usize) -> VfsResult<InodeId> {
        if fd < MAX_OPEN_FILES && self.files[fd].active {
            let vid = self.files[fd].vnode_id;
            self.files[fd] = OpenFile::empty();
            Ok(vid)
        } else {
            Err(VfsError::BadFd)
        }
    }

    pub fn open_count(&self) -> usize {
        self.files.iter().filter(|f| f.active).count()
    }

    pub fn close_all(&mut self) -> usize {
        let mut count = 0;
        for f in self.files.iter_mut() {
            if f.active {
                *f = OpenFile::empty();
                count += 1;
            }
        }
        count
    }
}
