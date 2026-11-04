// An open volume and walking a tree on it
//
// The superblock says where trees are, the chunk map where a logical address is
// on a device, and the node parser if the result is a tree block.
//
// One device. A logical address maps to `(devid, offset)`, and each is checked
// against this volume's device. A multi-device filesystem is not refused at
// mount; it fails at the first chunk elsewhere, with `NotFound`

use crate::chunk::{map_from_sys_array, Chunk, ChunkMap, Physical};
use crate::error::BtrfsError;
use crate::mount::{check_superblock, Geometry, Superblock};
use crate::node::{parse_node, Node};
use crate::structs::*;
use alloc::vec::Vec;
use fshost::BlockHost;

/// Deepest descent before it counts as a loop. A corrupted interior node can
/// point at an ancestor, despite the superblock's `MAX_LEVEL` check
const MAX_DESCENT: usize = MAX_LEVEL as usize + 1;

/// A btrfs filesystem, open for reading
pub struct Volume<H: BlockHost> {
    host: H,
    /// Volume start on the device in 512-byte sectors, non-zero in a partition
    start_lba: u64,
    pub superblock: Superblock,
    pub geometry: Geometry,
    pub chunks: ChunkMap,
    /// The opened device, as btrfs numbers it
    devid: u64,
    /// The checksum tree, found on first use, as its lookup walks the root tree
    csum_root: Option<u64>,
    /// The default subvolume's tree, likewise
    fs_root: Option<u64>,
}

impl<H: BlockHost> Volume<H> {
    /// Opens a volume: reads and judges the superblock and builds the bootstrap
    /// chunk map. The chunk tree needs this map; `load_chunk_tree` is the next
    /// step
    pub fn open(mut host: H, start_lba: u64) -> Result<Self, BtrfsError> {
        let mut sb = Superblock::zeroed();
        read_raw(&mut host, start_lba, SUPER_INFO_OFFSET, &mut sb.data)?;

        let geometry = check_superblock(&sb, Some(SUPER_INFO_OFFSET)).map_err(|_| {
            // The reason is in `MountReject`, through `check_superblock`
            BtrfsError::Unsupported
        })?;

        let chunks = map_from_sys_array(&sb, geometry.sectorsize)?;

        // The device of the bootstrap chunks is ours. From the chunks, so a
        // numbering disagreement fails at the first read
        let devid = chunks
            .iter()
            .next()
            .and_then(|c| c.stripes.first())
            .map(|s| s.devid)
            .ok_or(BtrfsError::Corrupt)?;

        Ok(Volume { host, start_lba, superblock: sb, geometry, chunks, devid, csum_root: None, fs_root: None })
    }

    pub fn label(&self) -> &str {
        self.superblock.label()
    }

    /// The host underneath, for a test damaging the medium
    pub fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    /// Reads `buf.len()` bytes from a logical address through the chunk map.
    /// Refuses a read crossing out of its chunk: halves may be on different
    /// devices, and a tree block never straddles a chunk
    pub fn read_logical(&mut self, logical: u64, buf: &mut [u8]) -> Result<(), BtrfsError> {
        let phys = self.chunks.map(logical, 0)?;
        self.read_mapped(&phys, buf)
    }

    fn read_mapped(&mut self, phys: &Physical, buf: &mut [u8]) -> Result<(), BtrfsError> {
        if phys.devid != self.devid {
            return Err(BtrfsError::NotFound);
        }
        if (buf.len() as u64) > phys.len {
            return Err(BtrfsError::Corrupt);
        }
        read_raw(&mut self.host, self.start_lba, phys.offset, buf)
    }

    /// Reads and checks one tree block. On a mirrored chunk a failing block is
    /// tried on the next copy, as DUP allows on one disk
    pub fn read_node(&mut self, logical: u64) -> Result<Node, BtrfsError> {
        let nodesize = self.geometry.nodesize as usize;
        let sectorsize = self.geometry.sectorsize;
        let fsid = *<&[u8; FSID_SIZE]>::try_from(self.superblock.fsid()).unwrap();

        let copies = {
            let chunk: &Chunk = self.chunks.find(logical).ok_or(BtrfsError::NotFound)?;
            chunk.profile.copies()
        };

        let mut last = BtrfsError::NotFound;
        for mirror in 0..copies {
            let phys = match self.chunks.map(logical, mirror) {
                Ok(p) => p,
                Err(e) => {
                    last = e;
                    continue;
                }
            };
            let mut buf = Vec::new();
            if buf.try_reserve_exact(nodesize).is_err() {
                return Err(BtrfsError::OutOfMemory);
            }
            buf.resize(nodesize, 0);

            if let Err(e) = self.read_mapped(&phys, &mut buf) {
                last = e;
                continue;
            }
            match parse_node(buf, logical, &fsid, sectorsize) {
                Ok(n) => return Ok(n),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// Reads the chunk tree into the map, which then covers the whole volume
    pub fn load_chunk_tree(&mut self) -> Result<(), BtrfsError> {
        let root = self.geometry.chunk_root;
        let sectorsize = self.geometry.sectorsize;

        let mut found = Vec::new();
        self.for_each_item(root, &mut |key, data| {
            if key.kind != CHUNK_ITEM_KEY {
                return Ok(true);
            }
            let c = crate::chunk::parse_chunk(data, 0, key.offset, sectorsize)?;
            found.try_reserve(1).map_err(|_| BtrfsError::OutOfMemory)?;
            found.push(c);
            Ok(true)
        })?;

        for c in found {
            self.chunks.insert(c)?;
        }
        Ok(())
    }

    /// Passes each item in a tree to `f`, in key order, stopping on false. A
    /// callback, since an iterator would hold a volume borrow while the descent
    /// needs `&mut`
    pub fn for_each_item(
        &mut self,
        root: u64,
        f: &mut dyn FnMut(Key, &[u8]) -> Result<bool, BtrfsError>,
    ) -> Result<(), BtrfsError> {
        self.walk_from(root, &Key::default(), f)
    }

    /// The same from the first key not less than `from`
    pub fn walk_from(
        &mut self,
        root: u64,
        from: &Key,
        f: &mut dyn FnMut(Key, &[u8]) -> Result<bool, BtrfsError>,
    ) -> Result<(), BtrfsError> {
        // Path to the leaf holding `from`, as (node, slot) pairs, so running
        // off a leaf can climb and find the next
        let mut path: Vec<(Node, u32)> = Vec::new();
        let mut logical = root;

        for _ in 0..MAX_DESCENT {
            let node = self.read_node(logical)?;
            if node.is_leaf() {
                let slot = match node.search(from)? {
                    Ok(exact) => exact,
                    Err(next) => next,
                };
                path.try_reserve(1).map_err(|_| BtrfsError::OutOfMemory)?;
                path.push((node, slot));
                return self.walk_forward(path, f);
            }
            // A node's key is the smallest under its pointer, so the subtree is
            // before the first greater key. `Err(0)` means all keys exceed the
            // target: start at the leftmost child
            let slot = match node.search(from)? {
                Ok(exact) => exact,
                Err(0) => 0,
                Err(next) => next - 1,
            };
            logical = node.blockptr_at(slot)?;
            path.try_reserve(1).map_err(|_| BtrfsError::OutOfMemory)?;
            path.push((node, slot));
        }
        // Deeper than the format allows: a tree pointing into itself
        Err(BtrfsError::Corrupt)
    }

    /// Walk leaves forward from the position `path` describes
    fn walk_forward(
        &mut self,
        mut path: Vec<(Node, u32)>,
        f: &mut dyn FnMut(Key, &[u8]) -> Result<bool, BtrfsError>,
    ) -> Result<(), BtrfsError> {
        loop {
            {
                let (leaf, slot) = path.last_mut().ok_or(BtrfsError::Corrupt)?;
                while *slot < leaf.nritems {
                    let key = leaf.key_at(*slot)?;
                    let data = leaf.item_data(*slot)?;
                    *slot += 1;
                    if !f(key, data)? {
                        return Ok(());
                    }
                }
            }

            // The leaf is spent. Climbs until a parent has another child, then
            // descends its left edge. Otherwise the tree is finished
            path.pop();
            let mut next_logical = None;
            while let Some((node, slot)) = path.last_mut() {
                *slot += 1;
                if *slot < node.nritems {
                    next_logical = Some(node.blockptr_at(*slot)?);
                    break;
                }
                path.pop();
            }
            let Some(mut logical) = next_logical else {
                return Ok(());
            };

            for _ in 0..MAX_DESCENT {
                let node = self.read_node(logical)?;
                let leaf = node.is_leaf();
                let next = if leaf { 0 } else { node.blockptr_at(0)? };
                path.try_reserve(1).map_err(|_| BtrfsError::OutOfMemory)?;
                path.push((node, 0));
                if leaf {
                    break;
                }
                logical = next;
            }
            if path.is_empty() || !path.last().unwrap().0.is_leaf() {
                return Err(BtrfsError::Corrupt);
            }
        }
    }

    /// Finds one item by exact key. Returns its bytes copied, as the node is
    /// dropped when the descent unwinds
    pub fn find_item(&mut self, root: u64, key: &Key) -> Result<Vec<u8>, BtrfsError> {
        let mut out: Option<Vec<u8>> = None;
        let want = *key;
        self.walk_from(root, key, &mut |k, data| {
            if k == want {
                let mut v = Vec::new();
                v.try_reserve_exact(data.len())
                    .map_err(|_| BtrfsError::OutOfMemory)?;
                v.extend_from_slice(data);
                out = Some(v);
            }
            // One item: the walk starts at `key`, so anything later is past it
            Ok(false)
        })?;
        out.ok_or(BtrfsError::NotFound)
    }
}

/// Reads bytes at a volume byte offset through the host's sectors. Handles an
/// unaligned start, though btrfs offsets are normally sector-aligned
fn read_raw<H: BlockHost>(
    host: &mut H,
    start_lba: u64,
    offset: u64,
    buf: &mut [u8],
) -> Result<(), BtrfsError> {
    if buf.is_empty() {
        return Ok(());
    }
    let first = offset / 512;
    let skew = (offset % 512) as usize;
    let sectors = (skew + buf.len()).div_ceil(512);

    if skew == 0 && buf.len() % 512 == 0 {
        host.read_sectors(start_lba + first, sectors as u32, buf)?;
        return Ok(());
    }

    let mut tmp = Vec::new();
    tmp.try_reserve_exact(sectors * 512)
        .map_err(|_| BtrfsError::OutOfMemory)?;
    tmp.resize(sectors * 512, 0);
    host.read_sectors(start_lba + first, sectors as u32, &mut tmp)?;
    buf.copy_from_slice(&tmp[skew..skew + buf.len()]);
    Ok(())
}

/// A volume over a borrowed host, so a test can build and then open an image.
/// `BlockHost` is implemented for `&mut H`
pub type BorrowedVolume<'a, H> = Volume<&'a mut H>;

// The filesystem over its trees
//
// The questions asked of a filesystem: the root, an inode, a name. Each is one
// search or walk

use crate::fs_tree::{
    parse_dir_records, DirEntry, Inode, Root, DIR_INDEX_KEY, DIR_ITEM_KEY, FS_TREE_OBJECTID,
    INODE_ITEM_KEY, ROOT_ITEM_KEY, XATTR_ITEM_KEY,
};

impl<H: BlockHost> Volume<H> {
    /// Finds a subvolume's tree in the root tree. `objectid` is its id;
    /// `FS_TREE_OBJECTID` is the default. The key's offset is a generation, so
    /// this looks for the first `ROOT_ITEM` of the objectid, a walk and not
    /// `find_item`
    pub fn find_root(&mut self, objectid: u64) -> Result<Root, BtrfsError> {
        let tree = self.geometry.root;
        let from = Key { objectid, kind: ROOT_ITEM_KEY, offset: 0 };

        let mut found: Option<Result<Root, BtrfsError>> = None;
        self.walk_from(tree, &from, &mut |k, data| {
            if k.objectid != objectid || k.kind != ROOT_ITEM_KEY {
                return Ok(false);
            }
            found = Some(Root::parse(data));
            Ok(false)
        })?;
        found.ok_or(BtrfsError::NotFound)?
    }

    /// The default subvolume's tree
    pub fn fs_root(&mut self) -> Result<Root, BtrfsError> {
        self.find_root(FS_TREE_OBJECTID)
    }

    /// The default subvolume's tree block, found once and kept. Callers
    /// identify a node by `(tree, objectid)`, and passing zero means the
    /// default, as tree 0 does not exist
    pub fn default_tree(&mut self) -> Result<u64, BtrfsError> {
        if let Some(t) = self.fs_root {
            return Ok(t);
        }
        let r = self.fs_root()?;
        self.fs_root = Some(r.bytenr);
        Ok(r.bytenr)
    }

    /// Resolve a tree word, treating zero as the default subvolume
    pub fn tree_or_default(&mut self, tree: u64) -> Result<u64, BtrfsError> {
        if tree == 0 {
            self.default_tree()
        } else {
            Ok(tree)
        }
    }

    /// One inode's metadata
    pub fn inode(&mut self, tree: u64, ino: u64) -> Result<Inode, BtrfsError> {
        let key = Key { objectid: ino, kind: INODE_ITEM_KEY, offset: 0 };
        let data = self.find_item(tree, &key)?;
        Inode::parse(&data)
    }

    /// Finds a name in a directory with one search: the key's offset is the
    /// name's hash. The item is a list of colliding names, so the name is still
    /// compared
    pub fn lookup(
        &mut self,
        tree: u64,
        dir: u64,
        name: &[u8],
    ) -> Result<DirEntry, BtrfsError> {
        let key = Key {
            objectid: dir,
            kind: DIR_ITEM_KEY,
            offset: crate::fs_tree::name_hash(name),
        };
        let item = self.find_item(tree, &key)?;
        let records = parse_dir_records(&key, &item)?;
        records
            .into_iter()
            .find(|e| e.name == name)
            .ok_or(BtrfsError::NotFound)
    }

    /// Every name in a directory, in btrfs's order. `DIR_INDEX`, keyed by
    /// position, for a stable order; `DIR_ITEM` would follow a checksum
    pub fn readdir(
        &mut self,
        tree: u64,
        dir: u64,
        f: &mut dyn FnMut(&DirEntry) -> Result<bool, BtrfsError>,
    ) -> Result<(), BtrfsError> {
        let from = Key { objectid: dir, kind: DIR_INDEX_KEY, offset: 0 };
        let mut inner: Result<(), BtrfsError> = Ok(());

        self.walk_from(tree, &from, &mut |k, data| {
            if k.objectid != dir || k.kind != DIR_INDEX_KEY {
                return Ok(false);
            }
            let records = match parse_dir_records(&k, data) {
                Ok(r) => r,
                Err(e) => {
                    inner = Err(e);
                    return Ok(false);
                }
            };
            for e in &records {
                match f(e) {
                    Ok(true) => {}
                    Ok(false) => return Ok(false),
                    Err(e) => {
                        inner = Err(e);
                        return Ok(false);
                    }
                }
            }
            Ok(true)
        })?;
        inner
    }

    /// Every extended attribute on an inode, the same on-disk shape as a
    /// directory entry
    pub fn xattrs(
        &mut self,
        tree: u64,
        ino: u64,
        f: &mut dyn FnMut(&DirEntry) -> Result<bool, BtrfsError>,
    ) -> Result<(), BtrfsError> {
        let from = Key { objectid: ino, kind: XATTR_ITEM_KEY, offset: 0 };
        let mut inner: Result<(), BtrfsError> = Ok(());

        self.walk_from(tree, &from, &mut |k, data| {
            if k.objectid != ino || k.kind != XATTR_ITEM_KEY {
                return Ok(false);
            }
            let records = match parse_dir_records(&k, data) {
                Ok(r) => r,
                Err(e) => {
                    inner = Err(e);
                    return Ok(false);
                }
            };
            for e in &records {
                match f(e) {
                    Ok(true) => {}
                    Ok(false) => return Ok(false),
                    Err(e) => {
                        inner = Err(e);
                        return Ok(false);
                    }
                }
            }
            Ok(true)
        })?;
        inner
    }
}

// Reading a file
//
// An inode and byte range in, bytes out, through the fs tree for extents, the
// chunk map for location, and the checksum tree for verification

use crate::file::{csum_key, parse_extent, Extent, CSUM_BYTES};
use crate::fs_tree::EXTENT_DATA_KEY;

impl<H: BlockHost> Volume<H> {
    /// Reads from a file at `offset` into `buf`, returning the amount read.
    /// Short at end of file, zero past it, as `read(2)`.
    ///
    /// Holes and preallocated ranges read as zeros. Preallocated must: its disk
    /// space holds stale data, perhaps a deleted file
    pub fn read_file(
        &mut self,
        tree: u64,
        ino: u64,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, BtrfsError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let size = self.inode(tree, ino)?.size;
        if offset >= size {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(size - offset) as usize;
        let sectorsize = self.geometry.sectorsize as u64;

        // Anything not covered by an extent is a hole. Starting from zeros
        // handles `NO_HOLES` with no special case
        buf[..want].fill(0);

        // From the file's first extent, not `offset`: an extent often starts
        // before its position, and `walk_from` lands on the first key at or
        // above its input, skipping the wanted record. The cost is walking
        // earlier records
        let from = Key { objectid: ino, kind: EXTENT_DATA_KEY, offset: 0 };

        // Collected, as reading needs `&mut self`, held by the walk
        let mut plan: Vec<(u64, Extent, Vec<u8>)> = Vec::new();
        let end = offset + want as u64;
        let mut inner: Result<(), BtrfsError> = Ok(());

        self.walk_from(tree, &from, &mut |k, data| {
            if k.objectid != ino || k.kind != EXTENT_DATA_KEY {
                return Ok(false);
            }
            if k.offset >= end {
                return Ok(false);
            }
            let ext = match parse_extent(&k, data, sectorsize as u32) {
                Ok(e) => e,
                Err(e) => {
                    inner = Err(e);
                    return Ok(false);
                }
            };
            if k.offset + ext.covers() <= offset {
                return Ok(true);
            }
            // An inline extent's bytes are in the item, which does not outlive
            // this callback
            let inline = match ext {
                Extent::Inline { data_at, data_len, .. } => {
                    data.get(data_at..data_at + data_len).unwrap_or(&[]).to_vec()
                }
                _ => Vec::new(),
            };
            if plan.try_reserve(1).is_err() {
                inner = Err(BtrfsError::OutOfMemory);
                return Ok(false);
            }
            plan.push((k.offset, ext, inline));
            Ok(true)
        })?;
        inner?;

        for (start, ext, inline) in plan {
            // Where the wanted range and this extent overlap
            let ext_end = start + ext.covers();
            let from_pos = offset.max(start);
            let to_pos = end.min(ext_end);
            if to_pos <= from_pos {
                continue;
            }
            let into = (from_pos - offset) as usize;
            let len = (to_pos - from_pos) as usize;
            let within = from_pos - start;

            match ext {
                // Already zeros
                Extent::Hole { .. } | Extent::Prealloc { .. } => {}
                Extent::Inline { .. } => {
                    let src = inline
                        .get(within as usize..within as usize + len)
                        .ok_or(BtrfsError::Corrupt)?;
                    buf[into..into + len].copy_from_slice(src);
                }
                Extent::Regular { logical, .. } => {
                    self.read_data(logical + within, &mut buf[into..into + len])?;
                }
            }
        }
        Ok(want)
    }

    /// Reads file data at a logical address, checked against the checksum tree.
    /// Checksums are per sector, so a read off a sector boundary goes through a
    /// sector buffer. A sector with no checksum item is not an error:
    /// `nodatasum` files and preallocated space have none
    fn read_data(&mut self, logical: u64, buf: &mut [u8]) -> Result<(), BtrfsError> {
        let ss = self.geometry.sectorsize as u64;
        let mut done = 0usize;

        while done < buf.len() {
            let at = logical + done as u64;
            let sector = (at / ss) * ss;
            let skew = (at - sector) as usize;
            let take = (buf.len() - done).min(ss as usize - skew);

            let mut whole = Vec::new();
            whole
                .try_reserve_exact(ss as usize)
                .map_err(|_| BtrfsError::OutOfMemory)?;
            whole.resize(ss as usize, 0);
            self.read_logical(sector, &mut whole)?;
            self.verify_sector(sector, &whole)?;

            buf[done..done + take].copy_from_slice(&whole[skew..skew + take]);
            done += take;
        }
        Ok(())
    }

    /// Checks one sector against the checksum tree, if it has one. A mismatch
    /// is an error, not a warning
    fn verify_sector(&mut self, logical: u64, data: &[u8]) -> Result<(), BtrfsError> {
        let csum_root = match self.csum_root {
            Some(r) => r,
            None => match self.find_root(crate::fs_tree::CSUM_TREE_OBJECTID) {
                Ok(r) => {
                    self.csum_root = Some(r.bytenr);
                    r.bytenr
                }
                // No checksum tree: a volume state, not a fault
                Err(_) => return Ok(()),
            },
        };

        let ss = self.geometry.sectorsize as u64;
        let key = csum_key(logical);
        let mut stored: Option<u32> = None;

        // The covering item may start earlier: one holds checksums for a run of
        // consecutive sectors
        let from = Key { offset: 0, ..key };
        let mut inner: Result<(), BtrfsError> = Ok(());
        self.walk_from(csum_root, &from, &mut |k, item| {
            if k.objectid != key.objectid || k.kind != key.kind {
                return Ok(false);
            }
            let covered = (item.len() / CSUM_BYTES) as u64 * ss;
            if k.offset + covered <= logical {
                return Ok(true);
            }
            if k.offset > logical {
                return Ok(false);
            }
            let index = ((logical - k.offset) / ss) as usize;
            match u32_at(item, index * CSUM_BYTES) {
                Ok(v) => stored = Some(v),
                Err(e) => inner = Err(e),
            }
            Ok(false)
        })?;
        inner?;

        let Some(stored) = stored else {
            // `nodatasum` or preallocated: no checksum was written
            return Ok(());
        };
        if stored != fshost::crc32c::crc32c(0, data) {
            return Err(BtrfsError::ChecksumMismatch);
        }
        Ok(())
    }
}
