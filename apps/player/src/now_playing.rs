//! The now-playing presentation seam.
//!
//! This is the only GPUI view that projects the moving Playback Session
//! position. It owns no transport authority: the engine's source-neutral
//! `PlaybackStatus` remains the input, and controls send commands back to the
//! runtime through the parent application's command seam.

use std::sync::Arc;
use std::time::Instant;

use gpui::{
    ClickEvent, Context, EventEmitter, Hsla, IntoElement, ParentElement, Styled, Window, div,
    prelude::*, px, relative, rgb,
};
use player_core::{AudioState, PlaybackDevice, PlaybackStatus, Snapshot};

use crate::{ACCENT, MUTED, Performance, border, clock, small_button, tone};

pub(crate) enum NowPlayingEvent {
    ViewList,
    Seek(u64),
}

impl EventEmitter<NowPlayingEvent> for NowPlaying {}

/// Presentation state for the persistent now-playing bar.
///
/// The presentation emits an intent for the parent when “View list” is
/// clicked; playback state and timing remain owned by `player-core` and the
/// runtime.
pub(crate) struct NowPlaying {
    playback: Option<PlaybackStatus>,
    audio_ready: bool,
    visible: bool,
    has_playing_list: bool,
    performance: Arc<Performance>,
}

impl NowPlaying {
    pub(crate) fn new(snapshot: &Snapshot, performance: Arc<Performance>) -> Self {
        Self {
            playback: snapshot.playback.clone(),
            audio_ready: matches!(snapshot.audio, AudioState::Ready),
            visible: matches!(snapshot.login, player_core::LoginState::Ready),
            has_playing_list: snapshot.implicit_queue.is_some(),
            performance,
        }
    }

    pub(crate) fn update_snapshot(
        &mut self,
        playback: Option<PlaybackStatus>,
        audio_ready: bool,
        visible: bool,
        has_playing_list: bool,
        cx: &mut Context<Self>,
    ) {
        if self.playback == playback
            && self.audio_ready == audio_ready
            && self.visible == visible
            && self.has_playing_list == has_playing_list
        {
            return;
        }
        self.playback = playback;
        self.audio_ready = audio_ready;
        self.visible = visible;
        self.has_playing_list = has_playing_list;
        cx.notify();
    }

    fn active(&self) -> bool {
        should_animate(
            self.visible,
            self.audio_ready,
            self.playback.as_ref().map(|p| p.device),
            self.playback.as_ref().is_some_and(|p| p.is_playing),
        )
    }
}

/// Animation belongs to the mounted now-playing presentation only while the
/// engine reports ready Native Playback and the session is actively playing.
fn should_animate(
    visible: bool,
    audio_ready: bool,
    device: Option<PlaybackDevice>,
    playing: bool,
) -> bool {
    visible && audio_ready && device == Some(PlaybackDevice::Native) && playing
}

impl gpui::Render for NowPlaying {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active();
        let started = self.performance.enabled.then(Instant::now);
        if active {
            // This call is intentionally scoped to this entity. GPUI notifies
            // only the entity that requested the next frame, leaving the root,
            // catalog, sidebar, and queue presentation event-driven.
            window.request_animation_frame();
        }

        let (title_line, position_ms, duration_ms, progress) = match &self.playback {
            Some(p) => {
                let visible = player_core::project_position(p, Instant::now());
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

        let has_playing_list = self.has_playing_list;
        let element = div()
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
                            cx.listener(|_, _, _, cx| {
                                cx.emit(NowPlayingEvent::ViewList);
                                cx.stop_propagation();
                            }),
                        )))
                    }),
            )
            .cursor_pointer()
            .on_click(cx.listener(move |_, event: &ClickEvent, window, cx| {
                if duration_ms > 0 {
                    let fraction =
                        (event.position().x / window.viewport_size().width).clamp(0., 1.);
                    cx.emit(NowPlayingEvent::Seek(
                        (duration_ms as f32 * fraction) as u64,
                    ));
                }
            }));

        if let Some(started) = started {
            self.performance.render(started.elapsed(), active);
        }
        element
    }
}

#[cfg(test)]
mod tests {
    use super::should_animate;
    use player_core::PlaybackDevice;

    #[test]
    fn animation_requires_visible_native_playback() {
        assert!(should_animate(
            true,
            true,
            Some(PlaybackDevice::Native),
            true
        ));
        assert!(!should_animate(
            false,
            true,
            Some(PlaybackDevice::Native),
            true
        ));
        assert!(!should_animate(
            true,
            false,
            Some(PlaybackDevice::Native),
            true
        ));
        assert!(!should_animate(
            true,
            true,
            Some(PlaybackDevice::Native),
            false
        ));
        assert!(!should_animate(
            true,
            true,
            Some(PlaybackDevice::Remote),
            true
        ));
    }
}
