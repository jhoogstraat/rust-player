//! The second column: the browsed library listing for the active sidebar
//! section. Track rows reuse the search-result recipe (title/artists |
//! album | duration, click plays, chip enqueues); playlist rows open their
//! track list.

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, MouseButton, MouseDownEvent, ParentElement,
    SharedString, Styled, div, prelude::*, px, uniform_list,
};
use player_core::{
    Command, LibraryEntry, LibrarySection, LibraryState, PlaybackList, SearchTarget,
};

use crate::{ACCENT, MUTED, PlayerApp, border, clock, rgb, wash};

/// The listing never collapses below a readable table width.
pub(crate) const LIBRARY_MIN_WIDTH: f32 = 300.0;

/// The second column for `section`, fed by `Snapshot::library`.
pub(crate) fn render_library(
    app: &PlayerApp,
    section: LibrarySection,
    cx: &Context<PlayerApp>,
) -> impl IntoElement {
    let playback_list = app
        .playback_list_projector
        .borrow_mut()
        .project_library(&app.snapshot.library);
    let (count, body) = match &app.snapshot.library {
        LibraryState::Idle => (
            None,
            status_row("Choose a section.".to_string()).into_any_element(),
        ),
        LibraryState::Loading { .. } => {
            (None, status_row("Loading…".to_string()).into_any_element())
        }
        LibraryState::Failed { message, .. } => {
            (None, status_row(message.clone()).into_any_element())
        }
        LibraryState::Done { entries, .. } => {
            let count = Some(entries.len());
            (
                count,
                // Each library section contains one row shape, so the
                // uniform list can lay out only the visible range.
                uniform_list(
                    "library-rows",
                    entries.len(),
                    cx.processor(move |app, range: Range<usize>, _, cx| {
                        let LibraryState::Done { entries, .. } = &app.snapshot.library else {
                            return Vec::new();
                        };
                        range
                            .filter_map(|index| {
                                entries.get(index).map(|entry| {
                                    render_library_entry(entry, index, playback_list.as_ref(), cx)
                                })
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
    playback_list: Option<&Arc<PlaybackList>>,
    cx: &Context<PlayerApp>,
) -> AnyElement {
    match entry {
        LibraryEntry::Track { playable } => {
            track_row(playable, index, playback_list.cloned(), cx).into_any_element()
        }
        LibraryEntry::Playlist {
            id,
            name,
            track_count,
            ..
        } => playlist_row(id, name, *track_count, index, cx).into_any_element(),
    }
}

/// One playable track row: click plays, the chip enqueues. With `list`,
/// playing the row also installs that list as the implicit playback list.
pub(crate) fn track_row(
    playable: &player_core::Playable,
    index: usize,
    list: Option<Arc<PlaybackList>>,
    cx: &Context<PlayerApp>,
) -> impl IntoElement {
    let play = playable.clone();
    let enqueue = playable.clone();
    let context_playable = playable.clone();
    let play_command = list.clone().map_or_else(
        || Command::Play(play.clone()),
        |list| Command::PlayFromList { list, index },
    );
    let pending_playable = playable.clone();
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
        .hover(|style| style.bg(wash(0.05)))
        .on_click(cx.listener(move |app, _, _, cx| {
            app.begin_playback(pending_playable.clone(), play_command.clone(), cx);
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |app, event: &MouseDownEvent, window, cx| {
                app.open_track_context_menu(
                    context_playable.clone(),
                    list.clone(),
                    index,
                    event.position,
                );
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .child(two_line_cell(
            playable.title.clone(),
            format!("{} — {}", playable.artists_display(), playable.album),
        ))
        .child(
            div()
                .flex_none()
                .text_size(px(11.0))
                .text_color(rgb(MUTED))
                .child(clock(playable.duration_ms)),
        )
        .child(enqueue_chip(index, enqueue, cx))
}

pub(crate) fn two_line_cell(title: String, subtitle: String) -> impl IntoElement {
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
                .child(title),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(rgb(MUTED))
                .overflow_hidden()
                .whitespace_nowrap()
                .truncate()
                .child(subtitle),
        )
}

fn enqueue_chip(
    index: usize,
    enqueue: player_core::Playable,
    cx: &Context<PlayerApp>,
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

/// One playlist row. Clicking opens its track list.
fn playlist_row(
    id: &str,
    name: &str,
    track_count: u32,
    index: usize,
    cx: &Context<PlayerApp>,
) -> impl IntoElement {
    let target = SearchTarget::Playlist {
        locator: id.to_string(),
        name: name.to_string(),
        from_search: false,
    };
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
        .cursor_pointer()
        .hover(|style| style.bg(wash(0.05)))
        .on_click(cx.listener(move |app, _, _, cx| {
            app.open_search_target(target.clone(), cx);
        }))
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
