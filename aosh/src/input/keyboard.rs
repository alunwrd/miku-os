// Compositor keyboard state: keymap, modifiers, and target window focus.
//
// Scancode-to-keysym resolution is driven by user layout, not hardware wiring.
// The compositor supplies the layout table (`KEYMAP_MAY_V2`, one group per
// layout),
// defaulting to the embedded US keymap until loaded. The active group arrives
// with
// modifiers, after which `keys.rs` performs key mapping

use libmay::conn::{Event, Pool};
use libmay::protocol;

use crate::mayland::Display;
use crate::object::engine::Engine;

#[derive(Default)]
pub struct Keyboard {
    keymap: Option<Pool>,
    mods: u32,
    group: u32,
}

impl Keyboard {
    /// Dispatches a keyboard event to the engine. Returns `false` if the event
    /// is not from a keyboard
    pub fn event(&mut self, engine: &Engine, display: &Display, e: &Event) -> bool {
        match *e {
            Event::Keymap {
                format,
                handle,
                size,
            } => {
                if format == protocol::keyboard::KEYMAP_MAY_V2 {
                    self.keymap = Pool::adopt(handle, size as usize);
                }
            }
            Event::Modifiers { mask, group } => {
                self.mods = mask;
                self.group = group;
            }
            Event::KeyboardEnter { surface } => {
                engine.keyboard(
                    display
                        .windows
                        .iter()
                        .find(|w| w.surface == surface)
                        .map(|w| w.oid),
                );
            }
            Event::KeyboardLeave { surface } => {
                let ours = display
                    .windows
                    .iter()
                    .find(|w| w.surface == surface)
                    .map(|w| w.oid);
                if ours.is_some() && engine.core.0.borrow().keys.window() == ours {
                    engine.keyboard(None);
                }
            }
            Event::Key {
                keycode, pressed, ..
            } => {
                if pressed {
                    let ch = match self.keymap.as_ref() {
                        Some(p) => libmay::keymap::Table::new(p.as_slice())
                            .and_then(|t| t.to_char_in(keycode as u16, self.mods, self.group)),
                        None => libmay::keymap::to_char(keycode as u16, self.mods),
                    };
                    engine.key(keycode, ch, self.mods);
                }
            }
            _ => return false,
        }
        true
    }
}
