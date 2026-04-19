//////////////////////////////////////////////////////////////////////////////////////
// ext2/3/4 extended attributes                                                     //
//                                                                                  //
// On-disk layout:                                                                  //
//   - Inline xattrs: stored after the inode's fixed + extra fields                 //
//     in the remaining space of the inode (inode_size - 128 - extra_isize)         //
//   - External xattr block: pointed to by i_file_acl, contains a                   // 
//     header + array of entries + values (values grow from end of block)           //
//                                                                                  //
// Entry format (on-disk):                                                          //
//   0: u8  name_len                                                                //
//   1: u8  name_index (namespace: 1=user, 2=system.posix_acl_access, 7=system)     //
//   2: u16 value_offset (from start of value area)                                 //
//   4: u32 value_block  (0 for inline values in same block)                        //
//   8: u32 value_size                                                              //
//  12: u32 hash                                                                    //
//  16: name[name_len]   (NOT null-terminated)                                      //
//////////////////////////////////////////////////////////////////////////////////////

use crate::miku_extfs::{FsError, MikuFS};
use crate::miku_extfs::structs::Inode;

const XATTR_MAGIC: u32 = 0xEA020000;

// namespace indices
pub const XATTR_INDEX_USER: u8 = 1;
pub const XATTR_INDEX_POSIX_ACL_ACCESS: u8 = 2;
pub const XATTR_INDEX_POSIX_ACL_DEFAULT: u8 = 3;
pub const XATTR_INDEX_TRUSTED: u8 = 4;
pub const XATTR_INDEX_SECURITY: u8 = 6;
pub const XATTR_INDEX_SYSTEM: u8 = 7;

#[derive(Clone, Copy)]
pub struct XattrEntry {
    pub name_index: u8,
    pub name: [u8; 64],
    pub name_len: u8,
    pub value: [u8; 256],
    pub value_len: u16,
}

impl XattrEntry {
    pub const fn empty() -> Self {
        Self {
            name_index: 0,
            name: [0; 64],
            name_len: 0,
            value: [0; 256],
            value_len: 0,
        }
    }

    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }
}

pub const MAX_XATTR_ENTRIES: usize = 16;

pub struct XattrList {
    pub entries: [XattrEntry; MAX_XATTR_ENTRIES],
    pub count: usize,
}

impl XattrList {
    pub const fn new() -> Self {
        Self {
            entries: [const { XattrEntry::empty() }; MAX_XATTR_ENTRIES],
            count: 0,
        }
    }
}

fn read_u16_le(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

fn read_u32_le(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn write_u16_le(buf: &mut [u8], off: usize, val: u16) {
    buf[off..off + 2].copy_from_slice(&val.to_le_bytes());
}

fn write_u32_le(buf: &mut [u8], off: usize, val: u32) {
    buf[off..off + 4].copy_from_slice(&val.to_le_bytes());
}

// parse xattr entries from a block buffer (external xattr block or inline area)
fn parse_xattr_entries(
    data: &[u8],
    entry_start: usize,
    value_base: usize,
    list: &mut XattrList,
) {
    let mut pos = entry_start;
    while pos + 16 <= data.len() && list.count < MAX_XATTR_ENTRIES {
        let name_len = data[pos] as usize;
        let name_index = data[pos + 1];
        let value_offset = read_u16_le(data, pos + 2) as usize;
        // value_block at pos+4 (u32) - we only handle block=0
        let value_size = read_u32_le(data, pos + 8) as usize;

        // end marker: name_len == 0 and name_index == 0
        if name_len == 0 && name_index == 0 {
            break;
        }

        if pos + 16 + name_len > data.len() {
            break;
        }

        let e = &mut list.entries[list.count];
        e.name_index = name_index;
        let copy_len = name_len.min(64);
        e.name[..copy_len].copy_from_slice(&data[pos + 16..pos + 16 + copy_len]);
        e.name_len = copy_len as u8;

        if value_size > 0 && value_offset > 0 {
            let voff = value_base + value_offset;
            if voff + value_size <= data.len() {
                let vcopy = value_size.min(256);
                e.value[..vcopy].copy_from_slice(&data[voff..voff + vcopy]);
                e.value_len = vcopy as u16;
            }
        }

        list.count += 1;

        // entries are 4-byte aligned
        let entry_size = ((16 + name_len) + 3) & !3;
        pos += entry_size;
    }
}

impl MikuFS {
    // read all xattrs for an inode (both inline and external block)
    pub fn read_xattrs(&mut self, inode_num: u32) -> Result<XattrList, FsError> {
        let inode = self.read_inode(inode_num)?;
        let mut list = XattrList::new();

        // inline xattrs: located after inode fixed fields (128 bytes) + extra_isize
        let inode_size = self.inode_size() as usize;
        let extra_isize = inode.extra_isize() as usize;
        if inode_size > 128 && extra_isize > 0 {
            let inline_start = 128 + extra_isize;
            if inline_start + 4 < inode_size {
                // inline xattr area has a 4-byte magic header
                let magic = read_u32_le(&inode.data, inline_start);
                if magic == XATTR_MAGIC {
                    let area_end = inode_size;
                    // entries start at inline_start + 4
                    // values are stored inline, offset relative to first value position
                    parse_xattr_entries(
                        &inode.data[..area_end],
                        inline_start + 4,
                        inline_start + 4, // value base = entry start for inline
                        &mut list,
                    );
                }
            }
        }

        // external xattr block
        let xattr_block = inode.file_acl_lo();
        if xattr_block != 0 {
            let bs = self.block_size as usize;
            let mut buf = [0u8; 4096];
            self.read_block_into(xattr_block, &mut buf[..bs])?;

            let magic = read_u32_le(&buf, 0);
            if magic == XATTR_MAGIC {
                // header is 32 bytes, entries start at offset 32
                // values grow backwards from end of block
                parse_xattr_entries(&buf[..bs], 32, 0, &mut list);
            }
        }

        Ok(list)
    }

    // get a single xattr value by name
    pub fn get_xattr(
        &mut self,
        inode_num: u32,
        name_index: u8,
        name: &str,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        let list = self.read_xattrs(inode_num)?;
        let name_bytes = name.as_bytes();
        for i in 0..list.count {
            let e = &list.entries[i];
            if e.name_index == name_index
                && e.name_len as usize == name_bytes.len()
                && &e.name[..e.name_len as usize] == name_bytes
            {
                let vlen = e.value_len as usize;
                let copy = vlen.min(buf.len());
                buf[..copy].copy_from_slice(&e.value[..copy]);
                return Ok(vlen);
            }
        }
        Err(FsError::NotFound)
    }

    // set xattr on the external xattr block (allocates one if needed)
    pub fn set_xattr(
        &mut self,
        inode_num: u32,
        name_index: u8,
        name: &str,
        value: &[u8],
    ) -> Result<(), FsError> {
        let mut inode = self.read_inode(inode_num)?;
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        let xattr_block = inode.file_acl_lo();

        if xattr_block != 0 {
            self.read_block_into(xattr_block, &mut buf[..bs])?;
        } else {
            // allocate new xattr block
            let group = ((inode_num - 1) / self.inodes_per_group) as usize;
            let new_block = self.alloc_block(group)?;
            // init header
            buf[..bs].fill(0);
            write_u32_le(&mut buf, 0, XATTR_MAGIC);
            write_u32_le(&mut buf, 4, 1); // refcount
            write_u32_le(&mut buf, 8, 0); // blocks
            // rest of header zeroed
            inode.set_file_acl_lo(new_block);
            self.write_inode(inode_num, &inode)?;
            self.write_block_data(new_block, &buf[..bs])?;
        }

        let xattr_block = inode.file_acl_lo();

        // rebuild entries: collect existing (minus the one we're replacing), add new
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len();

        // scan existing entries
        struct TmpEntry {
            name_idx: u8,
            name: [u8; 64],
            nlen: u8,
            value: [u8; 256],
            vlen: u16,
        }
        let mut entries: [core::mem::MaybeUninit<TmpEntry>; 16] =
            [const { core::mem::MaybeUninit::uninit() }; 16];
        let mut ecount = 0usize;

        // parse current entries
        let mut pos = 32usize;
        while pos + 16 <= bs && ecount < 15 {
            let nl = buf[pos] as usize;
            let ni = buf[pos + 1];
            if nl == 0 && ni == 0 { break; }
            let voff = read_u16_le(&buf, pos + 2) as usize;
            let vsz = read_u32_le(&buf, pos + 8) as usize;
            if pos + 16 + nl > bs { break; }

            // skip the entry we're replacing
            if ni == name_index && nl == name_len && &buf[pos + 16..pos + 16 + nl] == name_bytes {
                let entry_size = ((16 + nl) + 3) & !3;
                pos += entry_size;
                continue;
            }

            let mut tmp = TmpEntry {
                name_idx: ni,
                name: [0; 64],
                nlen: nl.min(64) as u8,
                value: [0; 256],
                vlen: 0,
            };
            tmp.name[..tmp.nlen as usize].copy_from_slice(&buf[pos + 16..pos + 16 + tmp.nlen as usize]);
            if vsz > 0 && voff > 0 && voff + vsz <= bs {
                let vc = vsz.min(256);
                tmp.value[..vc].copy_from_slice(&buf[voff..voff + vc]);
                tmp.vlen = vc as u16;
            }
            entries[ecount] = core::mem::MaybeUninit::new(tmp);
            ecount += 1;

            let entry_size = ((16 + nl) + 3) & !3;
            pos += entry_size;
        }

        // add new entry
        if value.len() > 0 {
            let mut tmp = TmpEntry {
                name_idx: name_index,
                name: [0; 64],
                nlen: name_len.min(64) as u8,
                value: [0; 256],
                vlen: value.len().min(256) as u16,
            };
            tmp.name[..tmp.nlen as usize].copy_from_slice(&name_bytes[..tmp.nlen as usize]);
            tmp.value[..tmp.vlen as usize].copy_from_slice(&value[..tmp.vlen as usize]);
            entries[ecount] = core::mem::MaybeUninit::new(tmp);
            ecount += 1;
        }

        // rebuild block
        buf[32..bs].fill(0);
        let mut wpos = 32usize;
        let mut val_end = bs; // values grow backwards from end

        for i in 0..ecount {
            let e = unsafe { entries[i].assume_init_ref() };
            let nl = e.nlen as usize;
            let entry_size = ((16 + nl) + 3) & !3;
            if wpos + entry_size > val_end { break; } // no space

            let vl = e.vlen as usize;
            if vl > 0 {
                let aligned_vl = (vl + 3) & !3;
                if val_end < aligned_vl || wpos + entry_size > val_end - aligned_vl {
                    break; // no space for value
                }
                val_end -= aligned_vl;
                buf[val_end..val_end + vl].copy_from_slice(&e.value[..vl]);
            }

            buf[wpos] = nl as u8;
            buf[wpos + 1] = e.name_idx;
            if vl > 0 {
                write_u16_le(&mut buf, wpos + 2, val_end as u16);
            }
            write_u32_le(&mut buf, wpos + 4, 0); // value_block
            write_u32_le(&mut buf, wpos + 8, vl as u32);
            write_u32_le(&mut buf, wpos + 12, 0); // hash (simplified)
            buf[wpos + 16..wpos + 16 + nl].copy_from_slice(&e.name[..nl]);
            wpos += entry_size;
        }

        self.write_block_data(xattr_block, &buf[..bs])?;

        // update ctime
        let now = self.get_timestamp();
        let mut inode = self.read_inode(inode_num)?;
        inode.set_ctime(now);
        self.write_inode(inode_num, &inode)?;

        Ok(())
    }

    // remove xattr
    pub fn remove_xattr(
        &mut self,
        inode_num: u32,
        name_index: u8,
        name: &str,
    ) -> Result<(), FsError> {
        // set with empty value removes it
        self.set_xattr(inode_num, name_index, name, &[])
    }
}
