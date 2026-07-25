use crate::fs::vfs::types::INVALID_ID;

#[derive(Clone, Copy)]
pub struct LruNode {
    pub prev: u16,
    pub next: u16,
    pub in_lru: bool,
}

impl LruNode {
    pub const fn empty() -> Self {
        Self {
            prev: INVALID_ID,
            next: INVALID_ID,
            in_lru: false,
        }
    }
}

pub struct LruList<const N: usize> {
    pub nodes: [LruNode; N],
    pub head: u16,
    pub tail: u16,
    pub count: u16,
}

impl<const N: usize> LruList<N> {
    pub const fn new() -> Self {
        Self {
            nodes: [LruNode::empty(); N],
            head: INVALID_ID,
            tail: INVALID_ID,
            count: 0,
        }
    }

    pub fn push_front(&mut self, idx: u16) {
        let i = idx as usize;
        if i >= N {
            return;
        }

        if self.nodes[i].in_lru {
            self.remove(idx);
        }

        self.nodes[i].prev = INVALID_ID;
        self.nodes[i].next = self.head;
        self.nodes[i].in_lru = true;

        if self.head != INVALID_ID {
            self.nodes[self.head as usize].prev = idx;
        }
        self.head = idx;
        if self.tail == INVALID_ID {
            self.tail = idx;
        }

        self.count = self.count.saturating_add(1);
    }

    pub fn remove(&mut self, idx: u16) {
        let i = idx as usize;
        if i >= N || !self.nodes[i].in_lru {
            return;
        }

        let prev = self.nodes[i].prev;
        let next = self.nodes[i].next;

        if prev != INVALID_ID {
            self.nodes[prev as usize].next = next;
        } else {
            self.head = next;
        }

        if next != INVALID_ID {
            self.nodes[next as usize].prev = prev;
        } else {
            self.tail = prev;
        }

        self.nodes[i].in_lru = false;
        self.nodes[i].prev = INVALID_ID;
        self.nodes[i].next = INVALID_ID;

        self.count = self.count.saturating_sub(1);
    }

    pub fn touch(&mut self, idx: u16) {
        if (idx as usize) >= N {
            return;
        }
        self.push_front(idx);
    }

    pub fn pop_lru(&mut self) -> Option<u16> {
        if self.tail == INVALID_ID {
            return None;
        }
        let idx = self.tail;
        self.remove(idx);
        Some(idx)
    }

    pub fn peek_lru(&self) -> Option<u16> {
        if self.tail == INVALID_ID {
            None
        } else {
            Some(self.tail)
        }
    }

    pub fn peek_mru(&self) -> Option<u16> {
        if self.head == INVALID_ID {
            None
        } else {
            Some(self.head)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn len(&self) -> usize {
        self.count as usize
    }

    pub fn iter(&self) -> LruIter<'_, N> {
        LruIter {
            list: self,
            current: self.head,
        }
    }

    #[cfg(debug_assertions)]
    pub fn assert_consistent(&self) {
        let mut counted = 0u16;
        let mut cur = self.head;
        while cur != INVALID_ID {
            let i = cur as usize;
            assert!(i < N, "lru: node index out of bounds");
            assert!(self.nodes[i].in_lru, "lru: node not marked in_lru");
            counted += 1;
            assert!(counted <= N as u16, "lru: cycle detected");
            cur = self.nodes[i].next;
        }
        assert_eq!(counted, self.count, "lru: count mismatch");
    }
}

pub struct LruIter<'a, const N: usize> {
    list: &'a LruList<N>,
    current: u16,
}

impl<'a, const N: usize> Iterator for LruIter<'a, N> {
    type Item = u16;

    fn next(&mut self) -> Option<u16> {
        if self.current == INVALID_ID {
            return None;
        }
        let idx = self.current;
        self.current = self.list.nodes[idx as usize].next;
        Some(idx)
    }
}
