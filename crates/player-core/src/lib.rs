//! The source-neutral contract between the UI and a playback runtime.
//!
//! Everything the version-one window renders or sends lives here: one
//! snapshot value type the runtime publishes, one command vocabulary it
//! sends, and a runtime trait shared by the real Spotatui adapter and a
//! scripted fake. Commands are accepted or rejected synchronously; they
//! never report completion — failures arrive later as notices in a
//! published snapshot.
//!
//! This crate depends on nothing heavier than `tokio::sync`.

pub mod fake;

use std::time::Instant;
use tokio::sync::watch;

/// A Music Source. One variant in version one: with one source every
/// capability is always present, so there is no capability vocabulary yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Spotify,
}

/// A piece of source-owned content resolved far enough that a Playback
/// Session can queue or start it. Identity is `(source, uri)`; display
/// metadata never defines it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Playable {
    pub source: Source,
    /// Opaque source-owned locator (`spotify:track:…` in version one).
    pub locator: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: String,
    pub duration_ms: u64,
}

impl Playable {
    /// "Artist A, Artist B" for result rows.
    pub fn artists_display(&self) -> String {
        self.artists.join(", ")
    }
}

/// Sign-in state. `InProgress` covers both browser-consent steps and the
/// paste-the-redirect-URL fallback; `Expired` offers Reauthenticate.
#[derive(Debug, Clone, PartialEq)]
pub enum LoginState {
    InProgress {
        message: String,
        wants_pasted_url: bool,
    },
    Ready,
    Expired {
        message: String,
    },
}

/// Catalog search lifecycle. Stale results are always replaced visibly by
/// `Loading` or `Failed`; nothing stays on screen as if current.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchState {
    Idle,
    Loading {
        query: String,
    },
    Done {
        query: String,
        results: Vec<Playable>,
    },
    Failed {
        query: String,
        message: String,
    },
}

/// The active Playable and where it stands. `position_ms` paired with
/// `observed_at` is authoritative at publish time; while playing, the UI
/// projects locally between snapshots so a 250 ms tick is smooth on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackStatus {
    pub playable: Playable,
    pub is_playing: bool,
    pub position_ms: u64,
    pub observed_at: Instant,
    pub volume_percent: Option<u8>,
}

/// Playback Health: whether audio can currently be produced. Independent of
/// Catalog Availability — an API failure never stops audio that keeps
/// playing.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioState {
    Ready,
    Starting,
    Unavailable { message: String },
}

/// The current error message, if any, with whether it can be dismissed.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub message: String,
    pub dismissible: bool,
}

/// One immutable runtime state: the six fields every version-one user story
/// needs. Catalog Availability shows up as `SearchState::Failed` plus
/// `notice`; Playback Health as `audio`.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub login: LoginState,
    pub search: SearchState,
    pub playback: Option<PlaybackStatus>,
    /// Upcoming Playables in order.
    pub queue: Vec<Playable>,
    pub audio: AudioState,
    pub notice: Option<Notice>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Snapshot {
            login: LoginState::InProgress {
                message: "Starting…".to_string(),
                wants_pasted_url: false,
            },
            search: SearchState::Idle,
            playback: None,
            queue: Vec::new(),
            audio: AudioState::Starting,
            notice: None,
        }
    }
}

impl Snapshot {
    /// The visible position right now, projected locally between snapshots.
    /// While playing this advances smoothly; each new snapshot snaps back to
    /// the authoritative `position_ms`.
    pub fn projected_position_ms(&self, now: Instant) -> Option<u64> {
        self.playback.as_ref().map(|p| project_position(p, now))
    }

    /// Whether the active transport should currently produce sound.
    pub fn is_playing(&self) -> bool {
        self.playback.as_ref().is_some_and(|p| p.is_playing)
    }
}

/// Project one playback status to `now`. Pure; unit-tested.
pub fn project_position(playback: &PlaybackStatus, now: Instant) -> u64 {
    if !playback.is_playing {
        return playback.position_ms;
    }
    let elapsed = now.saturating_duration_since(playback.observed_at);
    let projected = playback
        .position_ms
        .saturating_add(elapsed.as_millis() as u64);
    if playback.playable.duration_ms > 0 {
        projected.min(playback.playable.duration_ms)
    } else {
        projected
    }
}

/// Listener intent. Accepted or rejected synchronously by
/// [`Runtime::command`]; completion never reported.
#[derive(Debug, Clone)]
pub enum Command {
    /// Complete the sign-in fallback when the callback listener could not bind.
    SubmitPastedLoginUrl(String),
    Reauthenticate,
    Search(String),
    Play(Playable),
    Pause,
    Resume,
    /// Seek to an absolute position in milliseconds.
    Seek(u64),
    Next,
    Previous,
    SetVolume(u8),
    Enqueue(Playable),
    RemoveQueued(usize),
    MoveQueued {
        index: usize,
        up: bool,
    },
    ClearQueue,
    DismissNotice,
}

/// The runtime contract shared by the real Spotatui adapter and the fake.
pub trait Runtime: Send + Sync + 'static {
    /// Subscribe to immutable snapshots; the current state arrives immediately.
    fn subscribe(&self) -> watch::Receiver<Snapshot>;

    /// Send one command. Returns whether it was accepted; failures surface
    /// later as notices.
    fn command(&self, command: Command) -> bool;

    /// Stop playback and flush state cleanly. Blocks until done.
    fn shutdown(self);
}
