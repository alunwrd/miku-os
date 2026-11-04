// The vnodes the cache could give back, most recently used first
// (`dev/vfs-dentries-inodes.md`, "Stage 4: the table grows and shrinks")
//
// Intrusive: the links are in the vnode (`lru_prev`, `lru_next`, `on_lru`), so
// putting a vnode at the head, taking it out or reading the tail is O(1) and
// allocates nothing. Every vnode joins at the head when it is made
// (`init_vnode`) and moves there when a lookup finds it. Reclaim reads from the
// tail: what can go goes, what is busy for now goes back to the head (Linux's
// `LRU_ROTATE`), and what can never go (tmpfs, an unlinked file) leaves the
// list.

use crate::fs::vfs::table::Table;
use crate::fs::vfs::types::{InodeId, INVALID_ID};
use crate::fs::vfs::vnode::VNode;

pub struct Lru {
    head: InodeId,
    tail: InodeId,
    len: usize,
}

impl Lru {
    pub const fn new() -> Self {
        Self { head: INVALID_ID, tail: INVALID_ID, len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// The least recently used, if any
    pub fn tail(&self) -> Option<usize> {
        (self.tail != INVALID_ID).then_some(self.tail as usize)
    }

    /// Out of the list, if it is on it
    pub fn remove(&mut self, nodes: &mut Table<VNode>, id: usize) {
        if !nodes.contains(id) || !nodes[id].on_lru {
            return;
        }
        let (prev, next) = (nodes[id].lru_prev, nodes[id].lru_next);
        if prev == INVALID_ID {
            self.head = next;
        } else {
            nodes[prev as usize].lru_next = next;
        }
        if next == INVALID_ID {
            self.tail = prev;
        } else {
            nodes[next as usize].lru_prev = prev;
        }
        let n = &mut nodes[id];
        n.on_lru = false;
        n.lru_prev = INVALID_ID;
        n.lru_next = INVALID_ID;
        self.len -= 1;
    }

    /// To the head: the most recently used
    pub fn push_head(&mut self, nodes: &mut Table<VNode>, id: usize) {
        if !nodes.contains(id) {
            return;
        }
        if nodes[id].on_lru {
            if self.head == id as InodeId {
                return;
            }
            self.remove(nodes, id);
        }
        let old = self.head;
        {
            let n = &mut nodes[id];
            n.on_lru = true;
            n.lru_prev = INVALID_ID;
            n.lru_next = old;
        }
        if old == INVALID_ID {
            self.tail = id as InodeId;
        } else {
            nodes[old as usize].lru_prev = id as InodeId;
        }
        self.head = id as InodeId;
        self.len += 1;
    }
}
