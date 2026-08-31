//! Rust Player: a GPUI window over the source-neutral contract.
//!
//! `--fake` runs against the scripted fake runtime (no credentials, no audio
//! hardware); the default run boots the real Spotatui runtime under the
//! application's data root. The window renders immutable snapshots and sends
//! commands; it never sees Spotify, librespot, or Spotatui types.

mod icons;
mod library;
mod logging;
mod sidebar;
mod text_input;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, FontWeight, Hsla, IntoElement, KeyBinding, ParentElement, Render,
    SharedString, Styled, Window, WindowBackgroundAppearance, WindowBounds, WindowOptions, actions,
    div, hsla, prelude::*, px, relative, rgb,
};
use player_core::{Command, LibraryState, LoginState, Runtime, SearchDetail, SearchState, SearchTarget, Snapshot, fake::FakeRuntime};
use player_spotatui::ConnectOptions;
use text_input::{KeyOutcome, TextField};

pub(crate) const BG: u32 = 0x0c0c0e;
pub(crate) const PANEL: u32 = 0x18181b;
pub(crate) const TEXT: u32 = 0xf4f4f5;
pub(crate) const MUTED: u32 = 0x8b8b91;
pub(crate) const ACCENT: u32 = 0x4f46e5;

/// macOS and Windows composite the window over an OS-blurred backdrop
/// (`WindowBackgroundAppearance::Blurred`): AppKit vibrancy / DWM composition,
/// both guaranteed by the platform. Linux has no compositor guarantee (Wayland
/// blurs only under KWin, X11 not at all), so chrome stays fully opaque there.
const GLASS: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// Surface tone from a 24-bit `0xRRGGBB` color, thinned to `alpha` where glass
/// is active; opaque platforms keep full coverage so their look is unchanged.
pub(crate) fn tone(color: u32, alpha: f32) -> Hsla {
    Hsla::from(rgb(color)).opacity(if GLASS { alpha } else { 1.0 })
}

/// Hairline that reads over an unpredictable blurred backdrop.
pub(crate) fn border() -> Hsla {
    hsla(0., 0., 1., 0.10)
}

/// Interactive wash over any surface (Comet's dark-mode wash idiom):
/// low-alpha white reads as a tinted plate on glass and as a lighter grey
/// on opaque chrome alike.
pub(crate) fn wash(alpha: f32) -> Hsla {
    hsla(0., 0., 1., alpha)
}

actions!(
    player,
    [
        FocusSearch,
        NextTrack,
        PreviousTrack,
        VolumeUp,
        VolumeDown,
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
    /// The active sidebar destination; library sections feed the second
    /// column, Settings swaps the main area.
    nav: sidebar::NavSection,
    /// Holds focus whenever no text field does, so fixed shortcuts reach
    /// `handle_key` through the root element.
    root_focus: gpui::FocusHandle,
    search: TextField,
    paste: TextField,
    search_history: Vec<Option<SearchTarget>>,
    search_history_index: usize,
}

impl PlayerApp {
    fn send(&self, command: Command) {
        let runtime = RUNTIME.get().expect("runtime");
        if !runtime.command(command.clone()) {
            log::warn!("[ui] runtime rejected {command:?}");
        }
    }

    fn open_search_target(&mut self, target: SearchTarget, cx: &mut Context<Self>) {
        self.search_history.truncate(self.search_history_index + 1);
        self.search_history.push(Some(target.clone()));
        self.search_history_index += 1;
        self.send(Command::OpenSearchTarget(target));
        cx.notify();
    }

    fn move_search_history(&mut self, index: usize, cx: &mut Context<Self>) {
        self.search_history_index = index;
        if let Some(target) = self.search_history[index].clone() {
            self.send(Command::OpenSearchTarget(target));
        }
        cx.notify();
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
                        let command = if is_search {
                            self.search_history = vec![None];
                            self.search_history_index = 0;
                            Command::Search(value)
                        } else {
                            Command::SubmitPastedLoginUrl(value)
                        };
                        self.send(command);
                    }
                    cx.notify();
                }
                KeyOutcome::Edited => cx.notify(),
                KeyOutcome::Blur => window.focus(&self.root_focus, cx),
                KeyOutcome::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                KeyOutcome::Cut(text) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    cx.notify();
                }
                KeyOutcome::Paste => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
                        && field.paste(&text)
                    {
                        cx.notify();
                    }
                }
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
            "/" => {
                self.nav = sidebar::NavSection::Search;
                window.focus(&self.search.focus, cx);
                cx.notify();
            }
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
                app.nav = sidebar::NavSection::Search;
                window.focus(&app.search.focus, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &NextTrack, _, _| app.send(Command::Next)))
            .on_action(cx.listener(|app, _: &PreviousTrack, _, _| app.send(Command::Previous)))
            .on_action(cx.listener(|app, _: &VolumeUp, _, _| {
                app.send(volume_command(&app.snapshot, 10));
            }))
            .on_action(cx.listener(|app, _: &VolumeDown, _, _| {
                app.send(volume_command(&app.snapshot, -10));
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(tone(BG, 0.80))
            .text_color(rgb(TEXT))
            // Body: sign-in takes the whole window until the runtime is
            // ready; then the Comet column layout — fixed sidebar, second
            // column with the browsed library listing, main area.
            .child(if ready(&snap) {
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .overflow_hidden()
                    .child(sidebar::render_sidebar(self, cx).into_any_element())
                    .when_some(
                        self.nav.library(),
                        |row, section| {
                            row.child(
                                library::render_library(self, section, cx).into_any_element(),
                            )
                        },
                    )
                    .when(self.nav == sidebar::NavSection::Settings, |row| {
                        row.child(self.render_settings().into_any_element())
                    })
                    .when(self.nav == sidebar::NavSection::Search, |row| {
                        row.child(self.render_search(window, cx).into_any_element())
                    })
                    .child(self.render_queue(cx))
                    .into_any_element()
            } else {
                self.render_sign_in(window, cx).into_any_element()
            })
            // Notice bar
            .children(snap.notice.as_ref().map(|notice| {
                div()
                    .px(px(18.))
                    .py(px(6.))
                    .bg(tone(PANEL, 0.60))
                    .border_t_1()
                    .border_color(border())
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
            .child(self.render_now_playing(&snap, cx))
            // Transport row
            .child(
                div()
                    .h(px(52.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(18.))
                    .border_t_1()
                    .border_color(border())
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
    fn render_settings(&self) -> impl IntoElement {
        div()
            .flex_1()
            .min_w_0()
            .p(px(18.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Settings"),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(rgb(MUTED))
                    .child("Nothing to configure yet."),
            )
    }

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
                .bg(tone(PANEL, 0.60))
                .border_1()
                .border_color(border())
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

    fn render_search(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let snap = &self.snapshot;

        // Search results column.
        let results = if let Some(Some(target)) = self.search_history.get(self.search_history_index) {
            let mut rows = Vec::new();
            let name = match target {
                SearchTarget::Artist { name, .. }
                | SearchTarget::Album { name, .. }
                | SearchTarget::Playlist { name, .. } => name,
            };
            rows.push(
                div()
                    .px(px(18.))
                    .pt(px(12.))
                    .pb(px(8.))
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(name.clone())
                    .into_any_element(),
            );
            match &snap.search_detail {
                Some(SearchDetail::Artist { tracks, albums }) => {
                    if !tracks.is_empty() {
                        rows.push(search_heading("Most famous tracks").into_any_element());
                        rows.extend(tracks.iter().enumerate().map(|(i, track)| {
                            library::track_row(track, None, i, cx).into_any_element()
                        }));
                    }
                    if !albums.is_empty() {
                        rows.push(search_heading("Albums").into_any_element());
                        rows.extend(albums.iter().enumerate().map(|(i, album)| {
                            let target = SearchTarget::Album {
                                locator: album.locator.clone(),
                                name: album.name.clone(),
                            };
                            div()
                                .id(SharedString::from(format!("artist-album-{i}")))
                                .w_full()
                                .px(px(14.))
                                .py(px(8.))
                                .border_b_1()
                                .border_color(border())
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .hover(|style| style.bg(tone(PANEL, 0.60)))
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.open_search_target(target.clone(), cx);
                                }))
                                .child(library::two_line_cell(
                                    album.name.clone(),
                                    album.artists.join(", "),
                                ))
                                .into_any_element()
                        }));
                    }
                }
                Some(SearchDetail::Album { tracks }) | Some(SearchDetail::Playlist { tracks }) => {
                    rows.push(search_heading("Tracks").into_any_element());
                    rows.extend(tracks.iter().enumerate().map(|(i, track)| {
                        library::track_row(track, None, i, cx).into_any_element()
                    }));
                }
                None => rows.push(status_row("Loading…".to_string()).into_any_element()),
            }
            div()
                .id("search-detail")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(rows)
                .into_any_element()
        } else { match &snap.search {
            SearchState::Done { results, .. }
                if !results.tracks.is_empty()
                    || !results.artists.is_empty()
                    || !results.albums.is_empty()
                    || !results.playlists.is_empty() =>
            {
                let mut rows = Vec::new();

                if !results.tracks.is_empty() {
                    rows.push(search_heading("Tracks").into_any_element());
                    rows.extend(results.tracks.iter().enumerate().map(|(i, playable)| {
                        library::track_row(playable, None, i, cx).into_any_element()
                    }));
                }
                if !results.artists.is_empty() {
                    rows.push(search_heading("Artists").into_any_element());
                    rows.extend(results.artists.iter().enumerate().map(|(i, artist)| {
                        let target = SearchTarget::Artist { locator: artist.locator.clone(), name: artist.name.clone() };
                        div()
                            .id(SharedString::from(format!("artist-{i}")))
                            .px(px(18.))
                            .py(px(8.))
                            .border_b_1()
                            .border_color(border())
                            .text_size(px(13.))
                            .cursor_pointer()
                            .hover(|style| style.bg(tone(PANEL, 0.60)))
                            .on_click(cx.listener(move |app, _, _, cx| app.open_search_target(target.clone(), cx)))
                            .child(artist.name.clone())
                            .into_any_element()
                    }));
                }
                if !results.albums.is_empty() {
                    rows.push(search_heading("Albums").into_any_element());
                    rows.extend(results.albums.iter().enumerate().map(|(i, album)| {
                        let target = SearchTarget::Album { locator: album.locator.clone(), name: album.name.clone() };
                        div()
                            .id(SharedString::from(format!("album-{i}")))
                            .w_full()
                            .px(px(14.))
                            .py(px(8.))
                            .border_b_1()
                            .border_color(border())
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .hover(|style| style.bg(tone(PANEL, 0.60)))
                            .on_click(cx.listener(move |app, _, _, cx| app.open_search_target(target.clone(), cx)))
                            .child(library::two_line_cell(
                                album.name.clone(),
                                album.artists.join(", "),
                            ))
                            .into_any_element()
                    }));
                }
                if !results.playlists.is_empty() {
                    rows.push(search_heading("Playlists").into_any_element());
                    rows.extend(results.playlists.iter().enumerate().map(|(i, playlist)| {
                        let target = SearchTarget::Playlist { locator: playlist.locator.clone(), name: playlist.name.clone() };
                        div()
                            .id(SharedString::from(format!("playlist-{i}")))
                            .w_full()
                            .px(px(14.))
                            .py(px(8.))
                            .border_b_1()
                            .border_color(border())
                            .flex()
                            .items_center()
                            .text_size(px(13.))
                            .cursor_pointer()
                            .hover(|style| style.bg(tone(PANEL, 0.60)))
                            .on_click(cx.listener(move |app, _, _, cx| app.open_search_target(target.clone(), cx)))
                            .child(library::two_line_cell(
                                playlist.name.clone(),
                                format!("{} · {} tracks", playlist.owner, playlist.track_count),
                            ))
                            .into_any_element()
                    }));
                }
                div()
                    .id("results")
                    .flex_1()
                    .min_h_0()
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
            SearchState::Done { .. } => {
                status_row("No results.".to_string()).into_any_element()
            }
            _ => div().into_any_element(),
        }};

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .px(px(18.))
                    .pt(px(14.))
                    .pb(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(search_nav_button("search-back", "←", self.search_history_index > 0, cx, -1))
                    .child(search_nav_button(
                        "search-forward",
                        "→",
                        self.search_history_index + 1 < self.search_history.len(),
                        cx,
                        1,
                    ))
                    .child(
                        div()
                            .flex_1()
                            // Ported from Comet's `search_input_frame`.
                            .mb(px(4.))
                            .px(px(10.))
                            .py(px(6.))
                            .rounded(px(8.))
                            .bg(wash(0.04))
                            .text_size(px(13.))
                            .child(self.search.render_search("search-input", window)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(results),
            )
    }

    fn render_queue(&self, cx: &Context<Self>) -> impl IntoElement {
        let snap = &self.snapshot;

        div()
            .w(px(300.))
            .flex_none()
            .min_h_0()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(border())
            .child(
                div()
                    .px(px(14.))
                    .py(px(10.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(format!("UP NEXT ({})", snap.queue.len())),
                    )
                    .child(small_button(
                        "clear-queue".into(),
                        "Clear",
                        !snap.queue.is_empty(),
                        cx.listener(|app, _, _, _| app.send(Command::ClearQueue)),
                    )),
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
                    .border_color(border())
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
                                true,
                                cx.listener(move |app, _, _, _| {
                                    app.send(Command::MoveQueued { index: i, up: true });
                                }),
                            ))
                            .child(small_button(
                                SharedString::from(format!("q-down-{i}")),
                                "↓",
                                true,
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
                                true,
                                cx.listener(move |app, _, _, _| {
                                    app.send(Command::RemoveQueued(i));
                                }),
                            )),
                    )
            }))
    }

    fn render_now_playing(&self, snap: &Snapshot, cx: &Context<Self>) -> impl IntoElement {
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
            .id("now-playing")
            .border_t_1()
            .border_color(border())
            .h(px(40.))
            .relative()
            .bg(tone(0x232328, 0.75))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .w(relative(progress))
                    .bg(Hsla::from(rgb(ACCENT)).opacity(0.55)),
            )
            .child(
                div()
                    .size_full()
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
            .on_click(cx.listener(move |app, event: &gpui::ClickEvent, window, _| {
                if duration_ms > 0 {
                    let fraction = (event.position().x / window.viewport_size().width).clamp(0., 1.);
                    app.send(Command::Seek((duration_ms as f32 * fraction) as u64));
                }
            }))
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
                .border_color(border())
                .text_size(px(11.))
                .text_color(rgb(TEXT))
                .cursor_pointer()
                .hover(|style| style.bg(tone(PANEL, 0.60)))
                .on_click(cx.listener(move |app, _, _, _| {
                    app.send(Command::Search(query.clone()));
                }))
                .child("Retry"),
        )
}

fn search_heading(label: impl Into<SharedString>) -> impl IntoElement {
    div()
        .px(px(18.))
        .pt(px(16.))
        .pb(px(6.))
        .text_size(px(11.))
        .text_color(rgb(MUTED))
        .child(label.into())
}

fn search_nav_button(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    cx: &Context<PlayerApp>,
    direction: isize,
) -> AnyElement {
    small_button(SharedString::from(id), label, enabled, cx.listener(move |app, _, _, cx| {
            let index = app.search_history_index.saturating_add_signed(direction);
            app.move_search_history(index, cx);
        }))
    .into_any_element()
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
        .bg(tone(PANEL, 0.60))
        .border_1()
        .border_color(border())
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|style| style.bg(tone(0x232328, 0.75)))
        .on_click(handler)
        .child(label.into())
}

fn small_button(
    id: SharedString,
    label: &'static str,
    enabled: bool,
    handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px(px(6.))
        .py(px(2.))
        .rounded(px(4.))
        .border_1()
        .border_color(border())
        .text_size(px(11.))
        .text_color(rgb(if enabled { TEXT } else { MUTED }))
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.bg(tone(PANEL, 0.60)))
                .on_click(handler)
        })
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
            // Frosted shell: the desktop shows through blurred behind the
            // translucent chrome (`tone`). Gate matches platforms whose
            // compositor *guarantees* blur in this gpui pin — macOS installs
            // an NSVisualEffectView, Windows drives DWM composition. Linux is
            // deliberately conservative: Wayland blurs only under KWin's
            // org_kde_kwin_blur and X11 gets no blur at all, so it stays
            // opaque rather than showing raw desktop through 80%-alpha chrome.
            //
            // THEME-SWITCH REQUIREMENT: if a future theme/appearance switcher
            // can change the palette at runtime, it MUST re-push this value
            // via `window.set_background_appearance(...)` after every change,
            // unconditionally. gpui's macOS backend tears the
            // NSVisualEffectView out of the window the moment the value is
            // anything but Blurred and nothing reinstates it on its own — a
            // single missed re-apply kills vibrancy until restart. Comet runs
            // this loop on every appearance change
            // (crates/ui/src/appearance.rs `apply` + `reapply_window_background`),
            // as does zed's main.rs.
            window_background: if GLASS {
                WindowBackgroundAppearance::Blurred
            } else {
                WindowBackgroundAppearance::Opaque
            },
            app_id: Some("rust-player".into()),
            ..Default::default()
        },
        |window, cx| {
            let mut rx = runtime.subscribe();
            let mut position_rx = runtime.subscribe();
            let app = cx.new(|cx| {
                // Fold published snapshots into the entity.
                cx.spawn(async move |this, cx| {
                    loop {
                        let snapshot = rx.borrow_and_update().clone();
                        if this
                            .update(cx, |app: &mut PlayerApp, cx| {
                                // Delta gate (ticket 13): identical snapshots
                                // cause no clone and no re-render.
                                if app.snapshot == snapshot {
                                    return;
                                }
                                app.snapshot = snapshot;
                                // First ready snapshot: load the section the
                                // sidebar opens with, so the second column is
                                // never a dead "choose a section" hint.
                                if let Some(section) = app.nav.library()
                                    && ready(&app.snapshot)
                                    && matches!(app.snapshot.library, LibraryState::Idle)
                                {
                                    app.send(Command::Browse(section));
                                }
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

                // Position projection (ticket 13): while playing, the window
                // redraws on animation frames so `projected_position_ms(now)`
                // renders smoothly; paused/idle windows schedule nothing and
                // receive no wakes at all. Snapshot events above restart the
                // pump when playback resumes.
                cx.spawn(async move |this, cx| {
                    loop {
                        let playing = this.read_with(cx, |app, _| {
                            app.snapshot.is_playing()
                        })?;
                        if !playing {
                            // Sleep until the next snapshot event; re-check on wake.
                            if position_rx.changed().await.is_err() {
                                break;
                            }
                            continue;
                        }
                        this.update(cx, |_, cx| {
                            cx.notify();
                        })?;
                        cx.background_executor()
                            .timer(Duration::from_millis(250))
                            .await;
                    }
                    anyhow::Ok(())
                })
                .detach();

                PlayerApp {
                    snapshot: initial_snapshot,
                    nav: sidebar::NavSection::LikedSongs,
                    root_focus: cx.focus_handle(),
                    search: TextField::new(cx, "Search Spotify…"),
                    paste: TextField::new(cx, "http://127.0.0.1:8989/login?code=…"),
                    search_history: vec![None],
                    search_history_index: 0,
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

    let application = gpui_platform::application().with_assets(icons::Assets);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_preserves_rgb_and_requested_alpha() {
        let actual = tone(BG, 0.80).to_rgb();
        let expected = rgb(BG);
        let expected_alpha = if GLASS { 0.80 } else { 1.0 };

        assert!((actual.r - expected.r).abs() < f32::EPSILON);
        assert!((actual.g - expected.g).abs() < f32::EPSILON);
        assert!((actual.b - expected.b).abs() < f32::EPSILON);
        assert!((actual.a - expected_alpha).abs() < f32::EPSILON);
    }
}
