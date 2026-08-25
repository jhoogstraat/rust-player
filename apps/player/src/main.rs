//! Rust Player: a GPUI window over the source-neutral contract.
//!
//! `--fake` runs against the scripted fake runtime (no credentials, no audio
//! hardware); the default run boots the real Spotatui runtime under the
//! application's data root.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, Context, FontWeight, IntoElement, ParentElement, Render, SharedString, Styled,
    Window, WindowBounds, WindowOptions, div, prelude::*, px, rgb,
};
use player_core::{
    AudioState, Command, LoginState, Runtime, SearchState, Snapshot, fake::FakeRuntime,
};
use player_spotatui::ConnectOptions;

const BG: u32 = 0x0c0c0e;
const PANEL: u32 = 0x18181b;
const BORDER: u32 = 0x29292d;
const TEXT: u32 = 0xf4f4f5;
const MUTED: u32 = 0x8b8b91;
const ACCENT: u32 = 0x4f46e5;

fn data_root() -> PathBuf {
    std::env::var_os("RUST_PLAYER_DATA_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").expect("HOME is set");
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("rust-player")
        })
}

struct PlayerApp {
    runtime: Arc<dyn Runtime>,
    snapshot: Snapshot,
}

impl PlayerApp {
    fn send(&self, command: Command) {
        self.runtime.command(command);
    }
}

impl Render for PlayerApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = self.snapshot.clone();
        let now = Instant::now();

        let muted = rgb(MUTED);
        let text_color = rgb(TEXT);

        // --- derived view lines -------------------------------------------------
        let login_line: SharedString = match &snap.login {
            LoginState::InProgress {
                message,
                wants_pasted_url,
            } => format!(
                "sign-in: in progress — {message}{}",
                if *wants_pasted_url {
                    " [paste URL field requested]"
                } else {
                    ""
                }
            )
            .into(),
            LoginState::Ready => "sign-in: ready".into(),
            LoginState::Expired { message } => format!("sign-in: EXPIRED — {message}").into(),
        };

        let search_line: SharedString = match &snap.search {
            SearchState::Idle => "search: idle".into(),
            SearchState::Loading { query } => format!("search: loading “{query}”…").into(),
            SearchState::Done { query, results } => {
                format!("search: {} result(s) for “{query}”", results.len()).into()
            }
            SearchState::Failed { query, message } => {
                format!("search: FAILED for “{query}” — {message}").into()
            }
        };

        let position_line: SharedString = snap
            .projected_position_ms(now)
            .map(|ms| format!("{:.1}s", ms as f64 / 1000.0))
            .unwrap_or_else(|| "—".to_string())
            .into();

        let playback_line: SharedString = match &snap.playback {
            None => "now playing: nothing".into(),
            Some(p) => format!(
                "now playing: {}{} [{} · {}]",
                format!("{} — {}", p.playable.title, p.playable.artists_display()),
                if p.is_playing { " ▶" } else { " ⏸" },
                position_line,
                p.volume_percent
                    .map(|v| format!("{v}%"))
                    .unwrap_or_default()
            )
            .into(),
        };

        let queue_lines: Vec<SharedString> = snap
            .queue
            .iter()
            .enumerate()
            .map(|(i, p)| format!("  {}. {} — {}", i + 1, p.title, p.artists_display()).into())
            .collect();

        let audio_line: SharedString = match &snap.audio {
            AudioState::Ready => "audio: ready".into(),
            AudioState::Starting => "audio: starting…".into(),
            AudioState::Unavailable { message } => format!("audio: UNAVAILABLE — {message}").into(),
        };

        let notice_line: Option<SharedString> = snap.notice.as_ref().map(|n| {
            format!(
                "notice: {}{}",
                n.message,
                if n.dismissible { " ✕" } else { "" }
            )
            .into()
        });

        let is_playing = snap.is_playing();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(text_color)
            // Header
            .child(
                div()
                    .h(px(44.))
                    .flex()
                    .items_center()
                    .px(px(18.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Rust Player"),
            )
            // Body
            .child(
                div()
                    .flex_1()
                    .flex()
                    .gap(px(12.))
                    .p(px(18.))
                    .min_h_0()
                    // Snapshot panel
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(div().text_size(px(11.)).text_color(muted).child("SNAPSHOT"))
                            .child(div().text_size(px(13.)).child(login_line))
                            .child(div().text_size(px(13.)).child(audio_line))
                            .child(div().text_size(px(13.)).child(search_line))
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(playback_line),
                            ),
                    )
                    // Queue panel
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(muted)
                                    .child(format!("QUEUE ({})", snap.queue.len())),
                            )
                            .when(snap.queue.is_empty(), |el| {
                                el.child(
                                    div().text_size(px(13.)).text_color(muted).child("(empty)"),
                                )
                            })
                            .children(queue_lines.iter().enumerate().map(|(i, line)| {
                                let index = i;
                                div()
                                    .id(SharedString::from(format!("queue-row-{index}")))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .text_size(px(13.))
                                    .cursor_pointer()
                                    .hover(|style| style.text_color(rgb(ACCENT)))
                                    .on_click(cx.listener(move |app, _, _, _| {
                                        app.send(Command::RemoveQueued(index));
                                    }))
                                    .child(line.clone())
                                    .child(div().text_size(px(10.)).text_color(muted).child("✕"))
                            }))
                            .when(!snap.queue.is_empty(), |el| {
                                el.child(button(
                                    "clear-queue",
                                    "Clear upcoming",
                                    cx.listener(|app, _, _, _| app.send(Command::ClearQueue)),
                                ))
                            }),
                    ),
            )
            // Notice bar
            .children(notice_line.map(|line| {
                div()
                    .px(px(18.))
                    .py(px(6.))
                    .bg(rgb(PANEL))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(12.))
                    .text_color(muted)
                    .child(line)
                    .child(
                        div()
                            .id("dismiss-notice")
                            .text_size(px(12.))
                            .cursor_pointer()
                            .on_click(cx.listener(|app, _, _, _| app.send(Command::DismissNotice)))
                            .child("Dismiss"),
                    )
            }))
            // Transport bar
            .child(
                div()
                    .h(px(56.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(18.))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(button(
                        "toggle",
                        if is_playing { "Pause" } else { "Play" },
                        cx.listener(move |app, _, _, _| {
                            app.send(if is_playing {
                                Command::Pause
                            } else {
                                Command::Resume
                            });
                        }),
                    ))
                    .child(button(
                        "prev",
                        "⏮",
                        cx.listener(|app, _, _, _| app.send(Command::Previous)),
                    ))
                    .child(button(
                        "next",
                        "⏭",
                        cx.listener(|app, _, _, _| app.send(Command::Next)),
                    ))
                    .child(button(
                        "vol-down",
                        "Vol −",
                        cx.listener(|app, _, _, _| {
                            let current = app
                                .snapshot
                                .playback
                                .as_ref()
                                .and_then(|p| p.volume_percent)
                                .unwrap_or(80);
                            app.send(Command::SetVolume(current.saturating_sub(10)));
                        }),
                    ))
                    .child(button(
                        "vol-up",
                        "Vol +",
                        cx.listener(|app, _, _, _| {
                            let current = app
                                .snapshot
                                .playback
                                .as_ref()
                                .and_then(|p| p.volume_percent)
                                .unwrap_or(80);
                            app.send(Command::SetVolume((current + 10).min(100)));
                        }),
                    ))
                    .child(button(
                        "demo-search",
                        "Search demo",
                        cx.listener(|app, _, _, _| {
                            app.send(Command::Search("blue".to_string()));
                        }),
                    )),
            )
            // Search results
            .children(match &snap.search {
                SearchState::Done { results, .. } if !results.is_empty() => {
                    let rows: Vec<_> = results
                        .iter()
                        .enumerate()
                        .map(|(i, playable)| {
                            let playable = playable.clone();
                            let label =
                                format!("{} — {}", playable.title, playable.artists_display());
                            let album = format!("▶ play · {}", playable.album);
                            div()
                                .id(SharedString::from(format!("result-{i}")))
                                .px(px(18.))
                                .py(px(8.))
                                .border_t_1()
                                .border_color(rgb(BORDER))
                                .flex()
                                .items_center()
                                .justify_between()
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(PANEL)))
                                .on_click(cx.listener(move |app, _, _, _| {
                                    app.send(Command::Play(playable.clone()));
                                }))
                                .child(div().text_size(px(13.)).child(label))
                                .child(div().text_size(px(11.)).text_color(muted).child(album))
                        })
                        .collect();
                    Some(
                        div()
                            .max_h(px(220.))
                            .id("results-scroll")
                            .overflow_y_scroll()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .children(rows),
                    )
                }
                _ => None,
            })
    }
}

fn button(
    id: &'static str,
    label: impl Into<SharedString>,
    handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .h(px(34.))
        .px(px(14.))
        .rounded(px(8.))
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(PANEL))
        .border_1()
        .border_color(rgb(BORDER))
        .text_size(px(13.))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x232328)))
        .on_click(handler)
        .child(label.into())
}

fn main() {
    let fake = std::env::args().any(|arg| arg == "--fake");

    gpui_platform::application().run(move |cx: &mut App| {
        let runtime: Arc<dyn Runtime> = if fake {
            Arc::new(FakeRuntime::new())
        } else {
            player_spotatui::connect(ConnectOptions::new(data_root()))
        };
        let initial_snapshot = runtime.subscribe().borrow().clone();

        let bounds = Bounds::centered(None, gpui::size(px(920.), px(640.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(gpui::size(px(600.), px(420.))),
                app_id: Some("rust-player".into()),
                ..Default::default()
            },
            |_, cx| {
                let mut rx = runtime.subscribe();
                cx.new(|cx| {
                    // Fold published snapshots into the entity.
                    cx.spawn(async move |this, cx| {
                        loop {
                            let snapshot = rx.borrow_and_update().clone();
                            if this
                                .update(cx, |app: &mut PlayerApp, cx| {
                                    app.snapshot = snapshot;
                                    cx.notify();
                                })
                                .is_err()
                            {
                                break;
                            }
                            if rx.changed().await.is_err() {
                                break;
                            }
                        }
                    })
                    .detach();

                    // Project playback position between snapshots at the tick rate.
                    cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor()
                                .timer(Duration::from_millis(250))
                                .await;
                            if this.update(cx, |_, cx| cx.notify()).is_err() {
                                break;
                            }
                        }
                    })
                    .detach();

                    PlayerApp {
                        runtime,
                        snapshot: initial_snapshot,
                    }
                })
            },
        )
        .expect("failed to open player window");
        cx.activate(true);
    });
}
