//! The second column: the browsed library listing for the active sidebar
//! section. Track rows reuse the search-result recipe (title/artists |
//! album | duration, click plays, chip enqueues); playlist rows show name
//! and size — drilling into one is deferred until a source exposes it.

use std::ops::Range;

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement, SharedString, Styled, div,
    prelude::*, px, uniform_list,
};
use player_core::{LibraryEntry, LibrarySection, LibraryState};

use crate::{PlayerApp, border, rgb, tone, ACCENT, MUTED, PANEL};

/// The listing never collapses below a readable table width.
pub(crate) const LIBRARY_MIN_WIDTH: f32 = 300.0;

/// The second column for `section`, fed by `Snapshot::library`.
pub(crate) fn render_library(
    app: &PlayerApp,
    section: LibrarySection,
    cx: &mut Context<PlayerApp>,
) -> impl IntoElement {
    let (count, body) = match &app.snapshot.library {
        LibraryState::Idle => (None, status_row("Choose a section.".to_string()).into_any_element()),
        LibraryState::Loading { .. } => (
            None,
            status_row("Loading…".to_string()).into_any_element(),
        ),
        LibraryState::Failed { message, .. } => (
            None,
            status_row(message.clone()).into_any_element(),
        ),
        LibraryState::Done { entries, .. } => {
            let count = Some(entries.len());
            (
                count,
                // Each library section contains one row shape, so the
                // uniform list can lay out only the visible range.
                uniform_list(
                    "library-rows",
                    entries.len(),
                    cx.processor(|app, range: Range<usize>, _, cx| {
                        let LibraryState::Done { entries, .. } = &app.snapshot.library else {
                            return Vec::new();
                        };
                        range
                            .filter_map(|index| {
                                entries
                                    .get(index)
                                    .map(|entry| render_library_entry(entry, index, cx))
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .into_any_element(),
            )
        }
    };

    div()
        .flex_1()
        .min_w(px(LIBRARY_MIN_WIDTH))
        .h_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .border_r_1()
        .border_color(border())
        // Header
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(8.0))
                .px(px(14.0))
                .py(px(10.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(section.label()),
                )
                .children(count.map(|count| {
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(MUTED))
                        .child(format!("{count} items"))
                })),
        )
        // Scrollable listing
        .child(
            div()
                .id("library-list")
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(body),
        )
}

fn render_library_entry(
    entry: &LibraryEntry,
    index: usize,
    cx: &mut Context<PlayerApp>,
) -> AnyElement {
    match entry {
        LibraryEntry::Track {
            playable,
            played_at_ms,
        } => track_row(playable, *played_at_ms, index, cx).into_any_element(),
        LibraryEntry::Playlist {
            name,
            track_count,
            ..
        } => playlist_row(name, *track_count, index).into_any_element(),
    }
}

/// One playable track row: click plays, the chip enqueues.
fn track_row(
    playable: &player_core::Playable,
    played_at_ms: Option<u64>,
    index: usize,
    cx: &mut Context<PlayerApp>,
) -> impl IntoElement {
    let play = playable.clone();
    let enqueue = playable.clone();
    div()
        .id(SharedString::from(format!("library-track-{index}")))
        .w_full()
        .px(px(14.0))
        .py(px(8.0))
        .border_b_1()
        .border_color(border())
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .cursor_pointer()
        .hover(|style| style.bg(tone(PANEL, 0.60)))
        .on_click(cx.listener(move |app, _, _, _| app.send(player_core::Command::Play(play.clone()))))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .truncate()
                        .child(playable.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(MUTED))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .truncate()
                        .child(format!(
                            "{} — {}",
                            playable.artists_display(),
                            playable.album
                        )),
                ),
        )
        .children(played_at_ms.map(|at| {
            div()
                .flex_none()
                .text_size(px(11.0))
                .text_color(rgb(MUTED))
                .child(time_ago(at))
        }))
        .child(
            div()
                .flex_none()
                .text_size(px(11.0))
                .text_color(rgb(MUTED))
                .child(clock(playable.duration_ms)),
        )
        .child(enqueue_chip(index, enqueue, cx))
}

fn enqueue_chip(
    index: usize,
    enqueue: player_core::Playable,
    cx: &mut Context<PlayerApp>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("library-enqueue-{index}")))
        .px(px(8.0))
        .py(px(3.0))
        .rounded(px(5.0))
        .border_1()
        .border_color(border())
        .text_size(px(11.0))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(ACCENT)))
        .on_click(cx.listener(move |app, _, _, cx| {
            app.send(player_core::Command::Enqueue(enqueue.clone()));
            cx.stop_propagation();
        }))
        .child("+ Queue")
}

/// One playlist row. Drilling in is deferred until a source exposes it, so
/// the row is display-only.
fn playlist_row(name: &str, track_count: u32, index: usize) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("library-playlist-{index}")))
        .w_full()
        .px(px(14.0))
        .py(px(8.0))
        .border_b_1()
        .border_color(border())
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(13.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .truncate()
                .child(name.to_string()),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(rgb(MUTED))
                .child(format!("{track_count} tracks")),
        )
}

fn status_row(text: String) -> impl IntoElement {
    div()
        .px(px(14.0))
        .py(px(10.0))
        .text_size(px(12.0))
        .text_color(rgb(MUTED))
        .child(text)
}

/// Coarse relative time for Recently played stamps.
fn time_ago(unix_ms: u64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let minutes_ago = now_ms.saturating_sub(unix_ms) / 60_000;
    if minutes_ago < 2 {
        "just now".to_string()
    } else if minutes_ago < 60 {
        format!("{minutes_ago}m ago")
    } else if minutes_ago < 24 * 60 {
        format!("{}h ago", minutes_ago / 60)
    } else if minutes_ago < 7 * 24 * 60 {
        format!("{}d ago", minutes_ago / (24 * 60))
    } else {
        format!("{}w ago", minutes_ago / (7 * 24 * 60))
    }
}

/// `m:ss` (shared shape with the transport's clock; kept local so the
/// column renders without reaching into layout code).
fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
