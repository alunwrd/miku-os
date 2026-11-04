// One interface to a file
//
// Each has the same shape: `fn on_<interface>` taking the server, the client's
// index, the object the request was sent to, an opcode and a reader, and
// returning `Result<(), &'static str>`. The string is the reason the connection
// is about to be killed (see `dispatch`), so it reads as a sentence about what
// the client did wrong.
//
// Split by interface and not by subject, because a request arrives named by the
// object it was sent to. Where a request needs a decision rather than a reply,
// it calls `shell`:
//
//      display     may_display, may_registry, may_output: the handshake,
//                  and the global filter that decides what a client is told
//      compositor  may_compositor, may_surface, may_region: pixels arriving
//      buffers     may_shm, may_shm_pool, may_buffer: where pixels live
//      seat        may_seat and the input objects, the keymap, the cursor
//      xdg         may_xdg, may_xdg_surface, may_toplevel: the window role
//      desktop     may_desktop: what is on the screen, and what a user asked
//      wm          may_wm: the desktop's policy, held by one client
//      console     may_console: the kernel's text, drawn by somebody else
//      clipboard   may_clipboard: what one program hands to another
//      layer       may_layer: a surface that is not a window
//      outputs     may_output_manager, may_output_configuration: the outputs

pub mod buffers;
pub mod clipboard;
pub mod compositor;
pub mod console;
pub mod desktop;
pub mod display;
pub mod layer;
pub mod outputs;
pub mod seat;
pub mod wm;
pub mod xdg;
