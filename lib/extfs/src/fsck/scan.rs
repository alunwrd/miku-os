// Passes 0 and 1: the filesystem's frame, then every inode
//
// Pass 0 asks if the volume describes itself consistently: the group count
// follows the block count, and each group's bitmaps and inode table are inside
// the filesystem without overlap.
//
// Pass 1 reads every inode and follows every block pointer, building the map of
// who owns what. That map shows a block owned by two files, a block owned by
// nobody, and a bitmap disagreeing with both.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::maps::block_buf;
use super::{Fsck, Problem};
use crate::error::FsError;
use crate::fs::MikuFS;
use crate::host::ExtHost;
use crate::structs::*;

/// What a block pointer turned out to be
enum Claim {
    /// Usable, now recorded for this inode
    Kept,
    /// Unusable; the caller drops the pointer when repairing
    Bad,
    /// Owned elsewhere; this inode now owns the copy at this block
    Moved(u32),
}

impl<H: ExtHost> MikuFS<H> {
    // pass 0

    pub(crate) fn fsck_pass0(&mut self, st: &mut Fsck) -> Result<(), FsError> {
        if self.superblock.magic() != EXT2_MAGIC {
            st.found(Problem::BadMagic);
            return Ok(());
        }

        if self.superblock.has_metadata_csum() && !self.verify_superblock_csum() {
            let fixed = if st.opts.repair {
                self.update_superblock_csum();
                self.superblock_dirty = true;
                self.flush_superblock().is_ok()
            } else {
                false
            };
            st.note(Problem::BadSuperblockChecksum, fixed);
        }

        if self.superblock.state() & EXT2_ERROR_FS != 0 {
            // Not repaired here: cleared when the check finishes, only if it
            // finishes
            st.found(Problem::FilesystemMarkedBad);
        }

        let bpg = self.blocks_per_group;
        let span = st.blocks_count.saturating_sub(st.first_block);
        let want_groups = span.div_ceil(bpg.max(1));
        if bpg == 0
            || self.inodes_per_group == 0
            || want_groups != self.groups.len() as u32
            || st.inodes_count != self.inodes_per_group * self.groups.len() as u32
        {
            st.found(Problem::BadGeometry);
        }

        self.fsck_group_metadata(st)?;
        Ok(())
    }

    /// Where the filesystem's structures live: checked for bounds and overlap,
    /// then folded into the map telling pass 1 which blocks no file may claim
    fn fsck_group_metadata(&mut self, st: &mut Fsck) -> Result<(), FsError> {
        let bs = self.block_size;
        let groups = self.groups.len();
        let gdt_blocks = (groups as u32 * self.superblock.group_desc_size()).div_ceil(bs);
        let reserved = self.superblock.reserved_gdt_blocks() as u32;
        let itb = (self.inodes_per_group as u64 * self.inode_size() as u64).div_ceil(bs as u64) as u32;

        // (start, length, group). Intervals instead of a second bitmap: a few
        // per group
        let mut spans: Vec<(u32, u32, u32)> = Vec::new();
        for g in 0..groups {
            let g32 = g as u32;
            if self.group_has_super(g32) {
                let start = st.first_block + g32 * self.blocks_per_group;
                spans.push((start, 1 + gdt_blocks + reserved, g32));
            }
            spans.push((self.groups[g].block_bitmap(), 1, g32));
            spans.push((self.groups[g].inode_bitmap(), 1, g32));
            spans.push((self.groups[g].inode_table(), itb, g32));
        }

        for &(start, len, g) in spans.iter() {
            let end = start as u64 + len as u64;
            if start < st.first_block || end > st.blocks_count as u64 {
                st.found(Problem::GroupMetaOutOfRange { group: g, block: start as u64 });
            }
        }

        spans.sort_unstable();
        for w in spans.windows(2) {
            let (s0, l0, _) = w[0];
            let (s1, _, g1) = w[1];
            if s0 as u64 + l0 as u64 > s1 as u64 {
                st.found(Problem::GroupMetaOverlap { group: g1, block: s1 as u64 });
            }
        }

        // The map comes from the allocator's view of a fresh group's bitmap, so
        // "which blocks are metadata" has one definition
        let mut buf = block_buf(bs);
        for g in 0..groups {
            buf.fill(0);
            self.mark_group_metadata(g, &mut buf);
            st.meta.or_bytes(g * bs as usize, &buf);
        }

        if self.superblock.has_gdt_csum() {
            for g in 0..groups {
                if self.verify_group_desc_csum(g) {
                    continue;
                }
                let fixed = if st.opts.repair {
                    self.update_group_desc_csum(g);
                    self.flush_group_desc(g).is_ok()
                } else {
                    false
                };
                st.note(Problem::GroupDescChecksum { group: g as u32 }, fixed);
            }
        }
        Ok(())
    }

    // pass 1

    pub(crate) fn fsck_pass1(&mut self, st: &mut Fsck) -> Result<(), FsError> {
        let sectors_per_block = (self.block_size / 512) as u64;
        // Blocks with an `h_refcount` above one are shared by inodes on
        // purpose, so they are claimed once and counted separately
        let mut xattr_refs: BTreeMap<u32, u32> = BTreeMap::new();

        for ino in 1..=st.inodes_count {
            if !self.fsck_inode_readable(ino) {
                continue;
            }
            let (mut inode, csum_ok) = match self.read_inode_unchecked(ino) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let reserved = super::is_reserved(ino, st.first_ino);
            let links = inode.links_count();
            let mode = inode.mode();

            if !reserved {
                if links == 0 {
                    // Free, unless a deletion started and stopped. Its blocks
                    // are not claimed: pass 5 rebuilds the bitmap from claims,
                    // finishing the deletion
                    if mode != 0 && inode.dtime() == 0 {
                        let fixed = self.fsck_finish_delete(st, ino, &mut inode)?;
                        st.note(Problem::InodeZeroLinks { ino }, fixed);
                    }
                    continue;
                }
                if super::ft_for_mode(mode).is_none() {
                    st.found(Problem::InodeBadMode { ino, mode });
                    continue;
                }
            } else if mode == 0 && links == 0 && ino != EXT2_ROOT_INO {
                // A reserved inode never filled in: in use regardless, with
                // nothing to walk
                st.mark_inode(ino);
                continue;
            }

            if !csum_ok {
                let fixed = if st.opts.repair {
                    self.write_inode(ino, &inode).is_ok()
                } else {
                    false
                };
                st.note(Problem::InodeBadChecksum { ino }, fixed);
            }

            st.mark_inode(ino);
            if inode.is_directory() {
                st.mark_dir(ino);
                st.report.used_dirs += 1;
            }
            st.report.used_inodes += 1;

            if ino == EXT2_ROOT_INO && !inode.is_directory() {
                st.found(Problem::RootNotDirectory);
            }

            let mut dirty = false;
            let mut claimed = self.fsck_walk_map(st, ino, &mut inode, &mut dirty)?;

            // The extended attribute block is the inode's too
            let acl = inode.file_acl_full();
            if acl != 0 {
                claimed += self.fsck_claim_xattr(st, ino, acl, &mut xattr_refs)?;
            }

            let want = if inode.is_huge_file() { claimed } else { claimed * sectors_per_block };
            if inode.blocks_full() != want {
                let fixed = if st.opts.repair {
                    inode.set_blocks_full(want);
                    dirty = true;
                    true
                } else {
                    false
                };
                st.note(
                    Problem::BlockCountWrong { ino, on_disk: inode.blocks_full(), counted: want },
                    fixed,
                );
            }

            if dirty {
                self.write_inode(ino, &inode)?;
            }
        }

        self.fsck_check_xattr_refs(st, &xattr_refs)?;
        Ok(())
    }

    /// Whether this inode is worth reading. An `INODE_UNINIT` group has
    /// allocated none, and `bg_itable_unused` counts never-touched inodes at
    /// the table's end, holding old disk bytes, not zeros. Believing those
    /// would invent files
    fn fsck_inode_readable(&self, ino: u32) -> bool {
        let ipg = self.inodes_per_group;
        if ino == 0 || ipg == 0 {
            return false;
        }
        let g = ((ino - 1) / ipg) as usize;
        if g >= self.groups.len() {
            return false;
        }
        if self.groups[g].flags_gd() & EXT4_BG_INODE_UNINIT != 0 {
            return false;
        }
        if !self.superblock.has_gdt_csum() {
            return true;
        }
        let unused = self.groups[g].itable_unused().min(ipg);
        (ino - 1) % ipg < ipg - unused
    }

    /// An inode with an unfinished delete gets a deletion time, so it reads as
    /// deleted
    fn fsck_finish_delete(
        &mut self, st: &Fsck, ino: u32, inode: &mut Inode,
    ) -> Result<bool, FsError> {
        if !st.opts.repair {
            return Ok(false);
        }
        let now = self.deletion_timestamp();
        inode.set_dtime(now);
        self.write_inode(ino, inode)?;
        Ok(true)
    }

    /// Every block an inode reaches, including the blocks its map is made of
    fn fsck_walk_map(
        &mut self, st: &mut Fsck, ino: u32, inode: &mut Inode, dirty: &mut bool,
    ) -> Result<u64, FsError> {
        // None of these has a block map: the first keeps contents in the inode,
        // the second its target, and a device node its major and minor in
        // `i_block[0]` or `[1]`. Walking a device node's map reads 1:3 as block
        // 259
        if inode.has_inline_data() || inode.is_fast_symlink() || inode.is_special() {
            return Ok(0);
        }
        let mut count = 0u64;
        if inode.uses_extents() {
            self.fsck_walk_extent_root(st, ino, inode, dirty, &mut count)?;
        } else {
            self.fsck_walk_indirect_root(st, ino, inode, dirty, &mut count)?;
        }
        Ok(count)
    }

    // indirect block maps

    fn fsck_walk_indirect_root(
        &mut self, st: &mut Fsck, ino: u32, inode: &mut Inode,
        dirty: &mut bool, count: &mut u64,
    ) -> Result<(), FsError> {
        for i in 0..12 {
            let b = inode.block(i);
            if b == 0 {
                continue;
            }
            match self.fsck_claim(st, ino, b)? {
                Claim::Kept => *count += 1,
                Claim::Moved(new) => {
                    inode.set_block(i, new);
                    *dirty = true;
                    *count += 1;
                }
                Claim::Bad => {
                    if st.opts.repair {
                        inode.set_block(i, 0);
                        *dirty = true;
                    }
                }
            }
        }

        for (slot, depth) in [(12usize, 1u32), (13, 2), (14, 3)] {
            let b = inode.block(slot);
            if b == 0 {
                continue;
            }
            match self.fsck_claim(st, ino, b)? {
                Claim::Kept => {
                    *count += 1;
                    self.fsck_walk_indirect(st, ino, b, depth, count)?;
                }
                Claim::Moved(new) => {
                    inode.set_block(slot, new);
                    *dirty = true;
                    *count += 1;
                    self.fsck_walk_indirect(st, ino, new, depth, count)?;
                }
                Claim::Bad => {
                    if st.opts.repair {
                        inode.set_block(slot, 0);
                        *dirty = true;
                    }
                }
            }
        }
        Ok(())
    }

    /// `block` is an already claimed indirect block. Depth 1 means its entries
    /// are data blocks, 2 indirect blocks, and so on, at most three levels
    fn fsck_walk_indirect(
        &mut self, st: &mut Fsck, ino: u32, block: u32, depth: u32, count: &mut u64,
    ) -> Result<(), FsError> {
        let bs = self.block_size as usize;
        let mut buf = block_buf(self.block_size);
        self.read_block_into(block, &mut buf)?;
        let mut changed = false;

        for slot in 0..bs / 4 {
            let at = slot * 4;
            let entry = u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]);
            if entry == 0 {
                continue;
            }
            let outcome = self.fsck_claim(st, ino, entry)?;
            let child = match outcome {
                Claim::Kept => {
                    *count += 1;
                    Some(entry)
                }
                Claim::Moved(new) => {
                    buf[at..at + 4].copy_from_slice(&new.to_le_bytes());
                    changed = true;
                    *count += 1;
                    Some(new)
                }
                Claim::Bad => {
                    if st.opts.repair {
                        buf[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
                        changed = true;
                    }
                    None
                }
            };
            if depth > 1 {
                if let Some(c) = child {
                    self.fsck_walk_indirect(st, ino, c, depth - 1, count)?;
                }
            }
        }

        if changed {
            self.write_meta_block(block, &buf)?;
        }
        Ok(())
    }

    // extent block maps

    fn fsck_walk_extent_root(
        &mut self, st: &mut Fsck, ino: u32, inode: &mut Inode,
        dirty: &mut bool, count: &mut u64,
    ) -> Result<(), FsError> {
        let root = inode.extent_header();
        if !root.check(EXTENT_ROOT_SPACE, None) {
            st.found(Problem::BadExtentHeader { ino, block: 0 });
            return Ok(());
        }

        if root.depth == 0 {
            // Entries live in the inode, so edits land there and the caller
            // writes it back
            let mut entries = root.entries;
            let mut i = 0usize;
            while i < entries as usize {
                let ext = inode.extent_at(i);
                match self.fsck_extent_record(st, ino, &ext, count)? {
                    Record::Kept => i += 1,
                    Record::Moved(new_start) => {
                        inode.set_extent_at_raw(i, ext.block, ext.len, 0, new_start);
                        *dirty = true;
                        i += 1;
                    }
                    Record::Drop => {
                        for j in i..entries as usize - 1 {
                            let at = 52 + j * 12;
                            let next = 52 + (j + 1) * 12;
                            let moved: [u8; 12] = inode.data[next..next + 12].try_into().unwrap();
                            inode.data[at..at + 12].copy_from_slice(&moved);
                        }
                        entries -= 1;
                        inode.set_extent_entries(entries);
                        *dirty = true;
                    }
                }
            }
            return Ok(());
        }

        let gen = inode.generation();
        let mut i = 0usize;
        while i < root.entries as usize {
            let child = inode.extent_idx_at(i).leaf() as u32;
            match self.fsck_claim(st, ino, child)? {
                Claim::Kept => {
                    *count += 1;
                    self.fsck_walk_extent_node(st, ino, gen, child, root.depth - 1, count)?;
                }
                Claim::Moved(new) => {
                    inode.set_extent_idx_at_raw(i, inode.extent_idx_at(i).block, new, 0);
                    *dirty = true;
                    *count += 1;
                    self.fsck_walk_extent_node(st, ino, gen, new, root.depth - 1, count)?;
                }
                Claim::Bad => {
                    // An unreachable subtree: only reportable. Its blocks
                    // become unowned and pass 5 frees them
                    st.found(Problem::BadExtentHeader { ino, block: child });
                }
            }
            i += 1;
        }
        Ok(())
    }

    fn fsck_walk_extent_node(
        &mut self, st: &mut Fsck, ino: u32, gen: u32, node: u32, depth: u16, count: &mut u64,
    ) -> Result<(), FsError> {
        if depth > EXTENT_MAX_DEPTH {
            st.found(Problem::BadExtentHeader { ino, block: node });
            return Ok(());
        }
        let bs = self.block_size as usize;
        let mut buf = block_buf(self.block_size);
        self.read_block_into(node, &mut buf)?;

        let magic = u16::from_le_bytes([buf[0], buf[1]]);
        let mut entries = u16::from_le_bytes([buf[2], buf[3]]);
        let max = u16::from_le_bytes([buf[4], buf[5]]);
        let node_depth = u16::from_le_bytes([buf[6], buf[7]]);
        if magic != EXT4_EXT_MAGIC
            || node_depth != depth
            || max == 0
            || entries > max
            || 12 + max as usize * 12 > bs
        {
            st.found(Problem::BadExtentHeader { ino, block: node });
            return Ok(());
        }
        if !self.verify_extent_block(ino, gen, &buf) {
            st.found(Problem::BadExtentChecksum { ino, block: node });
        }

        let mut changed = false;
        let mut i = 0usize;
        while i < entries as usize {
            let at = 12 + i * 12;
            if depth > 0 {
                let child = u32::from_le_bytes([buf[at + 4], buf[at + 5], buf[at + 6], buf[at + 7]]);
                match self.fsck_claim(st, ino, child)? {
                    Claim::Kept => {
                        *count += 1;
                        self.fsck_walk_extent_node(st, ino, gen, child, depth - 1, count)?;
                    }
                    Claim::Moved(new) => {
                        buf[at + 4..at + 8].copy_from_slice(&new.to_le_bytes());
                        changed = true;
                        *count += 1;
                        self.fsck_walk_extent_node(st, ino, gen, new, depth - 1, count)?;
                    }
                    Claim::Bad => st.found(Problem::BadExtentHeader { ino, block: child }),
                }
                i += 1;
                continue;
            }

            let ext = Extent {
                block: u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]),
                len: u16::from_le_bytes([buf[at + 4], buf[at + 5]]),
                start_hi: u16::from_le_bytes([buf[at + 6], buf[at + 7]]),
                start_lo: u32::from_le_bytes([buf[at + 8], buf[at + 9], buf[at + 10], buf[at + 11]]),
            };
            match self.fsck_extent_record(st, ino, &ext, count)? {
                Record::Kept => i += 1,
                Record::Moved(new_start) => {
                    buf[at + 6..at + 8].copy_from_slice(&0u16.to_le_bytes());
                    buf[at + 8..at + 12].copy_from_slice(&new_start.to_le_bytes());
                    changed = true;
                    i += 1;
                }
                Record::Drop => {
                    for j in i..entries as usize - 1 {
                        let dst = 12 + j * 12;
                        let src = 12 + (j + 1) * 12;
                        let moved: [u8; 12] = buf[src..src + 12].try_into().unwrap();
                        buf[dst..dst + 12].copy_from_slice(&moved);
                    }
                    entries -= 1;
                    buf[2..4].copy_from_slice(&entries.to_le_bytes());
                    changed = true;
                }
            }
        }

        if changed {
            self.write_extent_node(ino, gen, node, &mut buf)?;
        }
        Ok(())
    }

    /// One leaf record. Extent-mapped files are repaired per record, as a
    /// record names one contiguous run; a mid-run fault would need a split and
    /// a tree grow on a damaged filesystem
    fn fsck_extent_record(
        &mut self, st: &mut Fsck, ino: u32, ext: &Extent, count: &mut u64,
    ) -> Result<Record, FsError> {
        let start = ext.start() as u32;
        let len = ext.actual_len();
        if len == 0 {
            return Ok(Record::Drop);
        }

        let mut unusable = None;
        let mut shared = false;
        for b in start..start + len {
            match st.block_bit(b) {
                None => {
                    unusable = Some(Problem::BlockOutOfRange { ino, block: b });
                    break;
                }
                Some(bit) => {
                    if st.meta.get(bit) && ino != EXT2_RESIZE_INO {
                        unusable = Some(Problem::BlockInMetadata { ino, block: b });
                        break;
                    }
                    if st.blocks.get(bit) {
                        shared = true;
                    }
                }
            }
        }

        if let Some(problem) = unusable {
            st.note(problem, st.opts.repair);
            return Ok(if st.opts.repair { Record::Drop } else { Record::Kept });
        }

        if shared {
            let moved = if st.opts.repair {
                self.fsck_clone_run(st, start, len)?
            } else {
                None
            };
            match moved {
                Some(new_start) => {
                    st.fixed(Problem::BlockClaimedTwice { ino, block: start });
                    for b in new_start..new_start + len {
                        if let Some(bit) = st.block_bit(b) {
                            st.blocks.set(bit);
                        }
                    }
                    *count += len as u64;
                    return Ok(Record::Moved(new_start));
                }
                None => {
                    st.found(Problem::BlockClaimedTwice { ino, block: start });
                }
            }
        }

        for b in start..start + len {
            if let Some(bit) = st.block_bit(b) {
                if st.blocks.set(bit) {
                    st.dup.set(bit);
                }
            }
        }
        *count += len as u64;
        Ok(Record::Kept)
    }

    // claiming

    /// Records that `block` belongs to `ino` and says if the pointer can stay
    fn fsck_claim(&mut self, st: &mut Fsck, ino: u32, block: u32) -> Result<Claim, FsError> {
        let Some(bit) = st.block_bit(block) else {
            st.note(Problem::BlockOutOfRange { ino, block }, st.opts.repair);
            return Ok(Claim::Bad);
        };

        // The resize inode's double-indirect block points at the reserved group
        // descriptor blocks, metadata by design, the one correct overlap.
        //
        // Those are walked as indirect blocks: e2fsprogs builds the resize
        // inode so each reserved descriptor block in group 0 is the indirect
        // block naming its copies in each group with a superblock, and
        // `i_blocks` counts all of it
        if st.meta.get(bit) {
            if ino == EXT2_RESIZE_INO {
                return Ok(Claim::Kept);
            }
            st.note(Problem::BlockInMetadata { ino, block }, st.opts.repair);
            return Ok(Claim::Bad);
        }

        if !st.blocks.set(bit) {
            return Ok(Claim::Kept);
        }

        // Claimed first by another. This inode gets a copy, without deciding
        // which owner is wrong
        st.dup.set(bit);
        if st.opts.repair {
            if let Some(new) = self.fsck_clone_run(st, block, 1)? {
                if let Some(nbit) = st.block_bit(new) {
                    st.blocks.set(nbit);
                }
                st.fixed(Problem::BlockClaimedTwice { ino, block });
                return Ok(Claim::Moved(new));
            }
        }
        st.found(Problem::BlockClaimedTwice { ino, block });
        Ok(Claim::Kept)
    }

    /// A private copy of `len` blocks from `start`. The run must be contiguous
    /// for the record. The allocator gives ascending blocks within a group,
    /// usually giving that; otherwise they are returned and the fault reported
    fn fsck_clone_run(
        &mut self, st: &mut Fsck, start: u32, len: u32,
    ) -> Result<Option<u32>, FsError> {
        let group = ((start.saturating_sub(st.first_block)) / self.blocks_per_group) as usize;
        let mut taken: Vec<u32> = Vec::new();
        let mut contiguous = true;

        for _ in 0..len {
            match self.alloc_block(group.min(self.groups.len().saturating_sub(1))) {
                Ok(b) => {
                    if let Some(&last) = taken.last() {
                        if b != last + 1 {
                            contiguous = false;
                        }
                    }
                    taken.push(b);
                }
                Err(_) => {
                    contiguous = false;
                    break;
                }
            }
        }

        if !contiguous || taken.len() != len as usize {
            for b in taken {
                self.free_block(b)?;
            }
            return Ok(None);
        }

        let new_start = taken[0];
        let mut buf = block_buf(self.block_size);
        for i in 0..len {
            self.read_block_into(start + i, &mut buf)?;
            self.write_block_data(new_start + i, &buf)?;
        }
        Ok(Some(new_start))
    }

    // extended attributes

    fn fsck_claim_xattr(
        &mut self, st: &mut Fsck, ino: u32, acl: u64, refs: &mut BTreeMap<u32, u32>,
    ) -> Result<u64, FsError> {
        if acl > u32::MAX as u64 {
            st.found(Problem::BlockOutOfRange { ino, block: u32::MAX });
            return Ok(0);
        }
        let block = acl as u32;
        let seen = refs.entry(block).or_insert(0);
        *seen += 1;
        if *seen > 1 {
            // Shared on purpose: claimed once, counted each time
            return Ok(1);
        }
        match self.fsck_claim(st, ino, block)? {
            Claim::Kept | Claim::Moved(_) => Ok(1),
            Claim::Bad => Ok(0),
        }
    }

    fn fsck_check_xattr_refs(
        &mut self, st: &mut Fsck, refs: &BTreeMap<u32, u32>,
    ) -> Result<(), FsError> {
        if refs.is_empty() {
            return Ok(());
        }
        let mut buf = block_buf(self.block_size);
        for (&block, &counted) in refs.iter() {
            if st.block_bit(block).is_none() {
                continue;
            }
            if self.read_block_into(block, &mut buf).is_err() {
                continue;
            }
            let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
            if magic != crate::xattr::XATTR_MAGIC {
                continue;
            }
            let on_disk = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
            if on_disk == counted {
                continue;
            }
            let fixed = if st.opts.repair {
                buf[4..8].copy_from_slice(&counted.to_le_bytes());
                self.stamp_xattr_block(block as u64, &mut buf);
                self.write_meta_block(block, &buf).is_ok()
            } else {
                false
            };
            st.note(Problem::XattrRefcountWrong { block, on_disk, counted }, fixed);
        }
        Ok(())
    }
}

/// What should happen to one extent record
enum Record {
    Kept,
    /// Its contents now live at this block
    Moved(u32),
    /// It maps nothing usable and should go
    Drop,
}
