//! The adapter that maps the source-neutral contract onto the Spotatui
//! fork's `frontend` module. Playback ordering remains engine-owned; this
//! adapter carries source-neutral implicit-list metadata and maps commands to
//! fork actions. The application crate never imports the fork.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::watch;

use player_core::{
    AudioState, CatalogRevision, Command, LibraryEntry, LibrarySection, LibraryState, LoginState,
    Notice, Playable, PlaybackList, PlaybackListProjector, PlaybackStatus, Runtime, SearchAlbum,
    SearchArtist, SearchDetail, SearchPlaylist, SearchResults, SearchState, SearchTarget, Snapshot,
    Source,
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
    /// Catalog intent, projection, revisions, and direct publication policy.
    catalog: Arc<Mutex<CatalogProjection>>,
    /// The source list that should resume after the explicit queue drains.
    implicit_queue: Arc<Mutex<Option<PlaybackList>>>,
    /// Delivers the manual redirect-URL paste to the blocked boot prompt.
    paste_url_tx: mpsc::Sender<String>,
    /// Wakes the boot thread for another attempt after a failed boot.
    retry_boot_tx: mpsc::Sender<()>,
}

/// Keeps catalog policy together while the engine owns Playback Session state.
#[derive(Default)]
struct CatalogProjection {
    last_query: Option<String>,
    requested_library: Option<LibrarySection>,
    revisions: RevisionTracker,
}

impl CatalogProjection {
    fn search(&mut self, query: String) {
        self.last_query = Some(query);
    }

    fn relay(&mut self, fork: &frontend::Snapshot) -> Snapshot {
        let mut snapshot = map_snapshot(fork, self.last_query.as_deref());
        snapshot.library = self
            .requested_library
            .map(|section| map_library(fork, section))
            .unwrap_or_default();
        self.revise(&mut snapshot);
        snapshot
    }

    fn begin_library(&mut self, current: Snapshot, section: LibrarySection) -> Snapshot {
        self.requested_library = Some(section);
        self.with_library(current, LibraryState::Loading { section })
    }

    fn cached_library(
        &mut self,
        current: Snapshot,
        fork: &frontend::Snapshot,
        section: LibrarySection,
    ) -> Snapshot {
        self.with_library(current, map_library(fork, section))
    }

    fn with_library(&mut self, current: Snapshot, library: LibraryState) -> Snapshot {
        let mut next = Snapshot { library, ..current };
        self.revise(&mut next);
        next
    }

    fn revise(&mut self, snapshot: &mut Snapshot) {
        self.revisions.revise(snapshot);
    }
}

#[derive(Default)]
struct RevisionTracker {
    next: u64,
    search: Option<(String, SearchResults, CatalogRevision)>,
    detail: Option<(SearchDetail, CatalogRevision)>,
    library: Option<(LibrarySection, Vec<LibraryEntry>, CatalogRevision)>,
}

impl RevisionTracker {
    fn next(&mut self) -> CatalogRevision {
        self.next += 1;
        CatalogRevision::new(self.next)
    }

    fn revise(&mut self, snapshot: &mut Snapshot) {
        match &mut snapshot.search {
            SearchState::Done {
                query,
                revision,
                results,
            } => {
                let value = self
                    .search
                    .as_ref()
                    .and_then(|(old_query, old_results, revision)| {
                        (old_query == query && old_results == results).then_some(*revision)
                    })
                    .unwrap_or_else(|| self.next());
                *revision = value;
                self.search = Some((query.clone(), results.clone(), value));
            }
            _ => self.search = None,
        }
        match &mut snapshot.search_detail {
            Some(detail) => {
                let raw = detail.clone();
                let value = self
                    .detail
                    .as_ref()
                    .and_then(|(old, revision)| (old == &raw).then_some(*revision))
                    .unwrap_or_else(|| self.next());
                set_detail_revision(detail, value);
                self.detail = Some((raw, value));
            }
            None => self.detail = None,
        }
        match &mut snapshot.library {
            LibraryState::Done {
                section,
                revision,
                entries,
            } => {
                let value = self
                    .library
                    .as_ref()
                    .and_then(|(old_section, old_entries, revision)| {
                        (old_section == section && old_entries == entries).then_some(*revision)
                    })
                    .unwrap_or_else(|| self.next());
                *revision = value;
                self.library = Some((*section, entries.clone(), value));
            }
            _ => self.library = None,
        }
    }
}

fn set_detail_revision(detail: &mut SearchDetail, revision: CatalogRevision) {
    match detail {
        SearchDetail::Artist {
            revision: current, ..
        }
        | SearchDetail::Album {
            revision: current, ..
        }
        | SearchDetail::Playlist {
            revision: current, ..
        } => *current = revision,
    }
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
        catalog: Arc::default(),
        implicit_queue: Arc::default(),
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
        let catalog = Arc::clone(&self.catalog);
        let implicit_queue = Arc::clone(&self.implicit_queue);
        runtime.handle().spawn(async move {
            loop {
                let fork_snapshot = rx.borrow_and_update().clone();
                let mut snapshot = catalog.lock().unwrap().relay(&fork_snapshot);
                snapshot.implicit_queue = map_implicit_queue(
                    implicit_queue.lock().unwrap().clone(),
                    snapshot.playback.as_ref(),
                );
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
                self.catalog.lock().unwrap().search(query.clone());
                self.with_frontend(|runtime| runtime.apply(EngineAction::SearchActiveSource(query)))
                    .is_some()
            }
            Command::OpenSearchTarget(target) => self
                .with_frontend(|runtime| {
                    runtime.apply(EngineAction::Open(match target {
                        SearchTarget::Artist { locator, name } => {
                            frontend::OpenTarget::Artist { id: locator, name }
                        }
                        SearchTarget::Album { locator, .. } => frontend::OpenTarget::Album(locator),
                        SearchTarget::Playlist {
                            locator,
                            from_search,
                            ..
                        } => frontend::OpenTarget::Playlist {
                            id: locator,
                            from_search,
                        },
                    }))
                })
                .is_some(),
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
                let current = self.tx.borrow().clone();
                publish(
                    &self.tx,
                    self.catalog.lock().unwrap().begin_library(current, section),
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
                            self.catalog
                                .lock()
                                .unwrap()
                                .cached_library(current, &fork, section),
                        );
                    }
                }
                true
            }
            Command::Play(playable) => {
                let Some(()) = self.with_frontend(|_| ()) else {
                    return false;
                };
                *self.implicit_queue.lock().unwrap() = None;
                self.with_frontend(|runtime| dispatch_command(runtime, Command::Play(playable)))
                    .is_some()
            }
            Command::PlayFromList { list, index } => {
                let mut list = (*list).clone();
                if list.tracks.is_empty() || index >= list.tracks.len() {
                    return false;
                }
                let Some(()) = self.with_frontend(|_| ()) else {
                    return false;
                };
                list.current_index = index;
                *self.implicit_queue.lock().unwrap() = Some(list.clone());
                self.with_frontend(|runtime| {
                    dispatch_command(
                        runtime,
                        Command::PlayFromList {
                            list: Arc::new(list),
                            index,
                        },
                    )
                })
                .is_some()
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
    let action = action_for_command(command);
    let _outcome = runtime.apply(action);
}

fn action_for_command(command: Command) -> EngineAction {
    match command {
        Command::Play(playable) => EngineAction::PlayUris {
            uris: vec![playable.locator],
            offset: None,
        },
        Command::PlayFromList { list, index } => EngineAction::PlayUris {
            uris: list
                .tracks
                .iter()
                .map(|playable| playable.locator.clone())
                .collect(),
            offset: Some(index),
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
        | Command::OpenSearchTarget(_)
        | Command::SubmitPastedLoginUrl(_)
        | Command::Reauthenticate
        | Command::Browse(_) => {
            unreachable!("handled in `command`")
        }
    }
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

    fn playable(locator: &str) -> Playable {
        Playable {
            source: Source::Spotify,
            locator: locator.to_string(),
            title: locator.to_string(),
            artists: vec!["Artist".to_string()],
            album: "Album".to_string(),
            duration_ms: 1_000,
        }
    }

    #[test]
    fn list_playback_submits_the_full_list_and_selected_offset() {
        let action = action_for_command(Command::PlayFromList {
            list: Arc::new(PlaybackList {
                source: player_core::PlaybackListSource::Album {
                    locator: "spotify:album:album".to_string(),
                    name: "Album".to_string(),
                },
                tracks: vec![
                    playable("spotify:track:first"),
                    playable("spotify:track:second"),
                ]
                .into(),
                current_index: 1,
            }),
            index: 1,
        });
        assert_eq!(
            action,
            EngineAction::PlayUris {
                uris: vec![
                    "spotify:track:first".to_string(),
                    "spotify:track:second".to_string(),
                ],
                offset: Some(1),
            }
        );
    }

    #[test]
    fn implicit_list_cursor_follows_the_playback_snapshot() {
        let list = PlaybackList {
            source: player_core::PlaybackListSource::SearchResults {
                query: "query".to_string(),
            },
            tracks: vec![
                playable("spotify:track:first"),
                playable("spotify:track:second"),
            ]
            .into(),
            current_index: 0,
        };
        let playback = PlaybackStatus {
            playable: playable("spotify:track:second"),
            is_playing: true,
            position_ms: 0,
            observed_at: std::time::Instant::now(),
            volume_percent: None,
        };
        assert_eq!(
            map_implicit_queue(Some(list), Some(&playback))
                .unwrap()
                .current_index,
            1
        );
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
    fn accepted_search_detail_and_library_listings_receive_revisions() {
        let mut catalog = CatalogProjection::default();
        let mut snapshot = map_snapshot(&idle_with_results(), Some("coltrane"));
        catalog.revise(&mut snapshot);
        let first_search_revision = match &snapshot.search {
            SearchState::Done { revision, .. } => *revision,
            _ => unreachable!(),
        };

        snapshot = map_snapshot(
            &frontend::Snapshot {
                search_loading: true,
                ..Default::default()
            },
            Some("coltrane"),
        );
        catalog.revise(&mut snapshot);
        assert!(matches!(snapshot.search, SearchState::Loading { .. }));

        snapshot = map_snapshot(&idle_with_results(), Some("coltrane"));
        catalog.revise(&mut snapshot);
        let refreshed_search_revision = match &snapshot.search {
            SearchState::Done { revision, .. } => *revision,
            _ => unreachable!(),
        };
        assert!(first_search_revision < refreshed_search_revision);

        let detail_fork = frontend::Snapshot {
            search_detail: Some(frontend::SearchDetail::Album {
                tracks: vec![track("detail")],
            }),
            ..Default::default()
        };
        snapshot = map_snapshot(&detail_fork, None);
        catalog.revise(&mut snapshot);
        let first_detail_revision = match &snapshot.search_detail {
            Some(SearchDetail::Album { revision, .. }) => *revision,
            _ => unreachable!(),
        };

        snapshot = map_snapshot(&frontend::Snapshot::default(), None);
        catalog.revise(&mut snapshot);
        assert!(snapshot.search_detail.is_none());

        snapshot = map_snapshot(&detail_fork, None);
        catalog.revise(&mut snapshot);
        let refreshed_detail_revision = match &snapshot.search_detail {
            Some(SearchDetail::Album { revision, .. }) => *revision,
            _ => unreachable!(),
        };
        assert!(first_detail_revision < refreshed_detail_revision);
    }

    #[test]
    fn maps_all_search_categories() {
        let mut fork = idle_with_results();
        fork.search_artists = vec![frontend::ArtistInfo {
            name: "Artist".to_string(),
            ..Default::default()
        }];
        fork.search_albums = vec![frontend::AlbumInfo {
            name: "Album".to_string(),
            artists: vec![frontend::ArtistRef {
                name: "Artist".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        }];
        fork.search_playlists = vec![frontend::PlaylistInfo {
            uri: "spotify:playlist:mix".to_string(),
            name: "Mix".to_string(),
            owner: "me".to_string(),
            track_count: 3,
            id: None,
            owner_id: None,
            collaborative: false,
            public: None,
            image_url: None,
        }];

        let Snapshot {
            search: SearchState::Done { results, .. },
            ..
        } = map_snapshot(&fork, Some("query"))
        else {
            panic!("expected completed search");
        };
        assert_eq!(results.tracks.len(), 2);
        assert_eq!(results.artists[0].name, "Artist");
        assert_eq!(results.albums[0].artists, ["Artist"]);
        assert_eq!(results.playlists[0].name, "Mix");
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
                entries,
                ..
            } if matches!(entries.as_slice(), [LibraryEntry::Track { playable, played_at_ms: None }]
                if playable.locator == "spotify:track:liked")
        ));

        let playlists = map_library(&fork, LibrarySection::Playlists);
        assert!(matches!(
            playlists,
            LibraryState::Done {
                section: LibrarySection::Playlists,
                entries,
                ..
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

        assert!(matches!(
            map_library(
                &frontend::Snapshot {
                    notice: Some("offline".to_string()),
                    notice_is_error: true,
                    ..Default::default()
                },
                LibrarySection::RecentlyPlayed
            ),
            LibraryState::Failed { section: LibrarySection::RecentlyPlayed, message }
                if message == "offline"
        ));
    }

    #[test]
    fn library_refresh_revises_catalog_without_replacing_active_playback_list() {
        let active = PlaybackList {
            source: player_core::PlaybackListSource::LikedSongs,
            tracks: vec![playable("spotify:track:playing")].into(),
            current_index: 0,
        };
        let current = Snapshot {
            implicit_queue: Some(active.clone()),
            ..Snapshot::default()
        };
        let mut catalog = CatalogProjection::default();
        let done = LibraryState::Done {
            section: LibrarySection::LikedSongs,
            revision: CatalogRevision::new(0),
            entries: vec![LibraryEntry::Track {
                playable: playable("spotify:track:catalog"),
                played_at_ms: None,
            }],
        };

        let first = catalog.with_library(current.clone(), done.clone());
        let first_revision = match first.library {
            LibraryState::Done { revision, .. } => revision,
            _ => unreachable!(),
        };
        assert_eq!(first.implicit_queue.as_ref(), Some(&active));

        let loading = catalog.with_library(
            current.clone(),
            LibraryState::Loading {
                section: LibrarySection::LikedSongs,
            },
        );
        assert!(matches!(loading.library, LibraryState::Loading { .. }));
        assert_eq!(loading.implicit_queue.as_ref(), Some(&active));

        let refreshed = catalog.with_library(current.clone(), done);
        assert!(matches!(
            refreshed.library,
            LibraryState::Done { revision, .. } if revision > first_revision
        ));
        assert_eq!(refreshed.implicit_queue.as_ref(), Some(&active));

        let failed = catalog.with_library(
            current,
            LibraryState::Failed {
                section: LibrarySection::LikedSongs,
                message: "offline".to_string(),
            },
        );
        assert!(matches!(failed.library, LibraryState::Failed { .. }));
        assert_eq!(failed.implicit_queue.as_ref(), Some(&active));
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
    if fork.notice_is_error
        && let Some(message) = fork
            .notice
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
    {
        return LibraryState::Failed {
            section,
            message: message.to_string(),
        };
    }
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
        Some(entries) => LibraryState::Done {
            section,
            revision: CatalogRevision::new(0),
            entries,
        },
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
    } else if !fork.search_tracks.is_empty()
        || !fork.search_artists.is_empty()
        || !fork.search_albums.is_empty()
        || !fork.search_playlists.is_empty()
    {
        SearchState::Done {
            query,
            revision: CatalogRevision::new(0),
            results: SearchResults {
                tracks: fork
                    .search_tracks
                    .iter()
                    .filter_map(playable_from_track)
                    .collect(),
                artists: fork
                    .search_artists
                    .iter()
                    .map(|artist| SearchArtist {
                        locator: artist
                            .uri
                            .clone()
                            .or_else(|| artist.id.clone())
                            .unwrap_or_default(),
                        name: artist.name.clone(),
                    })
                    .collect(),
                albums: fork
                    .search_albums
                    .iter()
                    .map(|album| SearchAlbum {
                        locator: album
                            .uri
                            .clone()
                            .or_else(|| album.id.clone())
                            .unwrap_or_default(),
                        name: album.name.clone(),
                        artists: album
                            .artists
                            .iter()
                            .map(|artist| artist.name.clone())
                            .collect(),
                    })
                    .collect(),
                playlists: fork
                    .search_playlists
                    .iter()
                    .map(|playlist| SearchPlaylist {
                        locator: playlist.uri.clone(),
                        name: playlist.name.clone(),
                        owner: playlist.owner.clone(),
                        track_count: playlist.track_count,
                    })
                    .collect(),
            },
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

    let search_detail = fork.search_detail.as_ref().map(|detail| match detail {
        frontend::SearchDetail::Artist { tracks, albums } => SearchDetail::Artist {
            revision: CatalogRevision::new(0),
            tracks: tracks.iter().filter_map(playable_from_track).collect(),
            albums: albums
                .iter()
                .map(|album| SearchAlbum {
                    locator: album
                        .uri
                        .clone()
                        .or_else(|| album.id.clone())
                        .unwrap_or_default(),
                    name: album.name.clone(),
                    artists: album
                        .artists
                        .iter()
                        .map(|artist| artist.name.clone())
                        .collect(),
                })
                .collect(),
        },
        frontend::SearchDetail::Album { tracks } => SearchDetail::Album {
            revision: CatalogRevision::new(0),
            tracks: tracks.iter().filter_map(playable_from_track).collect(),
        },
        frontend::SearchDetail::Playlist { tracks } => SearchDetail::Playlist {
            revision: CatalogRevision::new(0),
            tracks: tracks.iter().filter_map(playable_from_track).collect(),
        },
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
        search_detail,
        playback,
        queue,
        implicit_queue: None,
        library: LibraryState::Idle,
        audio,
        notice: notice.map(|message| Notice {
            message: message.to_string(),
            dismissible: true,
        }),
    }
}

fn map_implicit_queue(
    mut list: Option<PlaybackList>,
    playback: Option<&PlaybackStatus>,
) -> Option<PlaybackList> {
    if let (Some(list), Some(playback)) = (list.as_mut(), playback) {
        PlaybackListProjector::align_cursor(list, &playback.playable);
    }
    list
}
