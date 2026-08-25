use gpui::{
    App, AppContext as _, Bounds, Context, IntoElement, Render, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, rgb, size,
};

struct Player {
    playing: bool,
}

impl Render for Player {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = rgb(0x8b8b91);
        let text = rgb(0xf4f4f5);
        let panel = rgb(0x18181b);

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x0c0c0e))
            .text_color(text)
            .child(
                div()
                    .h(px(44.))
                    .flex()
                    .items_center()
                    .px(px(18.))
                    .border_b_1()
                    .border_color(rgb(0x29292d))
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Rust Player"),
            )
            .child(
                div().flex_1().flex().items_center().justify_center().child(
                    div()
                        .w(px(440.))
                        .p(px(24.))
                        .rounded(px(14.))
                        .bg(panel)
                        .border_1()
                        .border_color(rgb(0x2d2d32))
                        .flex()
                        .flex_col()
                        .gap(px(18.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(muted)
                                .child("SPOTIFY • FEASIBILITY SHELL"),
                        )
                        .child(
                            div()
                                .text_size(px(24.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("Nothing playing"),
                        )
                        .child(
                            div()
                                .id("play-pause")
                                .h(px(38.))
                                .px(px(16.))
                                .rounded(px(8.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(rgb(0x4f46e5))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x6366f1)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.playing = !this.playing;
                                    cx.notify();
                                }))
                                .child(if self.playing { "Pause" } else { "Play" }),
                        ),
                ),
            )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(820.), px(560.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(600.), px(420.))),
                app_id: Some("rust-player".into()),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Player { playing: false }),
        )
        .expect("failed to open player window");
        cx.activate(true);
    });
}
