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

use std::sync::Arc;
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

/// A catalog artist result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchArtist {
    pub locator: String,
    pub name: String,
}

/// A catalog album result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchAlbum {
    pub locator: String,
    pub name: String,
    pub artists: Vec<String>,
}

/// A catalog playlist result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPlaylist {
    pub locator: String,
    pub name: String,
    pub owner: String,
    pub track_count: u32,
}

/// Catalog results grouped by Spotify content type.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchResults {
    pub tracks: Vec<Playable>,
    pub artists: Vec<SearchArtist>,
    pub albums: Vec<SearchAlbum>,
    pub playlists: Vec<SearchPlaylist>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchTarget {
    Artist {
        locator: String,
        name: String,
    },
    Album {
        locator: String,
        name: String,
    },
    Playlist {
        locator: String,
        name: String,
        /// Whether the target came from search results or the user's library.
        from_search: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchDetail {
    Artist {
        tracks: Vec<Playable>,
        albums: Vec<SearchAlbum>,
    },
    Album {
        tracks: Vec<Playable>,
    },
    Playlist {
        tracks: Vec<Playable>,
    },
}

/// The user-visible source of the currently implicit playback list.
///
/// This is intentionally separate from the explicit queue: browsing or
/// selecting a new list replaces this value, while enqueue operations do not.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PlaybackListSource {
    LikedSongs,
    RecentlyPlayed,
    SearchResults { query: String },
    Artist { locator: String, name: String },
    Album { locator: String, name: String },
    Playlist { locator: String, name: String },
}

impl PlaybackListSource {
    /// A concise label suitable for the playing-list header.
    pub fn label(&self) -> String {
        match self {
            Self::LikedSongs => "Liked songs".to_string(),
            Self::RecentlyPlayed => "Recently played".to_string(),
            Self::SearchResults { query } if query.is_empty() => "Search results".to_string(),
            Self::SearchResults { query } => format!("Search results: {query}"),
            Self::Artist { name, .. } => format!("Artist: {name}"),
            Self::Album { name, .. } => format!("Album: {name}"),
            Self::Playlist { name, .. } => format!("Playlist: {name}"),
        }
    }
}

/// The ordered list that playback follows after the explicit queue drains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackList {
    pub source: PlaybackListSource,
    pub tracks: Arc<[Playable]>,
    /// Index of the track currently selected in `tracks`.
    pub current_index: usize,
}

impl PlaybackList {
    /// Return the currently selected track, if the list is valid and non-empty.
    pub fn current(&self) -> Option<&Playable> {
        self.tracks.get(self.current_index)
    }
}

/// A library section the navigation sidebar can browse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LibrarySection {
    LikedSongs,
    RecentlyPlayed,
    Playlists,
}

impl LibrarySection {
    /// Sidebar label.
    pub fn label(&self) -> &'static str {
        match self {
            LibrarySection::LikedSongs => "Liked songs",
            LibrarySection::RecentlyPlayed => "Recently played",
            LibrarySection::Playlists => "Playlists",
        }
    }
}

/// One row of a library listing.
#[derive(Debug, Clone, PartialEq)]
pub enum LibraryEntry {
    Track {
        playable: Playable,
        /// When the track was played (Unix millis); `Some` only where the
        /// section has a time dimension (Recently played).
        played_at_ms: Option<u64>,
    },
    Playlist {
        /// Opaque source-owned playlist identity.
        id: String,
        name: String,
        track_count: u32,
    },
}

/// Catalog lifecycle for the requested library section, mirroring
/// [`SearchState`]: the visible listing is always replaced by `Loading` or
/// `Failed` before new rows land, so nothing on screen pretends to be
/// current. One section is browsed at a time (ADR 0011).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum LibraryState {
    #[default]
    Idle,
    Loading {
        section: LibrarySection,
    },
    Done {
        section: LibrarySection,
        entries: Vec<LibraryEntry>,
    },
    Failed {
        section: LibrarySection,
        message: String,
    },
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
        results: SearchResults,
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

/// One immutable runtime state containing the fields every version-one user
/// story needs. Catalog Availability shows up as `SearchState::Failed` plus
/// `notice`; Playback Health as `audio`.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub login: LoginState,
    pub search: SearchState,
    pub search_detail: Option<SearchDetail>,
    pub playback: Option<PlaybackStatus>,
    /// Upcoming Playables in order.
    pub queue: Vec<Playable>,
    /// The list that supplies playback after `queue` is empty.
    pub implicit_queue: Option<PlaybackList>,
    /// Listing for the section the sidebar last asked to browse.
    pub library: LibraryState,
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
            search_detail: None,
            playback: None,
            queue: Vec::new(),
            implicit_queue: None,
            library: LibraryState::Idle,
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
    OpenSearchTarget(SearchTarget),
    /// Show this sidebar section's listing; the answer arrives as
    /// `Snapshot::library`.
    Browse(LibrarySection),
    Play(Playable),
    /// Start `list` at `index`; this replaces the previous implicit list.
    PlayFromList {
        list: Arc<PlaybackList>,
        index: usize,
    },
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

    /// Stop playback and flush state cleanly. Blocks until done. Callable
    /// through a shared handle (the application's quit hook).
    fn shutdown(&self);
}
