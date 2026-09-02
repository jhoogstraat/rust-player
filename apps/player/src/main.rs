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

use std::cell::RefCell;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use gpui::{
    Anchor, AnyElement, App, Bounds, Context, FontWeight, Hsla, IntoElement, KeyBinding,
    MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render, SharedString, Styled,
    Window, WindowBackgroundAppearance, WindowBounds, WindowOptions, actions, anchored, deferred,
    div, hsla, prelude::*, px, relative, rgb, uniform_list,
};
use player_core::{
    Command, LibraryState, LoginState, Playable, PlaybackList, PlaybackListProjector, Runtime,
    SearchAlbum, SearchDetail, SearchState, SearchTarget, Snapshot, fake::FakeRuntime,
};
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
    [FocusSearch, NextTrack, PreviousTrack, VolumeUp, VolumeDown,]
);

static RUNTIME: OnceLock<Arc<dyn Runtime>> = OnceLock::new();
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static PERFORMANCE: OnceLock<Arc<Performance>> = OnceLock::new();

/// Opt-in counters used by the documented performance baseline. Atomics keep
/// the hot render/subscription paths lock-free; with the flag off the methods
/// short-circuit and no log is emitted.
#[derive(Debug)]
struct Performance {
    enabled: bool,
    snapshots: AtomicU64,
    catalog_changes: AtomicU64,
    animation_frames: AtomicU64,
    playback_renders: AtomicU64,
    render_time_ns: AtomicU64,
    render_max_ns: AtomicU64,
}

impl Performance {
    fn new() -> Self {
        Self {
            enabled: std::env::var_os("RUST_PLAYER_PERF").is_some_and(|value| value != "0"),
            snapshots: AtomicU64::new(0),
            catalog_changes: AtomicU64::new(0),
            animation_frames: AtomicU64::new(0),
            playback_renders: AtomicU64::new(0),
            render_time_ns: AtomicU64::new(0),
            render_max_ns: AtomicU64::new(0),
        }
    }

    fn snapshot(&self, previous: &Snapshot, next: &Snapshot) {
        if !self.enabled {
            return;
        }
        self.snapshots.fetch_add(1, Ordering::Relaxed);
        if previous.search != next.search
            || previous.search_detail != next.search_detail
            || previous.library != next.library
        {
            self.catalog_changes.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn render(&self, elapsed: std::time::Duration, playing: bool) {
        if !self.enabled {
            return;
        }
        if playing {
            self.animation_frames.fetch_add(1, Ordering::Relaxed);
            self.playback_renders.fetch_add(1, Ordering::Relaxed);
            let elapsed_ns = elapsed.as_nanos().min(u64::MAX as u128) as u64;
            self.render_time_ns.fetch_add(elapsed_ns, Ordering::Relaxed);
            self.render_max_ns.fetch_max(elapsed_ns, Ordering::Relaxed);
        }
    }

    fn summary(&self) -> String {
        let renders = self.playback_renders.load(Ordering::Relaxed);
        let total_ns = self.render_time_ns.load(Ordering::Relaxed);
        let average_us = if renders == 0 {
            0
        } else {
            total_ns / renders / 1_000
        };
        format!(
            "snapshots={} catalog_changes={} animation_frame_requests={} playback_renders={} render_avg_us={} render_max_us={}",
            self.snapshots.load(Ordering::Relaxed),
            self.catalog_changes.load(Ordering::Relaxed),
            self.animation_frames.load(Ordering::Relaxed),
            renders,
            average_us,
            self.render_max_ns.load(Ordering::Relaxed) / 1_000,
        )
    }
}

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
    performance: Arc<Performance>,
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
    show_playing_list: bool,
    playback_list_projector: RefCell<PlaybackListProjector>,
    context_menu: Option<ContextMenu>,
}

enum ContextMenu {
    Track {
        playable: Playable,
        list: Option<Arc<PlaybackList>>,
        index: usize,
        position: Point<Pixels>,
    },
    Album {
        album: SearchAlbum,
        position: Point<Pixels>,
    },
}

impl PlayerApp {
    fn send(&self, command: Command) {
        let runtime = RUNTIME.get().expect("runtime");
        if !runtime.command(command.clone()) {
            log::warn!("[ui] runtime rejected {command:?}");
        }
    }

    fn open_search_target(&mut self, target: SearchTarget, cx: &mut Context<Self>) {
        self.nav = sidebar::NavSection::Search;
        self.show_playing_list = false;
        self.search_history.truncate(self.search_history_index + 1);
        self.search_history.push(Some(target.clone()));
        self.search_history_index += 1;
        self.send(Command::OpenSearchTarget(target));
        cx.notify();
    }

    fn search_for(&mut self, query: String, cx: &mut Context<Self>) {
        self.nav = sidebar::NavSection::Search;
        self.show_playing_list = false;
        self.search.clear();
        self.search_history = vec![None];
        self.search_history_index = 0;
        self.send(Command::Search(query));
        cx.notify();
    }

    fn artist_target(&self, name: &str) -> Option<SearchTarget> {
        if let SearchState::Done { results, .. } = &self.snapshot.search
            && let Some(artist) = results.artists.iter().find(|artist| artist.name == name)
        {
            return Some(SearchTarget::Artist {
                locator: artist.locator.clone(),
                name: artist.name.clone(),
            });
        }

        self.search_history
            .get(self.search_history_index)
            .and_then(|target| target.as_ref())
            .and_then(|target| match target {
                SearchTarget::Artist {
                    locator,
                    name: target_name,
                } if target_name == name => Some(SearchTarget::Artist {
                    locator: locator.clone(),
                    name: target_name.clone(),
                }),
                _ => None,
            })
    }

    fn album_target(&self, name: &str) -> Option<SearchTarget> {
        if let SearchState::Done { results, .. } = &self.snapshot.search
            && let Some(album) = results.albums.iter().find(|album| album.name == name)
        {
            return Some(SearchTarget::Album {
                locator: album.locator.clone(),
                name: album.name.clone(),
            });
        }

        if let Some(target) = self
            .search_history
            .get(self.search_history_index)
            .and_then(|target| target.as_ref())
            && let SearchTarget::Album {
                locator,
                name: target_name,
            } = target
            && target_name == name
        {
            return Some(SearchTarget::Album {
                locator: locator.clone(),
                name: target_name.clone(),
            });
        }

        if let Some(SearchDetail::Artist { albums, .. }) = &self.snapshot.search_detail
            && let Some(album) = albums.iter().find(|album| album.name == name)
        {
            return Some(SearchTarget::Album {
                locator: album.locator.clone(),
                name: album.name.clone(),
            });
        }

        None
    }

    fn open_named_target(
        &mut self,
        target: Option<SearchTarget>,
        name: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(target) = target {
            self.open_search_target(target, cx);
        } else {
            self.search_for(name, cx);
        }
        self.close_context_menu(cx);
    }

    pub(crate) fn open_track_context_menu(
        &mut self,
        playable: Playable,
        list: Option<Arc<PlaybackList>>,
        index: usize,
        position: Point<Pixels>,
    ) {
        self.context_menu = Some(ContextMenu::Track {
            playable,
            list,
            index,
            position,
        });
    }

    fn open_album_context_menu(&mut self, album: SearchAlbum, position: Point<Pixels>) {
        self.context_menu = Some(ContextMenu::Album { album, position });
    }

    fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    fn render_playback_list(&self, cx: &Context<Self>) -> impl IntoElement {
        let Some(list) = self.snapshot.implicit_queue.clone() else {
            return div()
                .flex_1()
                .min_w_0()
                .child(status_row("Nothing is queued implicitly.".to_string()));
        };
        let list = Arc::new(list);
        let source_label = list.source.label();
        let count = list.tracks.len();
        let rows_list = list.clone();
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_r_1()
            .border_color(border())
            .child(
                div()
                    .px(px(18.))
                    .py(px(12.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Playing list"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(MUTED))
                                    .child(format!("{source_label} · {count} tracks")),
                            ),
                    )
                    .child(small_button(
                        "close-playing-list".into(),
                        "Back",
                        true,
                        cx.listener(|app, _, _, cx| {
                            app.show_playing_list = false;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .id("playing-list-viewport")
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        uniform_list(
                            "playing-list-rows",
                            count,
                            cx.processor(move |_app, range: Range<usize>, _, cx| {
                                range
                                    .filter_map(|index| {
                                        rows_list.tracks.get(index).map(|track| {
                                            library::track_row_in_list(
                                                track,
                                                None,
                                                index,
                                                rows_list.clone(),
                                                cx,
                                            )
                                            .into_any_element()
                                        })
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .size_full(),
                    ),
            )
    }

    fn context_menu_item(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let id: SharedString = id.into();
        div()
            .id(id)
            .w_full()
            .px(px(9.))
            .py(px(7.))
            .rounded(px(6.))
            .text_size(px(13.))
            .cursor_pointer()
            .hover(|style| style.bg(wash(0.10)))
            .on_click(handler)
            .child(label.into())
    }

    fn render_context_menu(&self, cx: &Context<Self>) -> AnyElement {
        let Some(menu) = &self.context_menu else {
            return div().into_any_element();
        };
        let position = match menu {
            ContextMenu::Track { position, .. } | ContextMenu::Album { position, .. } => *position,
        };
        let menu_id = match menu {
            ContextMenu::Track { .. } => "track-context-menu",
            ContextMenu::Album { .. } => "album-context-menu",
        };
        let mut card = div()
            .id(menu_id)
            .w(px(230.))
            .p(px(4.))
            .rounded(px(10.))
            .border_1()
            .border_color(border())
            .bg(tone(PANEL, 0.96))
            .shadow_lg()
            .text_color(rgb(TEXT))
            .on_mouse_down_out(cx.listener(|app, _, _, cx| app.close_context_menu(cx)));

        match menu {
            ContextMenu::Track {
                playable,
                list,
                index,
                ..
            } => {
                let play_command = list.clone().map_or_else(
                    || Command::Play(playable.clone()),
                    |list| Command::PlayFromList {
                        list,
                        index: *index,
                    },
                );
                let enqueue = playable.clone();
                card = card
                    .child(Self::context_menu_item(
                        "context-play-track",
                        "Play",
                        cx.listener(move |app, _, _, cx| {
                            app.send(play_command.clone());
                            app.close_context_menu(cx);
                        }),
                    ))
                    .child(Self::context_menu_item(
                        "context-enqueue-track",
                        "Add to queue",
                        cx.listener(move |app, _, _, cx| {
                            app.send(Command::Enqueue(enqueue.clone()));
                            app.close_context_menu(cx);
                        }),
                    ))
                    .child(div().h(px(1.)).my(px(4.)).bg(border()));

                for (index, artist) in playable
                    .artists
                    .iter()
                    .filter(|artist| !artist.is_empty())
                    .enumerate()
                {
                    let artist = artist.clone();
                    let target = self.artist_target(&artist);
                    card = card.child(Self::context_menu_item(
                        SharedString::from(format!("context-track-artist-{index}")),
                        format!("Go to {artist}"),
                        cx.listener(move |app, _, _, cx| {
                            app.open_named_target(target.clone(), artist.clone(), cx);
                        }),
                    ));
                }

                let album = playable.album.clone();
                if !album.is_empty() {
                    let target = self.album_target(&album);
                    card = card.child(Self::context_menu_item(
                        "context-track-album",
                        format!("Go to {album}"),
                        cx.listener(move |app, _, _, cx| {
                            app.open_named_target(target.clone(), album.clone(), cx);
                        }),
                    ));
                }
            }
            ContextMenu::Album { album, .. } => {
                let target = SearchTarget::Album {
                    locator: album.locator.clone(),
                    name: album.name.clone(),
                };
                let artists = album.artists.clone();
                card = card.child(Self::context_menu_item(
                    "context-open-album",
                    "Open album",
                    cx.listener(move |app, _, _, cx| {
                        app.open_search_target(target.clone(), cx);
                        app.close_context_menu(cx);
                    }),
                ));
                for (index, artist) in artists
                    .into_iter()
                    .filter(|artist| !artist.is_empty())
                    .enumerate()
                {
                    let target = self.artist_target(&artist);
                    card = card.child(Self::context_menu_item(
                        SharedString::from(format!("context-album-artist-{index}")),
                        format!("Go to {artist}"),
                        cx.listener(move |app, _, _, cx| {
                            app.open_named_target(target.clone(), artist.clone(), cx);
                        }),
                    ));
                }
            }
        }

        deferred(
            anchored()
                .position(position)
                .anchor(Anchor::TopLeft)
                .snap_to_window_with_margin(px(8.))
                .child(div().occlude().child(card)),
        )
        .priority(1)
        .into_any_element()
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
        if event.keystroke.key == "escape" && self.context_menu.is_some() {
            self.close_context_menu(cx);
            return;
        }

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
                self.show_playing_list = false;
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
        let playing = self.snapshot.is_playing();
        let started = self.performance.enabled.then(Instant::now);
        if playing {
            window.request_animation_frame();
        }
        let snap = &self.snapshot;

        let element = div()
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
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|app, _, window, cx| {
                    app.close_context_menu(cx);
                    window.prevent_default();
                }),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(tone(BG, 0.80))
            .text_color(rgb(TEXT))
            // Body: sign-in takes the whole window until the runtime is
            // ready; then the Comet column layout — fixed sidebar, second
            // column with the browsed library listing, main area.
            .child(if ready(&snap) {
                let content = if self.show_playing_list {
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .flex()
                        .child(self.render_playback_list(cx))
                        .into_any_element()
                } else {
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .flex()
                        .when_some(self.nav.library(), |row, section| {
                            row.child(library::render_library(self, section, cx).into_any_element())
                        })
                        .when(self.nav == sidebar::NavSection::Settings, |row| {
                            row.child(self.render_settings().into_any_element())
                        })
                        .when(self.nav == sidebar::NavSection::Search, |row| {
                            row.child(self.render_search(window, cx).into_any_element())
                        })
                        .into_any_element()
                };
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .overflow_hidden()
                    .child(sidebar::render_sidebar(self, cx).into_any_element())
                    .child(content)
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
            .child(self.render_context_menu(cx))
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
                        icons::icon(if snap.is_playing() {
                            icons::PAUSE
                        } else {
                            icons::PLAY
                        })
                        .size(px(16.))
                        .text_color(rgb(TEXT)),
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
            );
        if let Some(started) = started {
            self.performance.render(started.elapsed(), playing);
        }
        element
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
        let results = if let Some(Some(target)) = self.search_history.get(self.search_history_index)
        {
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
            let detail_list = self
                .playback_list_projector
                .borrow_mut()
                .project_detail(target, snap.search_detail.as_ref());
            match &snap.search_detail {
                Some(SearchDetail::Artist { tracks, albums, .. }) => {
                    if let Some(list) = detail_list.as_ref() {
                        rows.push(search_heading("Most famous tracks").into_any_element());
                        rows.extend(tracks.iter().enumerate().map(|(i, track)| {
                            library::track_row_in_list(track, None, i, Arc::clone(list), cx)
                                .into_any_element()
                        }));
                    }
                    if !albums.is_empty() {
                        rows.push(search_heading("Albums").into_any_element());
                        rows.extend(albums.iter().enumerate().map(|(i, album)| {
                            let target = SearchTarget::Album {
                                locator: album.locator.clone(),
                                name: album.name.clone(),
                            };
                            let context_album = album.clone();
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
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |app, event: &MouseDownEvent, window, cx| {
                                        app.open_album_context_menu(
                                            context_album.clone(),
                                            event.position,
                                        );
                                        window.prevent_default();
                                        cx.stop_propagation();
                                        cx.notify();
                                    }),
                                )
                                .child(library::two_line_cell(
                                    album.name.clone(),
                                    album.artists.join(", "),
                                ))
                                .into_any_element()
                        }));
                    }
                }
                Some(SearchDetail::Album { tracks, .. })
                | Some(SearchDetail::Playlist { tracks, .. }) => {
                    if let Some(list) = detail_list.as_ref() {
                        rows.push(search_heading("Tracks").into_any_element());
                        rows.extend(tracks.iter().enumerate().map(|(i, track)| {
                            library::track_row_in_list(track, None, i, Arc::clone(list), cx)
                                .into_any_element()
                        }));
                    }
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
        } else {
            let playback_list = self
                .playback_list_projector
                .borrow_mut()
                .project_search(&snap.search);
            match &snap.search {
                SearchState::Done { results, .. }
                    if !results.tracks.is_empty()
                        || !results.artists.is_empty()
                        || !results.albums.is_empty()
                        || !results.playlists.is_empty() =>
                {
                    let mut rows = Vec::new();

                    if let Some(list) = playback_list.as_ref() {
                        rows.push(search_heading("Tracks").into_any_element());
                        rows.extend(results.tracks.iter().enumerate().map(|(i, playable)| {
                            library::track_row_in_list(playable, None, i, Arc::clone(list), cx)
                                .into_any_element()
                        }));
                    }
                    if !results.artists.is_empty() {
                        rows.push(search_heading("Artists").into_any_element());
                        rows.extend(results.artists.iter().enumerate().map(|(i, artist)| {
                            let target = SearchTarget::Artist {
                                locator: artist.locator.clone(),
                                name: artist.name.clone(),
                            };
                            div()
                                .id(SharedString::from(format!("artist-{i}")))
                                .px(px(18.))
                                .py(px(8.))
                                .border_b_1()
                                .border_color(border())
                                .text_size(px(13.))
                                .cursor_pointer()
                                .hover(|style| style.bg(tone(PANEL, 0.60)))
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.open_search_target(target.clone(), cx)
                                }))
                                .child(artist.name.clone())
                                .into_any_element()
                        }));
                    }
                    if !results.albums.is_empty() {
                        rows.push(search_heading("Albums").into_any_element());
                        rows.extend(results.albums.iter().enumerate().map(|(i, album)| {
                            let target = SearchTarget::Album {
                                locator: album.locator.clone(),
                                name: album.name.clone(),
                            };
                            let context_album = album.clone();
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
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.open_search_target(target.clone(), cx)
                                }))
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |app, event: &MouseDownEvent, window, cx| {
                                        app.open_album_context_menu(
                                            context_album.clone(),
                                            event.position,
                                        );
                                        window.prevent_default();
                                        cx.stop_propagation();
                                        cx.notify();
                                    }),
                                )
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
                            let target = SearchTarget::Playlist {
                                locator: playlist.locator.clone(),
                                name: playlist.name.clone(),
                                from_search: true,
                            };
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
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.open_search_target(target.clone(), cx)
                                }))
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
            }
        };

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
                    .child(search_nav_button(
                        "search-back",
                        "←",
                        self.search_history_index > 0,
                        cx,
                        -1,
                    ))
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
        let has_playing_list = snap.implicit_queue.is_some();
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .overflow_hidden()
                            .child(title_line),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(if duration_ms > 0 {
                                format!("{} / {}", clock(position_ms), clock(duration_ms))
                            } else {
                                String::new()
                            }),
                    )
                    .when(has_playing_list, |bar| {
                        bar.child(div().flex_none().child(small_button(
                            "view-playing-list".into(),
                            "View list",
                            true,
                            cx.listener(|app, _, _, cx| {
                                app.show_playing_list = true;
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )))
                    }),
            )
            .cursor_pointer()
            .on_click(
                cx.listener(move |app, event: &gpui::ClickEvent, window, _| {
                    if duration_ms > 0 {
                        let fraction =
                            (event.position().x / window.viewport_size().width).clamp(0., 1.);
                        app.send(Command::Seek((duration_ms as f32 * fraction) as u64));
                    }
                }),
            )
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
    small_button(
        SharedString::from(id),
        label,
        enabled,
        cx.listener(move |app, _, _, cx| {
            let index = app.search_history_index.saturating_add_signed(direction);
            app.move_search_history(index, cx);
        }),
    )
    .into_any_element()
}

fn input_shell(child: impl IntoElement) -> impl IntoElement {
    div().pb(px(10.)).max_w(px(480.)).child(child)
}

fn button(
    id: &'static str,
    label: impl IntoElement,
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
        .child(label)
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
    let performance = PERFORMANCE
        .get()
        .expect("performance initialized before window opens")
        .clone();

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
                                app.performance.snapshot(&app.snapshot, &snapshot);
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

                PlayerApp {
                    snapshot: initial_snapshot,
                    performance,
                    nav: sidebar::NavSection::LikedSongs,
                    root_focus: cx.focus_handle(),
                    search: TextField::new(cx, "Search Spotify…"),
                    paste: TextField::new(cx, "http://127.0.0.1:8989/login?code=…"),
                    search_history: vec![None],
                    search_history_index: 0,
                    show_playing_list: false,
                    playback_list_projector: RefCell::default(),
                    context_menu: None,
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
    let performance = Arc::new(Performance::new());
    if performance.enabled {
        log::info!("[perf] enabled (RUST_PLAYER_PERF=1)");
    }
    let _ = PERFORMANCE.set(performance.clone());

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
        cx.on_app_quit(move |_cx| {
            let performance = performance.clone();
            async move {
                if let Some(runtime) = RUNTIME.get() {
                    runtime.shutdown();
                }
                if performance.enabled {
                    log::info!(
                        "[perf] {} {}",
                        performance.summary(),
                        player_spotatui::performance_summary()
                    );
                }
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

    #[test]
    fn performance_summary_separates_catalog_and_playback_work() {
        let metrics = Performance {
            enabled: true,
            snapshots: AtomicU64::new(0),
            catalog_changes: AtomicU64::new(0),
            animation_frames: AtomicU64::new(0),
            playback_renders: AtomicU64::new(0),
            render_time_ns: AtomicU64::new(0),
            render_max_ns: AtomicU64::new(0),
        };
        let before = Snapshot::default();
        let mut catalog = before.clone();
        catalog.search = SearchState::Loading {
            query: "baseline".to_string(),
        };
        metrics.snapshot(&before, &catalog);
        metrics.render(std::time::Duration::from_micros(4), true);
        assert_eq!(
            metrics.summary(),
            "snapshots=1 catalog_changes=1 animation_frame_requests=1 playback_renders=1 render_avg_us=4 render_max_us=4"
        );
    }

    #[test]
    fn legacy_ui_list_caches_and_constructors_are_deleted() {
        let player = include_str!("main.rs");
        let library = include_str!("library.rs");
        let app = &player
            [player.find("struct PlayerApp").unwrap()..player.find("enum ContextMenu").unwrap()];
        let helpers = &player[..player.find("impl Render for PlayerApp").unwrap()];

        assert!(!app.contains("library_playback_list"));
        assert!(!helpers.contains("fn playback_list("));
        assert!(!library.contains("playback_list_for_library"));
        assert!(!library.contains("update_library_playback_list_cache"));
    }
}
