// mikuD socket activation - inetd-style service on-demand start
//
// Associates a port with a service. When a connection arrives on the port,
// mikuD starts the associated service. The socket is passed to the service.

extern crate alloc;
use alloc::vec::Vec;
use spin::Mutex;

use super::journal;

pub const MAX_SOCKETS: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SocketType {
    Stream, // TCP-like
    Dgram,  // UDP-like
}

impl SocketType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stream => "stream",
            Self::Dgram => "dgram",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "stream" | "tcp" => Some(Self::Stream),
            "dgram" | "udp" => Some(Self::Dgram),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub struct SocketUnit {
    pub name: &'static str,
    pub service: &'static str,    // service to activate
    pub port: u16,
    pub socket_type: SocketType,
    pub active: bool,
    pub connections: u32,          // total activations
    pub backlog: u16,              // max pending connections
    pub accept: bool,              // accept before passing to service
}

impl SocketUnit {
    pub const fn empty() -> Self {
        Self {
            name: "",
            service: "",
            port: 0,
            socket_type: SocketType::Stream,
            active: false,
            connections: 0,
            backlog: 5,
            accept: true,
        }
    }
}

pub struct SocketTable {
    pub sockets: [SocketUnit; MAX_SOCKETS],
    pub count: usize,
}

impl SocketTable {
    pub const fn new() -> Self {
        Self {
            sockets: [SocketUnit::empty(); MAX_SOCKETS],
            count: 0,
        }
    }

    pub fn add(&mut self, sock: SocketUnit) -> bool {
        if self.count >= MAX_SOCKETS {
            return false;
        }
        // Check for port conflict
        for s in self.sockets.iter() {
            if s.active && s.port == sock.port {
                return false;
            }
        }
        for slot in self.sockets.iter_mut() {
            if !slot.active {
                *slot = sock;
                slot.active = true;
                self.count += 1;
                return true;
            }
        }
        false
    }

    pub fn find_by_name(&self, name: &str) -> Option<usize> {
        self.sockets.iter().position(|s| s.active && s.name == name)
    }

    pub fn find_by_port(&self, port: u16) -> Option<usize> {
        self.sockets.iter().position(|s| s.active && s.port == port)
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let idx = match self.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        self.sockets[idx] = SocketUnit::empty();
        self.count = self.count.saturating_sub(1);
        true
    }
}

static SOCKETS: Mutex<SocketTable> = Mutex::new(SocketTable::new());

// -- public API --

pub fn register_socket(
    name: &'static str,
    service: &'static str,
    port: u16,
    socket_type: SocketType,
) -> bool {
    let mut sock = SocketUnit::empty();
    sock.name = name;
    sock.service = service;
    sock.port = port;
    sock.socket_type = socket_type;

    let ok = SOCKETS.lock().add(sock);
    if ok {
        crate::serial_println!("[mikud] socket '{}' port={} -> '{}'",
            name, port, service);
    }
    ok
}

pub fn activate_socket(port: u16) -> bool {
    let mut table = SOCKETS.lock();
    let idx = match table.find_by_port(port) {
        Some(i) => i,
        None => return false,
    };

    let service = table.sockets[idx].service;
    let name = table.sockets[idx].name;
    table.sockets[idx].connections = table.sockets[idx].connections.saturating_add(1);
    drop(table);

    crate::serial_println!("[mikud] socket activation on port {} -> '{}'", port, service);
    journal::log(journal::Event::SocketActivated, name, port as u64, 0);
    super::api::start_service(service)
}

pub fn stop_socket(name: &str) -> bool {
    let mut table = SOCKETS.lock();
    let idx = match table.find_by_name(name) {
        Some(i) => i,
        None => return false,
    };
    table.sockets[idx].active = false;
    true
}

pub fn remove_socket(name: &str) -> bool {
    SOCKETS.lock().remove(name)
}

pub fn list_sockets() -> Vec<SocketSnapshot> {
    let table = SOCKETS.lock();
    let mut out = Vec::new();
    for s in table.sockets.iter() {
        if !s.active { continue; }
        out.push(SocketSnapshot {
            name: s.name,
            service: s.service,
            port: s.port,
            socket_type: s.socket_type.as_str(),
            connections: s.connections,
            active: s.active,
        });
    }
    out
}

pub fn socket_for_port(port: u16) -> Option<&'static str> {
    let table = SOCKETS.lock();
    table.find_by_port(port).map(|idx| table.sockets[idx].service)
}

pub fn socket_count() -> usize {
    SOCKETS.lock().count
}

pub struct SocketSnapshot {
    pub name: &'static str,
    pub service: &'static str,
    pub port: u16,
    pub socket_type: &'static str,
    pub connections: u32,
    pub active: bool,
}
