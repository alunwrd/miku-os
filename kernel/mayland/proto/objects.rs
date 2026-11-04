// Per-connection object map
//
// Every id in the protocol is scoped to one connection, so this table is not
// global: two clients each own an object 1, and neither can name the other's.
// That is the protocol's security model, and why nothing here takes a client
// argument.
//
// The id space is split as Wayland splits it: ids below SERVER_ID_START are
// chosen by the client and arrive in new_id arguments, ids at or above it by
// the server. A client sending an id from the server's half is malformed and
// gets an error

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use super::protocol::Interface;

/// First id reserved for server-created objects. Wayland's WL_SERVER_ID_START
pub const SERVER_ID_START: u32 = 0xff00_0000;

/// A cap on live objects per connection. Wayland allows 0x00f00000, a limit on
/// the id space and not memory; ours is a real table, and a client with a
/// thousand live objects has a bug
pub const MAX_OBJECTS: usize = 1024;

#[derive(Clone, Copy, Debug)]
pub struct Object {
    pub id: u32,
    pub iface: Interface,
    pub version: u32,
    /// Interface-specific back-reference: a surface index, a pool index, and so
    /// on. Zero means none, so every user stores index + 1
    pub data: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObjectError {
    /// The id is outside the half of the space the sender may allocate from
    BadRange,
    /// Something is already live under that id
    InUse,
    /// MAX_OBJECTS reached
    Full,
    /// No object under that id
    NotFound,
}

pub struct ObjectMap {
    objects: Vec<Object>,
}

impl ObjectMap {
    pub fn new() -> Self {
        ObjectMap { objects: Vec::new() }
    }

    #[inline]
    pub fn len(&self) -> usize { self.objects.len() }

    pub fn get(&self, id: u32) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    #[inline]
    pub fn contains(&self, id: u32) -> bool {
        self.get(id).is_some()
    }

    /// Interface of a live object, for the dispatch switch
    pub fn iface_of(&self, id: u32) -> Option<Interface> {
        self.get(id).map(|o| o.iface)
    }

    /// Bind an id the client named in a new_id argument
    pub fn insert_client(&mut self, id: u32, iface: Interface, version: u32)
        -> Result<(), ObjectError>
    {
        if id == 0 || id >= SERVER_ID_START {
            return Err(ObjectError::BadRange);
        }
        self.insert(id, iface, version)
    }

    // There is no `insert_server`: every id in the protocol arrives in a
    // `new_id` argument

    fn insert(&mut self, id: u32, iface: Interface, version: u32)
        -> Result<(), ObjectError>
    {
        if self.contains(id) {
            return Err(ObjectError::InUse);
        }
        if self.objects.len() >= MAX_OBJECTS {
            return Err(ObjectError::Full);
        }
        self.objects.push(Object { id, iface, version, data: 0 });
        Ok(())
    }

    pub fn set_data(&mut self, id: u32, data: u32) -> Result<(), ObjectError> {
        match self.get_mut(id) {
            Some(o) => { o.data = data; Ok(()) }
            None => Err(ObjectError::NotFound),
        }
    }

    pub fn data_of(&self, id: u32) -> Option<u32> {
        self.get(id).map(|o| o.data)
    }

    pub fn remove(&mut self, id: u32) -> Result<Object, ObjectError> {
        match self.objects.iter().position(|o| o.id == id) {
            Some(i) => Ok(self.objects.swap_remove(i)),
            None => Err(ObjectError::NotFound),
        }
    }

    /// Every live object of one interface, to fan an event out to all of a
    /// client's keyboards, say
    pub fn ids_of(&self, iface: Interface) -> Vec<u32> {
        self.objects.iter().filter(|o| o.iface == iface).map(|o| o.id).collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Object> {
        self.objects.iter()
    }
}
