// The protocol: the bytes on the wire and the names they carry
//
// Nothing here knows what a window is. A message is a header and some
// arguments, an interface is a name and a table of opcodes, and an id is a
// number scoped to one connection. The one layer both halves of the system
// compile.
//
// Four of these files are in `libmay`, the crate both halves link, and are
// re-exported so the kernel names them `crate::mayland::wire` and the like. The
// kernel adds an id table that has to survive a hostile peer, and an outbound
// queue that needs `alloc`:
//
//      wire      message encoding, byte-for-byte Wayland's
//      protocol  the interface table: names, opcodes, argument orders
//      requests  every request a client can make, encoded once for both halves
//      keymap    the layout, which the compositor decides and a client reads
//      objects   per-connection id space              (kernel only)
//      outq      a growable queue of encoded events   (kernel only)
//      client    the protocol from the other end      (kernel only)
//
// `client` is here and not beside the compositor as it is not part of it:
// `shell`'s in-kernel clients (the console, the wallpaper, the demo) and the
// selftest talk to the server through it, in the shape of the userspace library
// `libmay::conn`. The two share their request encoders (`requests`) and differ
// in their transport and event decoder.

pub use libmay::wire;
pub use libmay::protocol;
// Every request a client makes: the in-kernel clients and the ring-3 ones
// encode the same bytes
pub use libmay::requests;
// The compositor decides which bits the modifier mask has and a client decides
// the letter, so the two halves are one file
pub use libmay::keymap;

pub mod objects;
pub mod outq;
pub mod client;
