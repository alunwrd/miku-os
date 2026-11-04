//! A gpui window on Mayland, with the input a program needs: a button that
//! counts clicks, a line that shows what is typed, and the wheel moving a
//! number. Every primitive kind the renderer has is on the screen too -
//! rounded and bordered quads, a dashed border, a shadow, a gradient, an
//! underline, and text in two faces - so one screendump checks all of them.
//!
//! `cargo build -p gpui_linux --example mayland_hello --features mayland`

#[cfg(target_os = "miku")]
fn main() {
    app::run();
}

#[cfg(not(target_os = "miku"))]
fn main() {}

#[cfg(target_os = "miku")]
mod app {
    use std::borrow::Cow;

    use gpui::{
        App, Bounds, Context, FocusHandle, KeyDownEvent, ScrollWheelEvent, SharedString, Window,
        WindowBounds, WindowOptions, div, linear_color_stop, linear_gradient, prelude::*, px, rgb,
        size,
    };

    struct Hello {
        focus: FocusHandle,
        clicks: usize,
        typed: String,
        last_key: SharedString,
        scrolled: f32,
    }

    impl Hello {
        fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
            let keystroke = &event.keystroke;
            self.last_key = format!("{keystroke}").into();
            match keystroke.key.as_str() {
                "backspace" => {
                    self.typed.pop();
                }
                "enter" => self.typed.clear(),
                _ => {
                    if !keystroke.modifiers.control
                        && let Some(text) = &keystroke.key_char
                    {
                        self.typed.push_str(text);
                    }
                }
            }
            cx.notify();
        }

        fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
            self.scrolled += event.delta.pixel_delta(px(20.)).y.as_f32();
            cx.notify();
        }
    }

    impl Render for Hello {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let swatch = |color: u32| {
                div()
                    .size_8()
                    .bg(rgb(color))
                    .border_1()
                    .border_dashed()
                    .rounded_md()
                    .border_color(gpui::white())
            };
            div()
                .track_focus(&self.focus)
                .on_key_down(cx.listener(Self::key))
                .on_scroll_wheel(cx.listener(Self::scroll))
                .size_full()
                .flex()
                .flex_col()
                .gap_4()
                .p_8()
                .bg(rgb(0x1e1f22))
                .text_color(rgb(0xdfe1e5))
                .font_family("IBM Plex Sans")
                .child(div().text_2xl().child("Zed's gpui, on MikuOS"))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(swatch(0xe06c75))
                        .child(swatch(0x98c379))
                        .child(swatch(0x61afef))
                        .child(swatch(0xe5c07b)),
                )
                .child(
                    div()
                        .id("button")
                        .w(px(220.))
                        .p_3()
                        .rounded_lg()
                        .shadow_lg()
                        .bg(linear_gradient(
                            90.,
                            linear_color_stop(rgb(0x3d7eff), 0.),
                            linear_color_stop(rgb(0x8a5cff), 1.),
                        ))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.clicks += 1;
                            cx.notify();
                        }))
                        .child(format!("clicked {} times", self.clicks)),
                )
                .child(
                    div()
                        .p_3()
                        .border_1()
                        .border_color(rgb(0x4e5157))
                        .rounded_md()
                        .font_family("Lilex")
                        .child(format!("typed: {}▏", self.typed)),
                )
                .child(
                    div()
                        .underline()
                        .child(format!("last key: {}", self.last_key)),
                )
                .child(format!("wheel: {:.0}", self.scrolled))
        }
    }

    fn fonts() -> Vec<Cow<'static, [u8]>> {
        vec![
            Cow::Borrowed(include_bytes!(
                "../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf"
            )),
            Cow::Borrowed(include_bytes!(
                "../../../assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBold.ttf"
            )),
            Cow::Borrowed(include_bytes!("../../../assets/fonts/lilex/Lilex-Regular.ttf")),
        ]
    }

    pub fn run() {
        gpui::Application::with_platform(gpui_linux::current_platform(false)).run(|cx: &mut App| {
            if let Err(error) = cx.text_system().add_fonts(fonts()) {
                eprintln!("mayland_hello: fonts: {error:#}");
                return;
            }
            let bounds = Bounds::centered(None, size(px(800.), px(600.)), cx);
            let window = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        let focus = cx.focus_handle();
                        window.focus(&focus, cx);
                        Hello {
                            focus,
                            clicks: 0,
                            typed: String::new(),
                            last_key: "none yet".into(),
                            scrolled: 0.,
                        }
                    })
                },
            );
            if let Err(error) = window {
                eprintln!("mayland_hello: no window: {error:#}");
                cx.quit();
            }
            cx.activate(true);
        });
    }
}
