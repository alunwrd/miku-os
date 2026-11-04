// Object table with reusable slots and generation-tagged ids
//
// An id holds the slot and its generation, so an id from before a slot was
// freed no longer matches and `get` reports it gone.
//
// Indexing (`objs[oid]`) is for ids the table keeps itself (`children`, binding
// targets), which destruction cleans up. Ids from outside (JavaScript, property
// values, the pointer's last hit) go through `get`.

use std::ops::{Index, IndexMut};

use super::value::Oid;
use super::Obj;

/// Low bits are the slot, high bits the generation: 1M slots, 4096 uses of each
const SLOT_BITS: u32 = 20;
const SLOT_MASK: u32 = (1 << SLOT_BITS) - 1;
const GEN_MASK: u32 = (1 << (32 - SLOT_BITS)) - 1;

struct Slot {
    gen: u32,
    obj: Option<Obj>,
}

#[derive(Default)]
pub struct Objs {
    slots: Vec<Slot>,
    free: Vec<u32>,
}

fn slot_of(oid: Oid) -> usize {
    (oid & SLOT_MASK) as usize
}

/// Dense slot index, for per-object lists (`scene::WinScene::nodes`)
pub fn slot(oid: Oid) -> usize {
    slot_of(oid)
}

fn gen_of(oid: Oid) -> u32 {
    oid >> SLOT_BITS
}

impl Objs {
    /// Id for a new object, reusing a freed slot if any
    pub fn reserve(&mut self) -> Oid {
        let slot = match self.free.pop() {
            Some(s) => s,
            None => {
                self.slots.push(Slot { gen: 0, obj: None });
                (self.slots.len() - 1) as u32
            }
        };
        slot | (self.slots[slot as usize].gen << SLOT_BITS)
    }

    pub fn put(&mut self, oid: Oid, obj: Obj) {
        self.slots[slot_of(oid)].obj = Some(obj);
    }

    /// The object, if `oid` is still it
    pub fn get(&self, oid: Oid) -> Option<&Obj> {
        let s = self.slots.get(slot_of(oid))?;
        if s.gen != gen_of(oid) {
            return None;
        }
        s.obj.as_ref()
    }

    pub fn get_mut(&mut self, oid: Oid) -> Option<&mut Obj> {
        let s = self.slots.get_mut(slot_of(oid))?;
        if s.gen != gen_of(oid) {
            return None;
        }
        s.obj.as_mut()
    }

    pub fn is_live(&self, oid: Oid) -> bool {
        self.get(oid).is_some()
    }

    /// Takes the object out and bumps the generation, invalidating its ids
    pub fn free(&mut self, oid: Oid) -> Option<Obj> {
        let slot = slot_of(oid);
        let s = self.slots.get_mut(slot)?;
        if s.gen != gen_of(oid) {
            return None;
        }
        let obj = s.obj.take();
        s.gen = (s.gen + 1) & GEN_MASK;
        self.free.push(slot as u32);
        obj
    }

    /// Every live object's id, in slot order
    pub fn ids(&self) -> Vec<Oid> {
        self.slots.iter().enumerate()
            .filter(|(_, s)| s.obj.is_some())
            .map(|(i, s)| i as u32 | (s.gen << SLOT_BITS))
            .collect()
    }

    /// How many objects are alive
    #[cfg(test)]
    pub fn live(&self) -> usize {
        self.slots.iter().filter(|s| s.obj.is_some()).count()
    }

    pub fn clear(&mut self) {
        self.slots.clear();
        self.free.clear();
    }
}

impl Index<Oid> for Objs {
    type Output = Obj;
    fn index(&self, oid: Oid) -> &Obj {
        self.slots[slot_of(oid)].obj.as_ref().expect("an object the table keeps is alive")
    }
}

impl IndexMut<Oid> for Objs {
    fn index_mut(&mut self, oid: Oid) -> &mut Obj {
        self.slots[slot_of(oid)].obj.as_mut().expect("an object the table keeps is alive")
    }
}
