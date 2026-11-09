// Userspace ABI: byte layouts of structs returned to user-mode programs. Stable
// contracts: do not reorder or resize fields without coordinating libmiku and
// userspace

use crate::fs::vfs::types::VNodeStat;

pub use miku_abi::kstat::{MikuDirent, MikuStat, MikuStatfs, DIRENT_NAME_LEN as UDIRENT_NAME_LEN};

pub const STAT_SIZE: u64 = core::mem::size_of::<MikuStat>() as u64;
pub const UDIRENT_SIZE: u64 = core::mem::size_of::<MikuDirent>() as u64;
pub const UDIRENT_INO_OFF: usize = core::mem::offset_of!(MikuDirent, inode_id);
pub const UDIRENT_KIND_OFF: usize = core::mem::offset_of!(MikuDirent, kind);
pub const UDIRENT_NAMELEN_OFF: usize = core::mem::offset_of!(MikuDirent, name_len);
pub const STATFS_SIZE: u64 = core::mem::size_of::<MikuStatfs>() as u64;

pub fn write_stat_to_user(ptr: u64, st: &VNodeStat) {
    let out = MikuStat {
        size: st.size,
        mode: st.mode.0 as u32,
        nlinks: st.nlinks as u32,
        uid: st.uid,
        gid: st.gid,
        kind: st.kind as u8,
        fs_type: st.fs_type as u8,
        dev_major: st.dev_major,
        dev_minor: st.dev_minor,
        atime: st.atime,
        mtime: st.mtime,
        ctime: st.ctime,
        inode_id: st.ino,
        blocks: st.blocks,
        _reserved: [0; 4],
    };
    unsafe { (ptr as *mut MikuStat).write_unaligned(out) };
}
