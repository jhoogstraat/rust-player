//! Rust Player: a GPUI window over the source-neutral contract.
//!
//! `--fake` runs against the scripted fake runtime (no credentials, no audio
//! hardware); the default run boots the real Spotatui runtime under the
//! application's data root. The window renders immutable snapshots and sends
//! commands; it never sees Spotify, librespot, or Spotatui types.

mod logging;
mod text_input;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, Context, FontWeight, IntoElement, KeyBinding, ParentElement, Render, SharedString,
    Styled, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, relative, rgb,
};
use player_core::{Command, LoginState, Runtime, SearchState, Snapshot, fake::FakeRuntime};
use player_spotatui::ConnectOptions;
use text_input::{KeyOutcome, TextField};

const BG: u32 = 0x0c0c0e;
const PANEL: u32 = 0x18181b;
const BORDER: u32 = 0x29292d;
const TEXT: u32 = 0xf4f4f5;
const MUTED: u32 = 0x8b8b91;
const ACCENT: u32 = 0x4f46e5;

actions!(
    player,
    [
        FocusSearch,
        NextTrack,
        PreviousTrack,
        VolumeUp,
        VolumeDown,
        ToggleQueuePanel,
    ]
);

static RUNTIME: OnceLock<Arc<dyn Runtime>> = OnceLock::new();
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

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
    snapshot: Snapshot,
    /// Holds focus whenever no text field does, so fixed shortcuts reach
    /// `handle_key` through the root element.
    root_focus: gpui::FocusHandle,
    search: TextField,
    paste: TextField,
    queue_open: bool,
}

impl PlayerApp {
    fn send(&self, command: Command) {
        let runtime = RUNTIME.get().expect("runtime");
        if !runtime.command(command.clone()) {
            log::warn!("[ui] runtime rejected {command:?}");
        }
    }

    /// Route a keystroke: whichever text field has focus edits itself and may
    /// submit; otherwise fixed shortcuts fire.
    fn handle_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focused_field = if self.search.focus.is_focused(window) {
            Some(true)
        } else if self.paste.focus.is_focused(window) {
            Some(false)
        } else {
            None
        };

        if let Some(is_search) = focused_field {
            let field = if is_search {
                &mut self.search
            } else {
                &mut self.paste
            };
            match field.key(event) {
                KeyOutcome::Submitted => {
                    let value = field.value.trim().to_string();
                    field.clear();
                    if !value.is_empty() {
                        self.send(if is_search {
                            Command::Search(value)
                        } else {
                            Command::SubmitPastedLoginUrl(value)
                        });
                    }
                    cx.notify();
                }
                KeyOutcome::Edited => cx.notify(),
                KeyOutcome::Blur => window.focus(&self.root_focus, cx),
                KeyOutcome::Ignored => {}
            }
            return;
        }

        // Fixed application shortcuts.
        match event.keystroke.key.as_str() {
            "space" => self.send(if self.snapshot.is_playing() {
                Command::Pause
            } else {
                Command::Resume
            }),
            "n" => self.send(Command::Next),
            "p" => self.send(Command::Previous),
            "+" | "=" => self.send(volume_command(&self.snapshot, 10)),
            "-" => self.send(volume_command(&self.snapshot, -10)),
            "/" => window.focus(&self.search.focus, cx),
            _ => {}
        }
    }
}

/// Step the volume by `delta` percent from the last reported level (80 until
/// the runtime reports one), clamped to 0–100.
fn volume_command(snapshot: &Snapshot, delta: i16) -> Command {
    let current = snapshot
        .playback
        .as_ref()
        .and_then(|p| p.volume_percent)
        .unwrap_or(80);
    Command::SetVolume((i16::from(current) + delta).clamp(0, 100) as u8)
}

/// `m:ss` for a millisecond count.
fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

impl Render for PlayerApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = self.snapshot.clone();

        div()
            .id("root")
            .key_context("Player")
            .track_focus(&self.root_focus)
            .on_key_down(cx.listener(Self::handle_key))
            .on_action(cx.listener(|app, _: &FocusSearch, window, cx| {
                window.focus(&app.search.focus, cx);
            }))
            .on_action(cx.listener(|app, _: &NextTrack, _, _| app.send(Command::Next)))
            .on_action(cx.listener(|app, _: &PreviousTrack, _, _| app.send(Command::Previous)))
            .on_action(cx.listener(|app, _: &VolumeUp, _, _| {
                app.send(volume_command(&app.snapshot, 10));
            }))
            .on_action(cx.listener(|app, _: &VolumeDown, _, _| {
                app.send(volume_command(&app.snapshot, -10));
            }))
            .on_action(cx.listener(|app, _: &ToggleQueuePanel, _, cx| {
                app.queue_open = !app.queue_open;
                cx.notify();
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            // Header
            .child(
                div()
                    .h(px(44.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(18.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Rust Player"),
                    )
                    .child(div().flex().gap(px(8.)).child(button(
                        "toggle-queue",
                        if self.queue_open {
                            "Hide Queue"
                        } else {
                            "Queue"
                        },
                        cx.listener(|app, _, _, cx| {
                            app.queue_open = !app.queue_open;
                            cx.notify();
                        }),
                    ))),
            )
            // Body: sign-in replaces the content area until the runtime is ready.
            .child(if ready(&snap) {
                self.render_content(window, cx).into_any_element()
            } else {
                self.render_sign_in(window, cx).into_any_element()
            })
            // Notice bar
            .children(snap.notice.as_ref().map(|notice| {
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
                    .text_color(rgb(MUTED))
                    .child(notice.message.clone())
                    .when(notice.dismissible, |el| {
                        el.child(
                            div()
                                .id("dismiss-notice")
                                .cursor_pointer()
                                .text_size(px(12.))
                                .hover(|style| style.text_color(rgb(TEXT)))
                                .on_click(
                                    cx.listener(|app, _, _, _| app.send(Command::DismissNotice)),
                                )
                                .child("Dismiss"),
                        )
                    })
            }))
            // Now-playing bar
            .child(self.render_now_playing(&snap))
            // Transport row
            .child(
                div()
                    .h(px(52.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(18.))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(button(
                        "toggle",
                        if snap.is_playing() { "Pause" } else { "Play" },
                        cx.listener(|app, _, _, _| {
                            app.send(if app.snapshot.is_playing() {
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
                        cx.listener(|app, _, _, _| app.send(volume_command(&app.snapshot, -10))),
                    ))
                    .child(button(
                        "vol-up",
                        "Vol +",
                        cx.listener(|app, _, _, _| app.send(volume_command(&app.snapshot, 10))),
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(log_path_label()),
                    ),
            )
    }
}

fn ready(snap: &Snapshot) -> bool {
    matches!(snap.login, LoginState::Ready)
}

fn log_path_label() -> String {
    match LOG_PATH.get() {
        Some(path) => format!("Logs: {}", path.display()),
        None => String::new(),
    }
}

impl PlayerApp {
    fn render_sign_in(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let message = match &self.snapshot.login {
            LoginState::InProgress { message, .. } => message.clone(),
            LoginState::Expired { message } => message.clone(),
            LoginState::Ready => String::new(),
        };
        let wants_paste = matches!(
            &self.snapshot.login,
            LoginState::InProgress {
                wants_pasted_url: true,
                ..
            }
        );
        let expired = matches!(&self.snapshot.login, LoginState::Expired { .. });

        div().flex_1().flex().items_center().justify_center().child(
            div()
                .w(px(520.))
                .p(px(28.))
                .rounded(px(14.))
                .bg(rgb(PANEL))
                .border_1()
                .border_color(rgb(BORDER))
                .flex()
                .flex_col()
                .gap(px(16.))
                .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(
                    match self.snapshot.login {
                        LoginState::Expired { .. } => "SIGN-IN · SESSION EXPIRED".to_string(),
                        _ => "SIGN-IN".to_string(),
                    },
                ))
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(if expired {
                            "Spotify session expired"
                        } else {
                            "Sign in to Spotify"
                        }),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(MUTED))
                        .whitespace_normal()
                        .child(message),
                )
                .when(wants_paste, |card| {
                    card.child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child("Paste the redirect URL below and press Enter:"),
                    )
                    .child(input_shell(self.paste.render("paste-url", window)))
                    .child(
                        div()
                            .id("paste-hint")
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(
                                "The browser finished but could not reach this app automatically.",
                            ),
                    )
                })
                .when(expired, |card| {
                    card.child(
                        div()
                            .id("reauthenticate")
                            .h(px(38.))
                            .px(px(16.))
                            .rounded(px(8.))
                            .bg(rgb(ACCENT))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(13.))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0x6366f1)))
                            .on_click(cx.listener(|app, _, _, _| app.send(Command::Reauthenticate)))
                            .child("Reauthenticate"),
                    )
                }),
        )
    }

    fn render_content(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let snap = &self.snapshot;

        // Search results column.
        let results = match &snap.search {
            SearchState::Done { results, .. } if !results.is_empty() => {
                let rows: Vec<_> = results
                    .iter()
                    .enumerate()
                    .map(|(i, playable)| {
                        let play = playable.clone();
                        let enqueue = playable.clone();
                        div()
                            .id(SharedString::from(format!("result-{i}")))
                            .px(px(18.))
                            .py(px(8.))
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .flex()
                            .items_center()
                            .justify_between()
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(PANEL)))
                            .on_click(cx.listener(move |app, _, _, _| {
                                app.send(Command::Play(play.clone()));
                            }))
                            .child(div().text_size(px(13.)).child(format!(
                                "{} — {}",
                                playable.title,
                                playable.artists_display()
                            )))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(MUTED))
                                            .child(playable.album.clone()),
                                    )
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("enqueue-{i}")))
                                            .px(px(8.))
                                            .py(px(3.))
                                            .rounded(px(5.))
                                            .border_1()
                                            .border_color(rgb(BORDER))
                                            .text_size(px(11.))
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(ACCENT)))
                                            .on_click(cx.listener(move |app, _, _, cx| {
                                                app.send(Command::Enqueue(enqueue.clone()));
                                                cx.stop_propagation();
                                            }))
                                            .child("+ Queue"),
                                    ),
                            )
                    })
                    .collect();
                div()
                    .max_h(px(240.))
                    .id("results")
                    .overflow_y_scroll()
                    .children(rows)
                    .into_any_element()
            }
            SearchState::Loading { query } => {
                status_row(format!("Searching “{query}”…")).into_any_element()
            }
            SearchState::Failed { query, message } => {
                status_row_with_retry(format!("Search failed: {message}"), query.clone(), cx)
                    .into_any_element()
            }
            SearchState::Done { results, .. } if results.is_empty() => {
                status_row("No results.".to_string()).into_any_element()
            }
            _ => div().into_any_element(),
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            // Search box
            .child(
                div()
                    .px(px(18.))
                    .pt(px(14.))
                    .pb(px(10.))
                    .child(input_shell(self.search.render("search-input", window))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .overflow_hidden()
                    // Results column
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(results),
                    )
                    .when(self.queue_open, |body| {
                        body.child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .flex()
                                .flex_col()
                                .border_l_1()
                                .border_color(rgb(BORDER))
                                .child(
                                    div()
                                        .px(px(14.))
                                        .py(px(10.))
                                        .text_size(px(11.))
                                        .text_color(rgb(MUTED))
                                        .child(format!("UP NEXT ({})", snap.queue.len())),
                                )
                                .when(snap.queue.is_empty(), |panel| {
                                    panel.child(
                                        div()
                                            .px(px(14.))
                                            .text_size(px(12.))
                                            .text_color(rgb(MUTED))
                                            .child("Empty — add something from the results."),
                                    )
                                })
                                .children(snap.queue.iter().enumerate().map(|(i, playable)| {
                                    div()
                                        .px(px(14.))
                                        .py(px(7.))
                                        .border_t_1()
                                        .border_color(rgb(BORDER))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .text_size(px(12.))
                                        .child(div().flex_1().overflow_hidden().child(format!(
                                            "{}. {} — {}",
                                            i + 1,
                                            playable.title,
                                            playable.artists_display()
                                        )))
                                        .child(
                                            div()
                                                .flex()
                                                .gap(px(6.))
                                                .child(small_button(
                                                    SharedString::from(format!("q-up-{i}")),
                                                    "↑",
                                                    cx.listener(move |app, _, _, _| {
                                                        app.send(Command::MoveQueued {
                                                            index: i,
                                                            up: true,
                                                        });
                                                    }),
                                                ))
                                                .child(small_button(
                                                    SharedString::from(format!("q-down-{i}")),
                                                    "↓",
                                                    cx.listener(move |app, _, _, _| {
                                                        app.send(Command::MoveQueued {
                                                            index: i,
                                                            up: false,
                                                        });
                                                    }),
                                                ))
                                                .child(small_button(
                                                    SharedString::from(format!("q-remove-{i}")),
                                                    "✕",
                                                    cx.listener(move |app, _, _, _| {
                                                        app.send(Command::RemoveQueued(i));
                                                    }),
                                                )),
                                        )
                                })),
                        )
                    }),
            )
    }

    fn render_now_playing(&self, snap: &Snapshot) -> impl IntoElement {
        let now = Instant::now();
        let (title_line, position_ms, duration_ms, progress) = match &snap.playback {
            Some(p) => {
                let visible = snap.projected_position_ms(now).unwrap_or(p.position_ms);
                let pct = if p.playable.duration_ms > 0 {
                    (visible as f32 / p.playable.duration_ms as f32).clamp(0., 1.)
                } else {
                    0.
                };
                (
                    format!(
                        "{} — {}{}",
                        p.playable.title,
                        p.playable.artists_display(),
                        if p.is_playing { "" } else { " ⏸" }
                    ),
                    visible,
                    p.playable.duration_ms,
                    pct,
                )
            }
            None => ("Nothing playing".to_string(), 0, 0, 0.),
        };

        div()
            .border_t_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(3.))
                    .bg(rgb(0x232328))
                    .child(div().h_full().w(relative(progress)).bg(rgb(ACCENT))),
            )
            .child(
                div()
                    .h(px(40.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(18.))
                    .child(div().text_size(px(12.)).overflow_hidden().child(title_line))
                    .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(
                        if duration_ms > 0 {
                            format!("{} / {}", clock(position_ms), clock(duration_ms))
                        } else {
                            String::new()
                        },
                    )),
            )
            .cursor_pointer()
    }
}

fn status_row(text: String) -> impl IntoElement {
    div()
        .px(px(18.))
        .py(px(10.))
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .child(text)
}

fn status_row_with_retry(text: String, query: String, cx: &Context<PlayerApp>) -> impl IntoElement {
    div()
        .px(px(18.))
        .py(px(10.))
        .flex()
        .items_center()
        .gap(px(10.))
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .child(text)
        .child(
            div()
                .id("retry-search")
                .px(px(8.))
                .py(px(3.))
                .rounded(px(5.))
                .border_1()
                .border_color(rgb(BORDER))
                .text_size(px(11.))
                .text_color(rgb(TEXT))
                .cursor_pointer()
                .hover(|style| style.bg(rgb(PANEL)))
                .on_click(cx.listener(move |app, _, _, _| {
                    app.send(Command::Search(query.clone()));
                }))
                .child("Retry"),
        )
}

fn input_shell(child: impl IntoElement) -> impl IntoElement {
    div().pb(px(10.)).max_w(px(480.)).child(child)
}

fn button(
    id: &'static str,
    label: impl Into<SharedString>,
    handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .h(px(30.))
        .px(px(12.))
        .rounded(px(7.))
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(PANEL))
        .border_1()
        .border_color(rgb(BORDER))
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x232328)))
        .on_click(handler)
        .child(label.into())
}

fn small_button(
    id: SharedString,
    label: &'static str,
    handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px(px(6.))
        .py(px(2.))
        .rounded(px(4.))
        .border_1()
        .border_color(rgb(BORDER))
        .text_size(px(11.))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(PANEL)))
        .on_click(handler)
        .child(label)
}

fn open_player_window(cx: &mut App) {
    let runtime = RUNTIME
        .get()
        .expect("runtime initialized before window opens")
        .clone();
    let initial_snapshot = runtime.subscribe().borrow().clone();

    let bounds = Bounds::centered(None, gpui::size(px(960.), px(640.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(gpui::size(px(620.), px(440.))),
            app_id: Some("rust-player".into()),
            ..Default::default()
        },
        |window, cx| {
            let mut rx = runtime.subscribe();
            let app = cx.new(|cx| {
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

                // Project playback position between snapshots at the tick rate so a
                // 250 ms tick is smooth on screen.
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
                    snapshot: initial_snapshot,
                    root_focus: cx.focus_handle(),
                    search: TextField::new(cx, "Search Spotify…"),
                    paste: TextField::new(cx, "http://127.0.0.1:8989/login?code=…"),
                    queue_open: true,
                }
            });
            // Shortcuts work from the first frame, before any click.
            let root_focus = app.read(cx).root_focus.clone();
            window.focus(&root_focus, cx);
            app
        },
    )
    .expect("failed to open player window");
}

fn main() {
    let fake = std::env::args().any(|arg| arg == "--fake");
    let data_root = data_root();

    // The application owns diagnostics; the fork's logger stays off.
    if let Ok(path) = logging::init(&data_root) {
        let _ = LOG_PATH.set(path);
    }

    let runtime: Arc<dyn Runtime> = if fake {
        Arc::new(FakeRuntime::new())
    } else {
        player_spotatui::connect(ConnectOptions::new(data_root.clone()))
    };
    let _ = RUNTIME.set(runtime);

    let application = gpui_platform::application();
    application.on_reopen(open_player_window);
    application.run(move |cx: &mut App| {
        // Fixed shortcuts: cmd-based only. Bindings dispatch before key-down
        // listeners, so a bare key here (e.g. "space") would fire even while
        // a text field is focused and never reach the field; bare-key
        // shortcuts live in `handle_key`, which checks focus first.
        cx.bind_keys([
            KeyBinding::new("cmd-f", FocusSearch, None),
            KeyBinding::new("cmd-right", NextTrack, None),
            KeyBinding::new("cmd-left", PreviousTrack, None),
            KeyBinding::new("cmd-up", VolumeUp, None),
            KeyBinding::new("cmd-down", VolumeDown, None),
            KeyBinding::new("cmd-k", ToggleQueuePanel, None),
        ]);

        // ⌘Q stops playback and flushes state cleanly. A `Subscription`
        // unregisters its hook on drop, and this closure returns before the
        // event loop starts, so the hook must be detached to survive.
        cx.on_app_quit(move |_cx| async move {
            if let Some(runtime) = RUNTIME.get() {
                runtime.shutdown();
            }
        })
        .detach();

        open_player_window(cx);
        cx.activate(true);
    });
}
