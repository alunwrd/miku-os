// Network stack, L2 to L4
//
// Parses bytes chosen by whoever is on the other end of the cable, so it runs
// out of ring 0 and `cargo test` covers it. Frames arrive and leave as slices:
// netd moves them through /dev/net0, a test hands them to itself.

pub mod arp;
pub mod conn;
pub mod dhcp;
pub mod eth;
pub mod tcp;
pub mod udp;
