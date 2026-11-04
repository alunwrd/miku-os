// Passes 2 and 3: what directories say, and if they form a tree
//
// Pass 2 walks each directory record by record. It cannot use `read_dir_each`:
// the reader stops at an unbelievable `rec_len` and continues with the next
// block, but a checker must report it at its offset.
//
// Pass 3 asks if they form a tree rooted at inode 2. A subtree whose name was
// lost is self-consistent and unreachable.

use alloc::vec::Vec;

use super::maps::block_buf;
use super::{name_is_legal, Fsck, Problem};
use crate::error::FsError;
use crate::ext2::htree::{dx_entry_block, DX_NODE_ENTRIES, DX_ROOT_ENTRIES};
use crate::fs::MikuFS;
use crate::host::ExtHost;
use crate::structs::*;

/// One record, as the strict walk sees it
struct Rec {
    len: usize,
    ino: u32,
    name_len: usize,
    file_type: u8,
}

impl<H: ExtHost> MikuFS<H> {
    // pass 2

    pub(crate) fn fsck_pass2(&mut self, st: &mut Fsck) -> Result<(), FsError> {
        // (name hash, directory) for each entry, so a duplicate can be found
        // without holding every name
        let mut names: Vec<(u64, u32)> = Vec::new();

        for ino in 1..=st.inodes_count {
            if !st.is_dir(ino) {
                continue;
            }
            self.fsck_check_dir(st, ino, &mut names)?;
        }

        st.parents.finish();
        self.fsck_confirm_duplicates(st, &mut names)?;
        Ok(())
    }

    fn fsck_check_dir(
        &mut self, st: &mut Fsck, ino: u32, names: &mut Vec<(u64, u32)>,
    ) -> Result<(), FsError> {
        let inode = match self.read_inode_unchecked(ino) {
            Ok((i, _)) => i,
            Err(_) => return Ok(()),
        };
        let bs = self.block_size as u64;
        let blocks = inode.size().div_ceil(bs) as u32;
        let indexed = self.dir_is_indexed(&inode);
        let index_blocks = if indexed {
            self.fsck_index_blocks(st, ino, &inode)?
        } else {
            Vec::new()
        };

        let mut saw_dot = false;
        let mut saw_dotdot = false;

        for logical in 0..blocks {
            let phys = match self.get_file_block(&inode, logical) {
                Ok(0) => continue,
                Ok(p) => p,
                Err(_) => continue,
            };
            if indexed && (logical == 0 || index_blocks.contains(&logical)) {
                let entries_off = if logical == 0 { DX_ROOT_ENTRIES } else { DX_NODE_ENTRIES };
                self.fsck_check_index_block(st, ino, &inode, phys, entries_off)?;
                if logical == 0 {
                    // Its two fake records are the real `.` and `..`
                    let (d, dd) = self.fsck_check_dots(st, ino, phys)?;
                    saw_dot |= d;
                    saw_dotdot |= dd;
                }
                continue;
            }

            let (d, dd) = self.fsck_check_dir_block(st, ino, &inode, logical, phys, names)?;
            saw_dot |= d;
            saw_dotdot |= dd;
        }

        if !saw_dot {
            st.found(Problem::DirMissingDot { ino });
        }
        if !saw_dotdot {
            st.found(Problem::DirMissingDotDot { ino });
        }
        Ok(())
    }

    /// Which logical blocks are index nodes. A block alone cannot tell: an
    /// interior node's first eight bytes and an empty leaf's are both a record
    /// covering the block. So the index is descended, which also finds wrong
    /// pointers
    fn fsck_index_blocks(
        &mut self, st: &mut Fsck, ino: u32, inode: &Inode,
    ) -> Result<Vec<u32>, FsError> {
        let mut nodes: Vec<u32> = Vec::new();
        let depth = match self.htree_depth(inode) {
            Ok(d) => d,
            Err(_) => return Ok(nodes),
        };
        if depth == 0 {
            return Ok(nodes);
        }

        // Level by level: the root's entries are nodes, as are theirs until the
        // last level, whose entries are leaves
        let mut level: Vec<u32> = alloc::vec![0u32];
        let mut entries_off = DX_ROOT_ENTRIES;
        for _ in 0..depth {
            let mut next: Vec<u32> = Vec::new();
            for &logical in level.iter() {
                let phys = match self.get_file_block(inode, logical) {
                    Ok(0) | Err(_) => continue,
                    Ok(p) => p,
                };
                let mut buf = block_buf(self.block_size);
                if self.read_block_into(phys, &mut buf).is_err() {
                    st.found(Problem::DirBadIndex { ino, block: phys });
                    continue;
                }
                let limit = u16::from_le_bytes([buf[entries_off], buf[entries_off + 1]]);
                let count = u16::from_le_bytes([buf[entries_off + 2], buf[entries_off + 3]]);
                if count == 0 || count > limit || limit != self.dx_limit(entries_off) {
                    st.found(Problem::DirBadIndex { ino, block: phys });
                    continue;
                }
                for i in 0..count {
                    next.push(dx_entry_block(&buf, entries_off, i));
                }
            }
            nodes.extend_from_slice(&next);
            level = next;
            entries_off = DX_NODE_ENTRIES;
        }
        // The last level's entries are leaves
        for l in level {
            if let Some(at) = nodes.iter().position(|&n| n == l) {
                nodes.remove(at);
            }
        }
        Ok(nodes)
    }

    fn fsck_check_index_block(
        &mut self, st: &mut Fsck, ino: u32, inode: &Inode, phys: u32, entries_off: usize,
    ) -> Result<(), FsError> {
        let mut buf = block_buf(self.block_size);
        if self.read_block_into(phys, &mut buf).is_err() {
            st.found(Problem::DirBadIndex { ino, block: phys });
            return Ok(());
        }

        let limit = u16::from_le_bytes([buf[entries_off], buf[entries_off + 1]]);
        let count = u16::from_le_bytes([buf[entries_off + 2], buf[entries_off + 3]]);
        if limit != self.dx_limit(entries_off) || count > limit {
            st.found(Problem::DirBadIndex { ino, block: phys });
            return Ok(());
        }
        if !self.verify_dx_block(ino, inode.generation(), entries_off, &buf) {
            let fixed = if st.opts.repair {
                self.write_dx_block(ino, phys, entries_off, &mut buf).is_ok()
            } else {
                false
            };
            st.note(Problem::DirBadChecksum { ino, block: phys }, fixed);
        }
        Ok(())
    }

    /// The `.` and `..` records at the front of an index root, the only entries
    /// that block holds
    fn fsck_check_dots(
        &mut self, st: &mut Fsck, ino: u32, phys: u32,
    ) -> Result<(bool, bool), FsError> {
        let mut buf = block_buf(self.block_size);
        if self.read_block_into(phys, &mut buf).is_err() {
            return Ok((false, false));
        }
        let dot = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        let dotdot = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);

        let mut saw_dot = false;
        if buf[6] == 1 && buf[8] == b'.' && dot == ino {
            st.links.inc(ino);
            saw_dot = true;
        }
        let mut saw_dotdot = false;
        if buf[18] == 2 && buf[20] == b'.' && buf[21] == b'.' && dotdot != 0 {
            st.links.inc(dotdot);
            st.parents.note_dotdot(ino, dotdot);
            saw_dotdot = true;
        }
        Ok((saw_dot, saw_dotdot))
    }

    /// One block of directory entries, walked strictly
    fn fsck_check_dir_block(
        &mut self, st: &mut Fsck, ino: u32, inode: &Inode,
        logical: u32, phys: u32, names: &mut Vec<(u64, u32)>,
    ) -> Result<(bool, bool), FsError> {
        let usable = self.dir_block_usable();
        let mut buf = block_buf(self.block_size);
        if self.read_block_into(phys, &mut buf).is_err() {
            st.found(Problem::DirBadChecksum { ino, block: phys });
            return Ok((false, false));
        }

        if !self.verify_dir_block(ino, inode.generation(), &buf) {
            let fixed = if st.opts.repair {
                self.write_dir_block(ino, phys, &mut buf).is_ok()
            } else {
                false
            };
            st.note(Problem::DirBadChecksum { ino, block: phys }, fixed);
        }

        let mut saw_dot = false;
        let mut saw_dotdot = false;
        let mut changed = false;
        let mut pos = 0usize;
        let mut prev: Option<usize> = None;

        while pos < usable {
            let Some(rec) = read_record(&buf, pos, usable) else {
                let fixed = if st.opts.repair {
                    salvage(&mut buf, pos, usable, prev);
                    changed = true;
                    true
                } else {
                    false
                };
                st.note(Problem::DirBadRecLen { ino, block: phys, at: pos as u32 }, fixed);
                pos = usable;
                break;
            };

            if rec.ino != 0 {
                let name = &buf[pos + 8..pos + 8 + rec.name_len];
                let nh = name_hash(name);
                let verdict = self.fsck_judge_entry(st, ino, logical, pos, name, &rec);
                match verdict {
                    Entry::Dot => saw_dot = true,
                    Entry::DotDot => saw_dotdot = true,
                    Entry::Ordinary => names.push((nh, ino)),
                    Entry::Remove(problem) => {
                        if st.opts.repair {
                            remove_record(&mut buf, pos, prev);
                            changed = true;
                            st.fixed(problem);
                        } else {
                            st.found(problem);
                        }
                        pos += rec.len;
                        // The record is gone, so the prior one stays previous
                        // for what follows
                        continue;
                    }
                    Entry::RetypeTo(want, problem) => {
                        if st.opts.repair {
                            buf[pos + 7] = want;
                            changed = true;
                            st.fixed(problem);
                        } else {
                            st.found(problem);
                        }
                        names.push((nh, ino));
                    }
                }
            }

            prev = Some(pos);
            pos += rec.len;
        }

        if pos != usable {
            st.found(Problem::DirBadChain { ino, block: phys });
        }

        if changed {
            self.write_dir_block(ino, phys, &mut buf)?;
        }
        Ok((saw_dot, saw_dotdot))
    }

    /// What to do about one entry, and what it counts as
    fn fsck_judge_entry(
        &mut self, st: &mut Fsck, ino: u32, logical: u32, pos: usize,
        name: &[u8], rec: &Rec,
    ) -> Entry {
        if !name_is_legal(name) {
            return Entry::Remove(Problem::DirBadName { ino, child: rec.ino });
        }

        if name == b"." {
            if logical == 0 && pos == 0 && rec.ino == ino {
                st.links.inc(ino);
                return Entry::Dot;
            }
            return Entry::Remove(Problem::DirBadName { ino, child: rec.ino });
        }
        if name == b".." {
            if logical == 0 {
                st.links.inc(rec.ino);
                st.parents.note_dotdot(ino, rec.ino);
                return Entry::DotDot;
            }
            return Entry::Remove(Problem::DirBadName { ino, child: rec.ino });
        }

        if st.ino_bit(rec.ino).is_none() {
            return Entry::Remove(Problem::DirEntryOutOfRange { ino, child: rec.ino });
        }
        if !st.inode_in_use(rec.ino) {
            return Entry::Remove(Problem::DirEntryFreeInode { ino, child: rec.ino });
        }

        st.links.inc(rec.ino);
        if st.is_dir(rec.ino) {
            st.parents.note_child(rec.ino, ino);
        }

        if self.superblock.has_filetype() {
            let want = self
                .read_inode_unchecked(rec.ino)
                .ok()
                .and_then(|(i, _)| super::ft_for_mode(i.mode()));
            if let Some(want) = want {
                if want != rec.file_type {
                    return Entry::RetypeTo(
                        want,
                        Problem::DirEntryBadType { ino, child: rec.ino },
                    );
                }
            }
        }
        Entry::Ordinary
    }

    /// A matching hash is not yet a duplicate name; confirming costs one more
    /// walk of the directories involved
    fn fsck_confirm_duplicates(
        &mut self, st: &mut Fsck, names: &mut Vec<(u64, u32)>,
    ) -> Result<(), FsError> {
        names.sort_unstable();
        let mut suspects: Vec<(u64, u32)> = Vec::new();
        for w in names.windows(2) {
            if w[0] == w[1] {
                suspects.push(w[0]);
            }
        }
        suspects.dedup();

        for (hash, dir) in suspects {
            let inode = match self.read_inode_unchecked(dir) {
                Ok((i, _)) => i,
                Err(_) => continue,
            };
            let mut seen: Vec<(u64, u32)> = Vec::new();
            self.read_dir_each(&inode, |e| {
                let n = &e.name[..e.name_len as usize];
                if name_hash(n) == hash {
                    seen.push((name_hash(n), e.inode));
                }
                true
            })?;
            if seen.len() > 1 {
                st.found(Problem::DirDuplicateName { ino: dir, child: seen[1].1 });
            }
        }
        Ok(())
    }

    // pass 3

    pub(crate) fn fsck_pass3(&mut self, st: &mut Fsck) -> Result<(), FsError> {
        if !st.is_dir(EXT2_ROOT_INO) {
            // With no root there is no tree to judge
            return Ok(());
        }

        spread_reachable(st, EXT2_ROOT_INO);

        for ino in 1..=st.inodes_count {
            if !st.is_dir(ino) {
                continue;
            }
            let parents = st.parents.parents_of(ino).len();
            if parents > 1 {
                // Not repaired: the filesystem does not record which name is
                // real
                st.found(Problem::DirTwoParents { ino });
            }

            if ino != EXT2_ROOT_INO && !st.is_reachable(ino) {
                let fixed = self.fsck_reconnect(st, ino)?;
                st.note(Problem::DirDisconnected { ino }, fixed);
                if !fixed {
                    continue;
                }
            }

            let should = if ino == EXT2_ROOT_INO {
                EXT2_ROOT_INO
            } else {
                match st.parents.parents_of(ino).first() {
                    Some(&(_, p)) => p,
                    None => continue,
                }
            };
            let Some(was) = st.parents.dotdot_of(ino) else { continue };
            if was == should {
                continue;
            }
            let fixed = self.fsck_set_dotdot(st, ino, should)?;
            st.note(Problem::DirDotDotWrong { ino, was, should }, fixed);
            if fixed {
                st.links.dec(was);
                st.links.inc(should);
            }
        }
        Ok(())
    }

    /// Give a directory nobody names a name in `/lost+found`
    fn fsck_reconnect(&mut self, st: &mut Fsck, ino: u32) -> Result<bool, FsError> {
        if !st.opts.repair {
            return Ok(false);
        }
        let Some(lf) = self.fsck_lost_found()? else { return Ok(false) };
        let (bytes, len) = super::orphan_name(ino);
        let name = core::str::from_utf8(&bytes[..len]).unwrap_or("");
        if self.add_dir_entry(lf, name, ino, FT_DIR).is_err() {
            return Ok(false);
        }
        st.reconnected = true;
        st.links.inc(ino);
        st.parents.reparent(ino, lf);
        // Everything under it returns too. Otherwise each child goes to
        // lost+found separately, leaving the parent naming a directory now with
        // two names
        spread_reachable(st, ino);
        Ok(true)
    }

    pub(crate) fn fsck_set_dotdot(
        &mut self, st: &Fsck, ino: u32, parent: u32,
    ) -> Result<bool, FsError> {
        if !st.opts.repair {
            return Ok(false);
        }
        let Ok(inode) = self.read_inode(ino) else { return Ok(false) };
        let Ok(phys) = self.get_file_block(&inode, 0) else { return Ok(false) };
        if phys == 0 {
            return Ok(false);
        }
        let mut buf = block_buf(self.block_size);
        if self.read_block_into(phys, &mut buf).is_err() {
            return Ok(false);
        }
        buf[12..16].copy_from_slice(&parent.to_le_bytes());
        self.write_dir_head(ino, &inode, phys, &mut buf)?;
        Ok(true)
    }

    /// `/lost+found`, if present. Nothing is created here on a damaged volume
    pub(crate) fn fsck_lost_found(&mut self) -> Result<Option<u32>, FsError> {
        let root = self.read_inode(EXT2_ROOT_INO)?;
        match self.lookup(EXT2_ROOT_INO, &root, "lost+found") {
            Ok(ino) => Ok(Some(ino)),
            Err(FsError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Mark `from` and everything below it as reachable
fn spread_reachable(st: &mut Fsck, from: u32) {
    let mut stack: Vec<u32> = alloc::vec![from];
    st.mark_reachable(from);
    while let Some(dir) = stack.pop() {
        let kids: Vec<u32> = st.parents.children_of(dir).iter().map(|&(_, c)| c).collect();
        for child in kids {
            if child == dir {
                continue;
            }
            if !st.mark_reachable(child) {
                stack.push(child);
            }
        }
    }
}

/// What one entry turned out to be
enum Entry {
    Dot,
    DotDot,
    Ordinary,
    Remove(Problem),
    RetypeTo(u8, Problem),
}

/// A record at `pos`, or `None` when the bytes there cannot be one
fn read_record(buf: &[u8], pos: usize, usable: usize) -> Option<Rec> {
    if pos + 8 > usable {
        return None;
    }
    let ino = u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
    let len = u16::from_le_bytes([buf[pos + 4], buf[pos + 5]]) as usize;
    let name_len = buf[pos + 6] as usize;
    if len < 8 || len % 4 != 0 || pos + len > usable {
        return None;
    }
    if ino != 0 && 8 + name_len > len {
        return None;
    }
    Some(Rec { len, ino, name_len, file_type: buf[pos + 7] })
}

/// Turns everything from `pos` to the end of the usable area into one empty
/// record, discarding what could not be walked, as e2fsck salvages a block
fn salvage(buf: &mut [u8], pos: usize, usable: usize, prev: Option<usize>) {
    match prev {
        Some(p) => {
            let len = (usable - p) as u16;
            buf[p + 4..p + 6].copy_from_slice(&len.to_le_bytes());
        }
        None if pos + 8 <= usable => {
            buf[pos..pos + 4].copy_from_slice(&0u32.to_le_bytes());
            buf[pos + 4..pos + 6].copy_from_slice(&((usable - pos) as u16).to_le_bytes());
            buf[pos + 6] = 0;
            buf[pos + 7] = 0;
        }
        None => {}
    }
}

/// Drops one record: folds it into the previous, or blanks it when first in the
/// block
fn remove_record(buf: &mut [u8], pos: usize, prev: Option<usize>) {
    match prev {
        Some(p) => {
            let plen = u16::from_le_bytes([buf[p + 4], buf[p + 5]]) as usize;
            let len = u16::from_le_bytes([buf[pos + 4], buf[pos + 5]]) as usize;
            buf[p + 4..p + 6].copy_from_slice(&((plen + len) as u16).to_le_bytes());
        }
        None => buf[pos..pos + 4].copy_from_slice(&0u32.to_le_bytes()),
    }
}

/// FNV-1a, compared only with itself; matches are confirmed by the names
fn name_hash(name: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in name {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
