// drivers/gpu - graphics processors
pub mod nvidia;

/// EDID parsing. Same wire format regardless of GPU vendor, so it lives
/// here instead of duplicated per-driver.
pub mod edid;
/// Bochs DISPI display interface, used by QEMU/virtual machines
pub mod bochs;
/// Mode selection: picks a display mode from whatever the monitor advertises
pub mod display;
/// What a compositor needs of a display, as a contract. Both backends, the card
/// and the processor, implement it
pub mod scanout;
/// The processor's backend: no raster, no engine, no second surface
pub mod softscan;
/// The display layer: outputs, heads and a change to them as one request,
/// whoever drives the screen (`dev/kms-architecture.md`)
pub mod kms;
