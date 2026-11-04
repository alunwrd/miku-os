// The desktop
//
// Decisions rather than mechanisms. The server knows which surfaces exist and
// how to talk to them; this layer knows where they belong, which is being
// looked at, how they arrive and leave, and what is behind them. It is the half
// a Hyprland config is written against, and `may_wm` carries the config in.
//
// It owns almost no state. A layout is a function of the server's surfaces plus
// a handful of numbers, written as `fn(&mut Server)`, so there is one answer
// for each window's geometry.
//
//      layout     where every window goes: the work area, goals, the algorithms
//      workspace  which of them is being looked at, and the slide between two
//      anim       curves, and what each class of movement is animated with
//      ghost      a window's last frame, kept alive for the length of its exit
//      wallpaper  what is behind everything, as an ordinary client
//      demo       clients, in the kernel, for when there is nothing else running
//
// The kernel's text is shown by `/bin/mayterm`: the kernel keeps the grid and
// hands it out over `may_console`.

pub mod anim;
pub mod demo;
pub mod ghost;
pub mod layout;
pub mod popup;
pub mod wallpaper;
pub mod workspace;
