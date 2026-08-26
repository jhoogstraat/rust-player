//! The adapter that maps the source-neutral contract onto the Spotatui
//! fork's `frontend` module. It contains no playback logic: commands become
//! fork `Action`s, and contract snapshots are composed from the fork's
//! published snapshot. The application crate never imports the fork.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::watch;

use player_core::{
    AudioState, Command, LibraryEntry, LibrarySection, LibraryState, LoginState, Notice, Playable,
    PlaybackStatus, Runtime, SearchState, Snapshot, Source,
};
use spotatui::frontend::{self, ActionOutcome, EngineAction, LibraryTarget, Onboarding};

/// Boot inputs for the real runtime.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// Directory the fork derives its config, cache, and state directories
    /// from (`~/Library/Application Support/rust-player` on macOS).
    pub data_root: PathBuf,
}

impl ConnectOptions {
    pub fn new(data_root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: data_root.into(),
        }
    }
}

/// Sign-in conversation while `boot` runs on its blocking thread. `info`
/// texts become `login: InProgress`; the one reachable `prompt_line` is the
/// manual redirect-URL paste, which blocks until the window submits a URL.
/// The prompt can fire more than once per boot (once per client-id
/// candidate), so the receiver is kept, not consumed.
struct BootOnboarding {
    login_tx: watch::Sender<Snapshot>,
    pasted_url_rx: Mutex<mpsc::Receiver<String>>,
}

impl BootOnboarding {
    fn in_progress(&self, message: String, wants_pasted_url: bool) {
        let next = LoginState::InProgress {
            message,
            wants_pasted_url,
        };
        self.login_tx.send_if_modified(|current| {
            if current.login == next {
                false
            } else {
                current.login = next;
                true
            }
        });
    }
}

impl Onboarding for BootOnboarding {
    fn info(&self, text: &str) {
        self.in_progress(text.to_string(), false);
        log::info!("[onboarding] {text}");
    }

    fn progress(&self, text: &str) {
        log::info!("[onboarding] {text}");
    }

    fn prompt_line(&self, prompt: &str) -> anyhow::Result<String> {
        let rx = self.pasted_url_rx.lock().unwrap();
        // Anything pasted before this prompt opened answered nothing.
        while rx.try_recv().is_ok() {}
        self.in_progress(prompt.to_string(), true);
        let url = rx
            .recv()
            .map_err(|_| anyhow::anyhow!("sign-in cancelled"))?;
        self.in_progress("Finishing sign-in…".to_string(), false);
        Ok(format!("{url}\n"))
    }

    fn pick_sources(
        &self,
        _options: &[frontend::Source],
    ) -> anyhow::Result<Option<Vec<frontend::Source>>> {
        // Embedded boots never run the first-run picker.
        Ok(None)
    }
}

pub struct SpotatuiPlayer {
    tx: watch::Sender<Snapshot>,
    /// Present once boot succeeded; taken by `shutdown`.
    frontend: Mutex<Option<frontend::Runtime>>,
    /// The query the current search state belongs to; the fork's snapshot
    /// carries results but not the text they answer.
    last_query: Arc<Mutex<Option<String>>>,
    /// The library section currently requested by the sidebar. The fork's
    /// frontend snapshot carries the corresponding cached rows.
    requested_library: Arc<Mutex<Option<LibrarySection>>>,
    /// Delivers the manual redirect-URL paste to the blocked boot prompt.
    paste_url_tx: mpsc::Sender<String>,
    /// Wakes the boot thread for another attempt after a failed boot.
    retry_boot_tx: mpsc::Sender<()>,
}

/// Connect to the real runtime. Returns immediately; sign-in progress flows
/// through the returned channel while `boot` runs on a dedicated blocking
/// thread (its `Onboarding` is synchronous). A failed boot parks that thread
/// until `Command::Reauthenticate` asks for another attempt.
pub fn connect(options: ConnectOptions) -> Arc<SpotatuiPlayer> {
    let (tx, _rx) = watch::channel(Snapshot::default());
    let (paste_url_tx, paste_url_rx) = mpsc::channel::<String>();
    let (retry_boot_tx, retry_boot_rx) = mpsc::channel::<()>();
    let player = Arc::new(SpotatuiPlayer {
        tx: tx.clone(),
        frontend: Mutex::new(None),
        last_query: Arc::default(),
        requested_library: Arc::default(),
        paste_url_tx,
        retry_boot_tx,
    });

    let onboarding: Arc<dyn Onboarding> = Arc::new(BootOnboarding {
        login_tx: tx,
        pasted_url_rx: Mutex::new(paste_url_rx),
    });
    let player_for_boot = Arc::downgrade(&player);
    std::thread::Builder::new()
        .name("player-boot".into())
        .spawn(move || boot_loop(player_for_boot, options, onboarding, retry_boot_rx))
        .expect("spawn boot thread");

    player
}

/// Boot the fork; on failure, publish the error and wait for a retry.
fn boot_loop(
    player: Weak<SpotatuiPlayer>,
    options: ConnectOptions,
    onboarding: Arc<dyn Onboarding>,
    retry_rx: mpsc::Receiver<()>,
) {
    frontend::Runtime::install_panic_hook();
    loop {
        let Some(player) = player.upgrade() else {
            return;
        };
        publish(&player.tx, Snapshot::default());
        let outcome = frontend::Runtime::boot(
            frontend::Options::new(options.data_root.clone()),
            Arc::clone(&onboarding),
        );
        match outcome {
            Ok(runtime) => {
                player.stage(runtime);
                return;
            }
            Err(error) => {
                log::error!("[boot] runtime boot failed: {error:#}");
                publish(
                    &player.tx,
                    Snapshot {
                        login: LoginState::Expired {
                            message: format!("Could not start Spotify: {error:#}"),
                        },
                        audio: AudioState::Unavailable {
                            message: "Playback engine unavailable".to_string(),
                        },
                        ..Snapshot::default()
                    },
                );
            }
        }
        drop(player);
        if retry_rx.recv().is_err() {
            return;
        }
        // Clicks queued while this attempt ran asked for the same thing.
        while retry_rx.try_recv().is_ok() {}
    }
}

impl SpotatuiPlayer {
    /// Adopt a booted runtime: relay every fork snapshot into the contract
    /// on the runtime's own reactor, then make it reachable for commands.
    fn stage(&self, runtime: frontend::Runtime) {
        let mut rx = runtime.subscribe();
        let relay_tx = self.tx.clone();
        let last_query = Arc::clone(&self.last_query);
        let requested_library = Arc::clone(&self.requested_library);
        runtime.handle().spawn(async move {
            loop {
                let fork_snapshot = rx.borrow_and_update().clone();
                let query = last_query.lock().unwrap().clone();
                let mut snapshot = map_snapshot(&fork_snapshot, query.as_deref());
                snapshot.library = requested_library
                    .lock()
                    .unwrap()
                    .map(|section| map_library(&fork_snapshot, section))
                    .unwrap_or_default();
                publish(&relay_tx, snapshot);
                if rx.changed().await.is_err() {
                    break;
                }
            }
        });
        *self.frontend.lock().unwrap() = Some(runtime);
    }

    fn with_frontend<T>(&self, f: impl FnOnce(&frontend::Runtime) -> T) -> Option<T> {
        self.frontend.lock().unwrap().as_ref().map(f)
    }

    fn login(&self) -> LoginState {
        self.tx.borrow().login.clone()
    }
}

impl Runtime for SpotatuiPlayer {
    fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.tx.subscribe()
    }

    fn command(&self, command: Command) -> bool {
        match command {
            Command::SubmitPastedLoginUrl(url) => {
                let prompt_open = matches!(
                    self.login(),
                    LoginState::InProgress {
                        wants_pasted_url: true,
                        ..
                    }
                );
                prompt_open && self.paste_url_tx.send(url).is_ok()
            }
            Command::Search(query) => {
                *self.last_query.lock().unwrap() = Some(query.clone());
                self.with_frontend(|runtime| runtime.apply(EngineAction::SearchActiveSource(query)))
                    .is_some()
            }
            Command::Reauthenticate => {
                let staged = self
                    .with_frontend(|runtime| runtime.apply(EngineAction::BeginSpotifyLogin))
                    .is_some();
                // Before a successful boot the only re-login is another boot.
                staged
                    || (matches!(self.login(), LoginState::Expired { .. })
                        && self.retry_boot_tx.send(()).is_ok())
            }
            Command::Browse(section) => {
                // The playlist list is fetched during fork startup; liked
                // songs and recently played are fetched by these actions.
                let Some(()) = self.with_frontend(|_| ()) else {
                    return false;
                };
                *self.requested_library.lock().unwrap() = Some(section);
                let current = self.tx.borrow().clone();
                publish(
                    &self.tx,
                    Snapshot {
                        library: LibraryState::Loading { section },
                        ..current
                    },
                );

                self.with_frontend(|runtime| match section {
                    LibrarySection::LikedSongs => {
                        runtime.apply(EngineAction::OpenLibrary(LibraryTarget::LikedSongs))
                    }
                    LibrarySection::RecentlyPlayed => {
                        runtime.apply(EngineAction::OpenLibrary(LibraryTarget::RecentlyPlayed))
                    }
                    // No playlist LibraryTarget exists; startup already
                    // dispatches GetPlaylists and the snapshot relays it.
                    LibrarySection::Playlists => ActionOutcome::Applied,
                });

                // Playlists have no open-library action. Re-read the current
                // fork snapshot so an already-loaded startup cache is visible
                // immediately instead of waiting for an unrelated change.
                if section == LibrarySection::Playlists {
                    if let Some(fork) = self.with_frontend(|runtime| {
                        let rx = runtime.subscribe();
                        rx.borrow().clone()
                    }) {
                        let current = self.tx.borrow().clone();
                        publish(
                            &self.tx,
                            Snapshot {
                                library: map_library(&fork, section),
                                ..current
                            },
                        );
                    }
                }
                true
            }
            other => self
                .with_frontend(|runtime| dispatch_command(runtime, other))
                .is_some(),
        }
    }

    fn shutdown(&self) {
        let Some(runtime) = self.frontend.lock().unwrap().take() else {
            return;
        };
        if let Err(error) = runtime.shutdown() {
            log::warn!("[shutdown] runtime shutdown failed: {error:#}");
        }
    }
}

fn publish(tx: &watch::Sender<Snapshot>, next: Snapshot) {
    tx.send_if_modified(|current| {
        if *current == next {
            false
        } else {
            *current = next;
            true
        }
    });
}

fn dispatch_command(runtime: &frontend::Runtime, command: Command) {
    let action = match command {
        Command::Play(playable) => EngineAction::PlayUris {
            uris: vec![playable.locator],
            offset: None,
        },
        Command::Pause => EngineAction::Pause,
        Command::Resume => EngineAction::Play,
        Command::Seek(position_ms) => {
            EngineAction::SeekTo(u32::try_from(position_ms).unwrap_or(u32::MAX))
        }
        Command::Next => EngineAction::NextTrack,
        Command::Previous => EngineAction::PreviousTrack,
        Command::SetVolume(percent) => EngineAction::SetVolume(percent.min(100)),
        Command::Enqueue(playable) => EngineAction::EnqueueNative(track_info(&playable)),
        Command::RemoveQueued(index) => EngineAction::RemoveNativeQueued(index),
        Command::MoveQueued { index, up } => EngineAction::MoveNativeQueued { index, up },
        Command::ClearQueue => EngineAction::ClearNativeQueue,
        // The fork has no dedicated dismiss. An error notice blocks plain
        // notifications, so overwrite it as an *error* with an empty message
        // and the minimum TTL: `map_snapshot` drops empty notices at once and
        // the fork expires it on its next tick.
        Command::DismissNotice => EngineAction::NotifyError(String::new(), 0),
        Command::Search(_)
        | Command::SubmitPastedLoginUrl(_)
        | Command::Reauthenticate
        | Command::Browse(_) => {
            unreachable!("handled in `command`")
        }
    };
    let _outcome = runtime.apply(action);
}

fn track_info(playable: &Playable) -> frontend::TrackInfo {
    frontend::TrackInfo {
        uri: Some(playable.locator.clone()),
        name: playable.title.clone(),
        artists: playable.artists.clone(),
        album: playable.album.clone(),
        duration_ms: playable.duration_ms,
        id: None,
        album_id: None,
        artist_refs: Vec::new(),
        is_playable: true,
        is_local: false,
        track_number: 0,
        explicit: false,
        image_url: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(name: &str) -> frontend::TrackInfo {
        frontend::TrackInfo {
            uri: Some(format!("spotify:track:{name}")),
            name: name.to_string(),
            artists: vec![],
            album: String::new(),
            duration_ms: 1000,
            id: None,
            album_id: None,
            artist_refs: Vec::new(),
            is_playable: true,
            is_local: false,
            track_number: 0,
            explicit: false,
            image_url: None,
        }
    }

    /// Fork state while idle-with-results and playing.
    fn idle_with_results() -> frontend::Snapshot {
        frontend::Snapshot {
            search_tracks: vec![track("a"), track("b")],
            spotify_connected: true,
            audio_ready: true,
            ..Default::default()
        }
    }

    /// Regression: the fork's playback poll sets its *global* spinner every
    /// few seconds; only a real catalog-search event may flip visible
    /// results into `SearchState::Loading`.
    #[test]
    fn global_spinner_churn_does_not_reload_search_results() {
        let (tx, mut rx) = watch::channel(Snapshot::default());

        publish(&tx, map_snapshot(&idle_with_results(), Some("coltrane")));
        assert!(matches!(
            rx.borrow_and_update().search,
            SearchState::Done { .. }
        ));

        // Global spinner churn from an unrelated dispatch (playback poll):
        // old results stay published, only the global flag flips.
        let mut polling = idle_with_results();
        polling.search_loading = false;
        publish(&tx, map_snapshot(&polling, Some("coltrane")));
        let search = rx.borrow_and_update().search.clone();
        assert!(
            matches!(search, SearchState::Done { .. }),
            "spinner churn flipped visible results into {search:?}"
        );

        // A real catalog search raises the scoped flag.
        publish(
            &tx,
            map_snapshot(
                &frontend::Snapshot {
                    search_loading: true,
                    spotify_connected: true,
                    ..Default::default()
                },
                Some("coltrane"),
            ),
        );
        assert!(matches!(
            rx.borrow_and_update().search,
            SearchState::Loading { .. }
        ));
    }

    #[test]
    fn maps_fork_library_rows_and_keeps_unfetched_sections_loading() {
        let fork = frontend::Snapshot {
            library: frontend::LibrarySnapshot {
                liked_songs: Some(vec![track("liked")]),
                recently_played: Some(vec![track("recent")]),
                playlists: Some(vec![frontend::PlaylistInfo {
                    uri: "spotify:playlist:mix".to_string(),
                    name: "Mix".to_string(),
                    owner: "me".to_string(),
                    track_count: 3,
                    id: Some("mix".to_string()),
                    owner_id: None,
                    collaborative: false,
                    public: Some(false),
                    image_url: None,
                }]),
            },
            ..Default::default()
        };

        let liked = map_library(&fork, LibrarySection::LikedSongs);
        assert!(matches!(
            liked,
            LibraryState::Done {
                section: LibrarySection::LikedSongs,
                entries
            } if matches!(entries.as_slice(), [LibraryEntry::Track { playable, played_at_ms: None }]
                if playable.locator == "spotify:track:liked")
        ));

        let playlists = map_library(&fork, LibrarySection::Playlists);
        assert!(matches!(
            playlists,
            LibraryState::Done {
                section: LibrarySection::Playlists,
                entries
            } if matches!(entries.as_slice(), [LibraryEntry::Playlist { id, name, track_count }]
                if id == "mix" && name == "Mix" && *track_count == 3)
        ));

        assert!(matches!(
            map_library(
                &frontend::Snapshot::default(),
                LibrarySection::RecentlyPlayed
            ),
            LibraryState::Loading {
                section: LibrarySection::RecentlyPlayed
            }
        ));
    }
}

fn playable_from_track(track: &frontend::TrackInfo) -> Option<Playable> {
    Some(Playable {
        source: Source::Spotify,
        locator: track.uri.clone()?,
        title: track.name.clone(),
        artists: track.artists.clone(),
        album: track.album.clone(),
        duration_ms: track.duration_ms,
    })
}

fn library_track_entry(
    track: &frontend::TrackInfo,
    played_at_ms: Option<u64>,
) -> Option<LibraryEntry> {
    Some(LibraryEntry::Track {
        playable: playable_from_track(track)?,
        played_at_ms,
    })
}

fn map_library(fork: &frontend::Snapshot, section: LibrarySection) -> LibraryState {
    let entries = match section {
        LibrarySection::LikedSongs => fork.library.liked_songs.as_ref().map(|tracks| {
            tracks
                .iter()
                .filter_map(|track| library_track_entry(track, None))
                .collect::<Vec<_>>()
        }),
        LibrarySection::RecentlyPlayed => fork.library.recently_played.as_ref().map(|tracks| {
            tracks
                .iter()
                .filter_map(|track| library_track_entry(track, None))
                .collect::<Vec<_>>()
        }),
        LibrarySection::Playlists => fork.library.playlists.as_ref().map(|playlists| {
            playlists
                .iter()
                .map(|playlist| LibraryEntry::Playlist {
                    id: playlist.id.clone().unwrap_or_else(|| playlist.uri.clone()),
                    name: playlist.name.clone(),
                    track_count: playlist.track_count,
                })
                .collect::<Vec<_>>()
        }),
    };

    match entries {
        Some(entries) => LibraryState::Done { section, entries },
        None => LibraryState::Loading { section },
    }
}

fn map_snapshot(fork: &frontend::Snapshot, last_query: Option<&str>) -> Snapshot {
    // An empty notice is a dismissal in flight (see `dispatch_command`),
    // never a message — and never an error either.
    let notice = fork
        .notice
        .as_deref()
        .map(str::trim)
        .filter(|message| !message.is_empty());
    let error = notice.filter(|_| fork.notice_is_error);

    let login = if fork.spotify_connected {
        LoginState::Ready
    } else if let Some(message) = error {
        LoginState::Expired {
            message: message.to_string(),
        }
    } else {
        LoginState::InProgress {
            message: notice.unwrap_or("Connecting…").to_string(),
            wants_pasted_url: false,
        }
    };

    let query = last_query.unwrap_or_default().to_string();
    let search = if fork.search_loading {
        SearchState::Loading { query }
    } else if !fork.search_tracks.is_empty() {
        SearchState::Done {
            query,
            results: fork
                .search_tracks
                .iter()
                .filter_map(playable_from_track)
                .collect(),
        }
    } else if let (Some(query), Some(message)) = (last_query, error) {
        SearchState::Failed {
            query: query.to_string(),
            message: message.to_string(),
        }
    } else {
        SearchState::Idle
    };

    let playback = fork.playback.as_ref().and_then(|state| {
        Some(PlaybackStatus {
            playable: playable_from_track(state.track.as_ref()?)?,
            is_playing: state.is_playing,
            position_ms: fork.position_ms.unwrap_or(state.progress_ms),
            observed_at: fork.as_of,
            volume_percent: state.volume_percent,
        })
    });

    let queue = fork
        .queue_upcoming
        .iter()
        .filter_map(playable_from_track)
        .collect();

    let audio = if fork.audio_ready {
        AudioState::Ready
    } else if fork.audio_pending {
        AudioState::Starting
    } else {
        AudioState::Unavailable {
            message: notice
                .unwrap_or("Native audio unavailable. Browsing still works; restart to retry.")
                .to_string(),
        }
    };

    Snapshot {
        login,
        search,
        playback,
        queue,
        library: LibraryState::Idle,
        audio,
        notice: notice.map(|message| Notice {
            message: message.to_string(),
            dismissible: true,
        }),
    }
}
