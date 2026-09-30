//! The now-playing presentation seam.
//!
//! This is the only GPUI view that projects the moving Playback Session
//! position. It owns no transport authority: the engine's source-neutral
//! `PlaybackStatus` remains the input, and controls send commands back to the
//! runtime through the parent application's command seam.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    ClickEvent, Context, EventEmitter, Hsla, IntoElement, ParentElement, SharedString, Styled,
    Task, Window, div, prelude::*, px, relative, rgb,
};
use player_core::{AudioState, LoginState, Playable, PlaybackDevice, PlaybackStatus, Snapshot};

use crate::{ACCENT, MUTED, Performance, border, clock, small_button, tone};

/// The shortest wait between progress updates, used while the window is focused.
const FOCUSED_PROGRESS_UPDATE_INTERVAL: Duration = Duration::from_millis(100);
/// The longest wait, and the only one in the background. It stays well under
/// a second so the clock label never skips one.
const BACKGROUND_PROGRESS_UPDATE_INTERVAL: Duration = Duration::from_millis(500);

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
    pending_playable: Option<Playable>,
    pending_transport: Option<SharedString>,
    audio_ready: bool,
    visible: bool,
    has_playing_list: bool,
    performance: Arc<Performance>,
    progress_task: Task<()>,
    /// The wait before the next progress update, refreshed by every render.
    progress_interval: Duration,
    title_line: SharedString,
    duration_ms: u64,
    clock_second: Option<u64>,
    clock_label: SharedString,
}

impl NowPlaying {
    pub(crate) fn new(
        snapshot: &Snapshot,
        performance: Arc<Performance>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (title_line, duration_ms) = playback_metadata(None);
        let mut now_playing = Self {
            playback: None,
            pending_playable: None,
            pending_transport: None,
            audio_ready: false,
            visible: false,
            has_playing_list: false,
            performance,
            progress_task: Task::ready(()),
            progress_interval: FOCUSED_PROGRESS_UPDATE_INTERVAL,
            title_line,
            duration_ms,
            clock_second: None,
            clock_label: SharedString::default(),
        };
        now_playing.update_snapshot(snapshot, cx);
        now_playing
    }

    pub(crate) fn update_snapshot(&mut self, snapshot: &Snapshot, cx: &mut Context<Self>) {
        let audio_ready = matches!(snapshot.audio, AudioState::Ready);
        let visible = matches!(snapshot.login, LoginState::Ready);
        let has_playing_list = snapshot.implicit_queue.is_some();
        if self.playback == snapshot.playback
            && self.audio_ready == audio_ready
            && self.visible == visible
            && self.has_playing_list == has_playing_list
        {
            return;
        }
        let was_active = self.active();
        if self.playback != snapshot.playback {
            self.playback = snapshot.playback.clone();
            (self.title_line, self.duration_ms) = playback_metadata(self.playback.as_ref());
            self.clock_second = None;
            self.clock_label = SharedString::default();
        }
        self.audio_ready = audio_ready;
        self.visible = visible;
        self.has_playing_list = has_playing_list;
        self.changed(was_active, cx);
    }

    pub(crate) fn update_pending(
        &mut self,
        pending_playable: Option<Playable>,
        pending_transport: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        if self.pending_playable == pending_playable && self.pending_transport == pending_transport
        {
            return;
        }
        let was_active = self.active();
        self.pending_playable = pending_playable;
        self.pending_transport = pending_transport;
        self.changed(was_active, cx);
    }

    /// Repaint after a state change, and start progress updates if the change
    /// made the bar move. A running task ends itself once the bar stops.
    fn changed(&mut self, was_active: bool, cx: &mut Context<Self>) {
        if self.active() && !was_active {
            self.progress_task = cx.spawn(async move |this, cx| {
                while let Ok(Some(interval)) = this.update(cx, |now_playing, cx| {
                    now_playing.active().then(|| {
                        cx.notify();
                        now_playing.progress_interval
                    })
                }) {
                    cx.background_executor().timer(interval).await;
                }
            });
        }
        cx.notify();
    }

    fn has_pending(&self) -> bool {
        self.pending_playable.is_some() || self.pending_transport.is_some()
    }

    fn active(&self) -> bool {
        !self.has_pending()
            && should_animate(
                self.visible,
                self.audio_ready,
                self.playback.as_ref().map(|p| p.device),
                self.playback.as_ref().is_some_and(|p| p.is_playing),
            )
    }
}

fn playback_metadata(playback: Option<&PlaybackStatus>) -> (SharedString, u64) {
    match playback {
        Some(playback) => (
            SharedString::from(format!(
                "{} — {}{}",
                playback.playable.title,
                playback.playable.artists_display(),
                if playback.is_playing { "" } else { " ⏸" }
            )),
            playback.playable.duration_ms,
        ),
        None => (SharedString::new_static("Nothing playing"), 0),
    }
}

fn pending_metadata(
    pending_playable: Option<&Playable>,
    pending_transport: Option<&SharedString>,
) -> Option<SharedString> {
    if let Some(playable) = pending_playable {
        return Some(SharedString::from(format!(
            "Starting · {} — {}",
            playable.title,
            playable.artists_display()
        )));
    }
    pending_transport.cloned()
}

/// Progress updates belong to the mounted now-playing presentation only while
/// the engine reports ready Native Playback and the session is actively playing.
fn should_animate(
    visible: bool,
    audio_ready: bool,
    device: Option<PlaybackDevice>,
    playing: bool,
) -> bool {
    visible && audio_ready && device == Some(PlaybackDevice::Native) && playing
}

/// The wait until the bar can look different. GPUI snaps the bar to device
/// pixels, so updating before the track has moved by one pixel's worth of
/// time would repaint an identical frame.
fn progress_update_interval(duration_ms: u64, device_width: f32, window_active: bool) -> Duration {
    let shortest = if window_active {
        FOCUSED_PROGRESS_UPDATE_INTERVAL
    } else {
        BACKGROUND_PROGRESS_UPDATE_INTERVAL
    };
    Duration::from_millis(duration_ms)
        .div_f32(device_width.max(1.))
        .clamp(shortest, BACKGROUND_PROGRESS_UPDATE_INTERVAL)
}

impl gpui::Render for NowPlaying {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let started = self.performance.enabled.then(Instant::now);
        let active = self.active();
        let has_pending = self.has_pending();
        let position_ms = match &self.playback {
            Some(p) if !has_pending => player_core::project_position(p, Instant::now()),
            _ => 0,
        };
        let duration_ms = if has_pending { 0 } else { self.duration_ms };
        let progress = if duration_ms > 0 {
            (position_ms as f32 / duration_ms as f32).clamp(0., 1.)
        } else {
            0.
        };
        self.progress_interval = progress_update_interval(
            duration_ms,
            f32::from(window.viewport_size().width) * window.scale_factor(),
            window.is_window_active(),
        );
        let clock_label = if duration_ms > 0 {
            let second = position_ms / 1000;
            if self.clock_second != Some(second) {
                self.clock_second = Some(second);
                self.clock_label =
                    SharedString::from(format!("{} / {}", clock(position_ms), clock(duration_ms)));
            }
            self.clock_label.clone()
        } else {
            self.clock_second = None;
            self.clock_label = SharedString::default();
            SharedString::default()
        };
        let title_line = pending_metadata(
            self.pending_playable.as_ref(),
            self.pending_transport.as_ref(),
        )
        .unwrap_or_else(|| self.title_line.clone());

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
                            .child(clock_label),
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
    use super::*;

    #[test]
    fn progress_updates_wait_for_the_next_device_pixel_within_bounds() {
        let interval = |duration_s: u64, width, active| {
            progress_update_interval(duration_s * 1000, width, active).as_millis()
        };
        // A four-minute track on a 960 px bar moves one pixel every 250 ms.
        assert_eq!(interval(240, 960., true), 250);
        // Wide bars and short tracks never update faster than the focused rate.
        assert_eq!(interval(240, 3840., true), 100);
        // Long tracks still update often enough for the clock label.
        assert_eq!(interval(3600, 960., true), 500);
        assert_eq!(interval(240, 3840., false), 500);
        // Nothing playing and an unmeasured window are not special cases.
        assert_eq!(interval(0, 0., true), 100);
    }
}
