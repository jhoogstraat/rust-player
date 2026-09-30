//! The adapter that maps the source-neutral contract onto the Spotatui
//! fork's `frontend` module. Playback ordering remains engine-owned; this
//! adapter maps source-neutral implicit-list metadata and commands to
//! fold-acknowledged fork actions. Pre-boot auth channels acknowledge on
//! hand-off because no fold exists yet. The application crate never imports
//! the fork.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::watch;

use player_core::{
    AudioState, CatalogRevision, Command, LibraryEntry, LibrarySection, LibraryState, LoginState,
    Playable, PlaybackDevice, PlaybackList, PlaybackStatus, Runtime, SearchAlbum, SearchArtist,
    SearchDetail, SearchPlaylist, SearchResults, SearchState, SearchTarget, Snapshot, Source,
};
use spotatui::frontend::{self, EngineAction, LibraryTarget, Onboarding};

static PERFORMANCE_ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
static SNAPSHOT_TRANSLATIONS: AtomicU64 = AtomicU64::new(0);
static LIBRARY_TRANSLATIONS: AtomicU64 = AtomicU64::new(0);

fn performance_enabled() -> bool {
    *PERFORMANCE_ENABLED
        .get_or_init(|| std::env::var_os("RUST_PLAYER_PERF").is_some_and(|value| value != "0"))
}

/// Translation counters for the opt-in performance baseline. These are
/// intentionally separate from the GPUI subscriber's catalog-change count.
pub fn performance_summary() -> String {
    format!(
        "adapter_snapshot_translations={} adapter_library_translations={}",
        SNAPSHOT_TRANSLATIONS.load(Ordering::Relaxed),
        LIBRARY_TRANSLATIONS.load(Ordering::Relaxed),
    )
}

/// Sign-in conversation while `boot` runs on its blocking thread. `info`
/// texts become `login: InProgress`; the one reachable `prompt_line` is the
/// manual redirect-URL paste, which blocks until the window submits a URL.
/// The prompt can fire more than once per boot (once per client-id
/// candidate), so the receiver is kept, not consumed.
struct BootOnboarding {
    login_tx: watch::Sender<Snapshot>,
    pasted_url_rx: Mutex<mpsc::Receiver<String>>,
    stop: Arc<AtomicBool>,
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
        if self.stop.load(Ordering::Acquire) {
            return Err(anyhow::anyhow!("sign-in cancelled"));
        }
        self.in_progress(prompt.to_string(), true);
        let url = rx
            .recv()
            .map_err(|_| anyhow::anyhow!("sign-in cancelled"))?;
        if self.stop.load(Ordering::Acquire) {
            return Err(anyhow::anyhow!("sign-in cancelled"));
        }
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
    /// Serializes command dispatch so command order matches fold order.
    command_lock: Mutex<()>,
    /// Present once boot succeeded; taken by `shutdown`. The terminal bit
    /// closes the hand-off race where shutdown wins before boot stages a
    /// runtime.
    frontend: Mutex<FrontendSlot<frontend::Runtime>>,
    /// Delivers the manual redirect-URL paste to the blocked boot prompt.
    paste_url_tx: mpsc::Sender<String>,
    /// Wakes the boot thread for another attempt after a failed boot.
    retry_boot_tx: mpsc::Sender<()>,
    /// Cancels a failed boot's retry wait and any blocking onboarding prompt.
    stop_boot: Arc<AtomicBool>,
}

struct FrontendSlot<T> {
    runtime: Option<T>,
    shutting_down: bool,
}

impl<T> Default for FrontendSlot<T> {
    fn default() -> Self {
        Self {
            runtime: None,
            shutting_down: false,
        }
    }
}

impl<T> FrontendSlot<T> {
    fn stage(&mut self, runtime: T) -> Result<(), T> {
        if self.shutting_down {
            Err(runtime)
        } else {
            self.runtime = Some(runtime);
            Ok(())
        }
    }

    fn begin_shutdown(&mut self) -> Option<T> {
        self.shutting_down = true;
        self.runtime.take()
    }
}

/// Connect to the real runtime, which keeps its config, cache, and state under
/// `data_root`. Returns immediately; sign-in progress flows
/// through the returned channel while `boot` runs on a dedicated blocking
/// thread (its `Onboarding` is synchronous). A failed boot parks that thread
/// until `Command::Reauthenticate` asks for another attempt.
pub fn connect(data_root: PathBuf) -> Arc<SpotatuiPlayer> {
    let (tx, _rx) = watch::channel(Snapshot::default());
    let (paste_url_tx, paste_url_rx) = mpsc::channel::<String>();
    let (retry_boot_tx, retry_boot_rx) = mpsc::channel::<()>();
    let stop_boot = Arc::new(AtomicBool::new(false));
    let player = Arc::new(SpotatuiPlayer {
        tx: tx.clone(),
        command_lock: Mutex::new(()),
        frontend: Mutex::new(FrontendSlot::default()),
        paste_url_tx,
        retry_boot_tx,
        stop_boot: Arc::clone(&stop_boot),
    });

    let onboarding: Arc<dyn Onboarding> = Arc::new(BootOnboarding {
        login_tx: tx,
        pasted_url_rx: Mutex::new(paste_url_rx),
        stop: Arc::clone(&stop_boot),
    });
    let player_for_boot = Arc::downgrade(&player);
    std::thread::Builder::new()
        .name("player-boot".into())
        .spawn(move || {
            boot_loop(
                player_for_boot,
                data_root,
                onboarding,
                retry_boot_rx,
                stop_boot,
            )
        })
        .expect("spawn boot thread");

    player
}

/// Boot the fork; on failure, publish the error and wait for a retry.
fn boot_loop(
    player: Weak<SpotatuiPlayer>,
    data_root: PathBuf,
    onboarding: Arc<dyn Onboarding>,
    retry_rx: mpsc::Receiver<()>,
    stop: Arc<AtomicBool>,
) {
    frontend::Runtime::install_panic_hook();
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        let Some(player) = player.upgrade() else {
            return;
        };
        publish(&player.tx, Snapshot::default());
        let outcome = frontend::Runtime::boot(
            frontend::Options::new(data_root.clone()),
            Arc::clone(&onboarding),
        );
        match outcome {
            Ok(runtime) => {
                player.stage(runtime);
                return;
            }
            Err(error) => {
                if stop.load(Ordering::Acquire) {
                    return;
                }
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
        if !wait_for_retry(&retry_rx, &stop) {
            return;
        }
        // Clicks queued while this attempt ran asked for the same thing.
        while retry_rx.try_recv().is_ok() {}
    }
}

fn wait_for_retry(retry_rx: &mpsc::Receiver<()>, stop: &AtomicBool) -> bool {
    if stop.load(Ordering::Acquire) {
        return false;
    }
    retry_rx
        .recv()
        .map(|()| !stop.load(Ordering::Acquire))
        .unwrap_or(false)
}

impl SpotatuiPlayer {
    /// Adopt a booted runtime: relay every fork snapshot into the contract
    /// on the runtime's own reactor, then make it reachable for commands.
    fn stage(&self, runtime: frontend::Runtime) {
        let mut rx = runtime.subscribe();
        let relay_tx = self.tx.clone();
        runtime.handle().spawn(async move {
            let mut cache = TranslationCache::default();
            loop {
                // Clone first: mapping under the borrow would hold up the fold.
                let engine_snapshot = rx.borrow_and_update().clone();
                let snapshot = map_snapshot(&engine_snapshot, &mut cache);
                publish(&relay_tx, snapshot);
                if rx.changed().await.is_err() {
                    break;
                }
            }
        });
        let rejected = self.frontend.lock().unwrap().stage(runtime);
        if let Err(runtime) = rejected
            && let Err(error) = runtime.shutdown()
        {
            log::warn!("[shutdown] late runtime shutdown failed: {error:#}");
        }
    }

    fn with_frontend<T>(&self, f: impl FnOnce(&frontend::Runtime) -> T) -> Option<T> {
        self.frontend.lock().unwrap().runtime.as_ref().map(f)
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
        let _command_guard = self.command_lock.lock().unwrap();
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
            Command::PlayFromList { ref list, index } if index >= list.tracks.len() => false,
            command => {
                let retry_boot = matches!(command, Command::Reauthenticate);
                self.with_frontend(|runtime| {
                    // An enqueue the engine refused is the one folded rejection.
                    !matches!(
                        runtime.apply(action_for_command(command)),
                        frontend::ActionOutcome::Queued { accepted: 0 }
                    )
                })
                .unwrap_or_else(|| {
                    // Before a successful boot the retry is a channel
                    // hand-off; no fold exists yet to acknowledge it.
                    retry_boot
                        && !self.stop_boot.load(Ordering::Acquire)
                        && matches!(self.login(), LoginState::Expired { .. })
                        && self.retry_boot_tx.send(()).is_ok()
                })
            }
        }
    }

    fn shutdown(&self) {
        self.stop_boot.store(true, Ordering::Release);
        let _ = self.retry_boot_tx.send(());
        let _ = self.paste_url_tx.send(String::new());
        let Some(runtime) = self.frontend.lock().unwrap().begin_shutdown() else {
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

/// Revision-keyed source-neutral catalog projections. Only the latest value
/// for each independent surface is retained; lifecycle states clear it.
#[derive(Default)]
struct TranslationCache {
    search: Option<(CatalogRevision, SearchResults)>,
    detail: Option<(CatalogRevision, SearchDetail)>,
    library: Option<(LibrarySection, CatalogRevision, LibraryState)>,
    implicit_playback: Option<(Arc<frontend::ImplicitPlaybackList>, Option<PlaybackList>)>,
}

impl TranslationCache {
    fn library(&mut self, fork: &frontend::Snapshot, section: LibrarySection) -> LibraryState {
        let has_data = match section {
            LibrarySection::LikedSongs => fork.library.liked_songs.is_some(),
            LibrarySection::RecentlyPlayed => fork.library.recently_played.is_some(),
            LibrarySection::Playlists => fork.library.playlists.is_some(),
        };
        if !has_data {
            self.library = None;
            return map_library(fork, section);
        }
        let revision = CatalogRevision::new(fork.library_revision);
        if let Some((cached_section, cached_revision, cached)) = &self.library
            && *cached_section == section
            && *cached_revision == revision
        {
            return cached.clone();
        }
        let mapped = map_library(fork, section);
        self.library = Some((section, revision, mapped.clone()));
        mapped
    }

    fn implicit_playback(
        &mut self,
        list: Option<&Arc<frontend::ImplicitPlaybackList>>,
    ) -> Option<PlaybackList> {
        let Some(list) = list else {
            self.implicit_playback = None;
            return None;
        };
        if let Some((cached_list, mapped)) = &self.implicit_playback
            && Arc::ptr_eq(cached_list, list)
        {
            return mapped.clone();
        }
        let mapped = map_implicit_queue(list);
        self.implicit_playback = Some((Arc::clone(list), mapped.clone()));
        mapped
    }
}

fn map_search(
    fork: &frontend::Snapshot,
    error: Option<&str>,
    cache: &mut TranslationCache,
) -> SearchState {
    let query = fork.search_query.clone().unwrap_or_default();
    let done = !fork.search_loading
        && fork.search_query.is_some()
        && (error.is_none() || search_data_available(fork));
    if !done {
        cache.search = None;
        return if fork.search_loading {
            SearchState::Loading { query }
        } else if let (Some(_), Some(message)) = (&fork.search_query, error) {
            SearchState::Failed {
                query,
                message: message.to_string(),
            }
        } else {
            SearchState::Idle
        };
    }
    let revision = CatalogRevision::new(fork.search_revision);
    let results = if let Some((cached_revision, results)) = &cache.search
        && *cached_revision == revision
    {
        results.clone()
    } else {
        let results = map_search_results(fork);
        cache.search = Some((revision, results.clone()));
        results
    };
    SearchState::Done {
        query,
        revision,
        results,
    }
}

fn map_search_results(fork: &frontend::Snapshot) -> SearchResults {
    SearchResults {
        tracks: playables(&fork.search_tracks),
        artists: fork
            .search_artists
            .iter()
            .map(|artist| SearchArtist {
                locator: artist
                    .uri
                    .clone()
                    .or_else(|| artist.id.clone())
                    .map(|locator| spotify_locator("artist", &locator))
                    .unwrap_or_default(),
                name: artist.name.clone(),
            })
            .collect(),
        albums: fork.search_albums.iter().map(search_album).collect(),
        playlists: fork
            .search_playlists
            .iter()
            .map(|playlist| SearchPlaylist {
                locator: spotify_locator("playlist", &playlist.uri),
                name: playlist.name.clone(),
                owner: playlist.owner.clone(),
                track_count: playlist.track_count,
            })
            .collect(),
    }
}

fn playables(tracks: &[frontend::TrackInfo]) -> Arc<[Playable]> {
    tracks.iter().filter_map(playable_from_track).collect()
}

fn search_album(album: &frontend::AlbumInfo) -> SearchAlbum {
    SearchAlbum {
        locator: album
            .uri
            .clone()
            .or_else(|| album.id.clone())
            .map(|locator| spotify_locator("album", &locator))
            .unwrap_or_default(),
        name: album.name.clone(),
        artists: album
            .artists
            .iter()
            .map(|artist| artist.name.clone())
            .collect(),
    }
}

fn spotify_locator(kind: &str, locator: &str) -> String {
    if locator.starts_with("spotify:") {
        locator.to_string()
    } else {
        format!("spotify:{kind}:{locator}")
    }
}

fn map_detail(fork: &frontend::Snapshot, cache: &mut TranslationCache) -> Option<SearchDetail> {
    let Some(detail) = fork.search_detail.as_ref() else {
        cache.detail = None;
        return None;
    };
    let revision = CatalogRevision::new(fork.detail_revision);
    if let Some((cached_revision, detail)) = &cache.detail
        && *cached_revision == revision
    {
        return Some(detail.clone());
    }
    let locator = |kind, locator: &Option<String>| {
        locator
            .as_deref()
            .map(|locator| spotify_locator(kind, locator))
    };
    let detail = match detail {
        frontend::SearchDetail::Artist {
            target_locator,
            complete,
            tracks,
            albums,
        } => SearchDetail::Artist {
            target_locator: locator("artist", target_locator),
            complete: *complete,
            revision,
            tracks: playables(tracks),
            albums: albums.iter().map(search_album).collect(),
        },
        frontend::SearchDetail::Album {
            target_locator,
            complete,
            tracks,
        } => SearchDetail::Album {
            target_locator: locator("album", target_locator),
            complete: *complete,
            revision,
            tracks: playables(tracks),
        },
        frontend::SearchDetail::Playlist {
            target_locator,
            complete,
            tracks,
        } => SearchDetail::Playlist {
            target_locator: locator("playlist", target_locator),
            complete: *complete,
            revision,
            tracks: playables(tracks),
        },
    };
    cache.detail = Some((revision, detail.clone()));
    Some(detail)
}

fn action_for_command(command: Command) -> EngineAction {
    match command {
        Command::Play(playable) => EngineAction::PlayUris {
            uris: vec![playable.locator],
            offset: None,
        },
        Command::PlayFromList { list, index } => EngineAction::PlayUrisWithList {
            uris: list
                .tracks
                .iter()
                .map(|playable| playable.locator.clone())
                .collect(),
            offset: index,
            list: frontend::ImplicitPlaybackList {
                source: implicit_playback_source(&list.source),
                tracks: list.tracks.iter().map(track_info).collect(),
                current_index: index,
            },
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
        Command::DismissNotice => EngineAction::DismissNotice,
        Command::Search(query) => EngineAction::SearchActiveSource(query),
        Command::OpenSearchTarget(target) => EngineAction::Open(match target {
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
        }),
        Command::Browse(section) => EngineAction::OpenLibrary(match section {
            LibrarySection::Playlists => LibraryTarget::Playlists,
            LibrarySection::LikedSongs => LibraryTarget::LikedSongs,
            LibrarySection::RecentlyPlayed => LibraryTarget::RecentlyPlayed,
        }),
        Command::Reauthenticate => EngineAction::BeginSpotifyLogin,
        Command::SubmitPastedLoginUrl(_) => unreachable!("handled in `command`"),
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

fn library_section(target: LibraryTarget) -> Option<LibrarySection> {
    match target {
        LibraryTarget::Playlists => Some(LibrarySection::Playlists),
        LibraryTarget::LikedSongs => Some(LibrarySection::LikedSongs),
        LibraryTarget::RecentlyPlayed => Some(LibrarySection::RecentlyPlayed),
        _ => None,
    }
}

fn map_library(fork: &frontend::Snapshot, section: LibrarySection) -> LibraryState {
    if performance_enabled() {
        LIBRARY_TRANSLATIONS.fetch_add(1, Ordering::Relaxed);
    }
    let has_data = match section {
        LibrarySection::LikedSongs => fork.library.liked_songs.is_some(),
        LibrarySection::RecentlyPlayed => fork.library.recently_played.is_some(),
        LibrarySection::Playlists => fork.library.playlists.is_some(),
    };
    if !has_data
        && fork.notice_is_error
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
    let track_entries = |tracks: &[frontend::TrackInfo]| -> Vec<LibraryEntry> {
        tracks
            .iter()
            .filter_map(playable_from_track)
            .map(|playable| LibraryEntry::Track { playable })
            .collect()
    };
    let entries = match section {
        LibrarySection::LikedSongs => fork.library.liked_songs.as_deref().map(track_entries),
        LibrarySection::RecentlyPlayed => {
            fork.library.recently_played.as_deref().map(track_entries)
        }
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
            revision: CatalogRevision::new(fork.library_revision),
            entries: entries.into(),
        },
        None => LibraryState::Loading { section },
    }
}

fn search_data_available(fork: &frontend::Snapshot) -> bool {
    fork.search_has_results_page
        || !fork.search_tracks.is_empty()
        || !fork.search_artists.is_empty()
        || !fork.search_albums.is_empty()
        || !fork.search_playlists.is_empty()
}

fn map_snapshot(fork: &frontend::Snapshot, cache: &mut TranslationCache) -> Snapshot {
    if performance_enabled() {
        SNAPSHOT_TRANSLATIONS.fetch_add(1, Ordering::Relaxed);
    }
    // An empty notice is a dismissal in flight (see `Command::DismissNotice`),
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

    let search = map_search(fork, error, cache);

    let playback = fork.playback.as_ref().and_then(|state| {
        Some(PlaybackStatus {
            playable: playable_from_track(state.track.as_ref()?)?,
            device: if fork.native_playback {
                PlaybackDevice::Native
            } else {
                PlaybackDevice::Remote
            },
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
    let library = match fork.library_target.and_then(library_section) {
        Some(section) => cache.library(fork, section),
        None => {
            cache.library = None;
            LibraryState::Idle
        }
    };

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
        search_detail: map_detail(fork, cache),
        playback,
        queue,
        implicit_queue: cache.implicit_playback(fork.implicit_playback.as_ref()),
        library,
        audio,
        notice: notice.map(str::to_string),
    }
}

fn implicit_playback_source(
    source: &player_core::PlaybackListSource,
) -> frontend::ImplicitPlaybackSource {
    match source {
        player_core::PlaybackListSource::LikedSongs => frontend::ImplicitPlaybackSource::LikedSongs,
        player_core::PlaybackListSource::RecentlyPlayed => {
            frontend::ImplicitPlaybackSource::RecentlyPlayed
        }
        player_core::PlaybackListSource::SearchResults { query } => {
            frontend::ImplicitPlaybackSource::SearchResults {
                query: query.clone(),
            }
        }
        player_core::PlaybackListSource::Artist { locator, name } => {
            frontend::ImplicitPlaybackSource::Artist {
                locator: locator.clone(),
                name: name.clone(),
            }
        }
        player_core::PlaybackListSource::Album { locator, name } => {
            frontend::ImplicitPlaybackSource::Album {
                locator: locator.clone(),
                name: name.clone(),
            }
        }
        player_core::PlaybackListSource::Playlist { locator, name } => {
            frontend::ImplicitPlaybackSource::Playlist {
                locator: locator.clone(),
                name: name.clone(),
            }
        }
    }
}

fn map_implicit_queue(list: &frontend::ImplicitPlaybackList) -> Option<PlaybackList> {
    let tracks = list
        .tracks
        .iter()
        .map(playable_from_track)
        .collect::<Option<Vec<_>>>()?;
    if tracks.is_empty() || list.current_index >= tracks.len() {
        return None;
    }
    Some(PlaybackList {
        source: match &list.source {
            frontend::ImplicitPlaybackSource::LikedSongs => {
                player_core::PlaybackListSource::LikedSongs
            }
            frontend::ImplicitPlaybackSource::RecentlyPlayed => {
                player_core::PlaybackListSource::RecentlyPlayed
            }
            frontend::ImplicitPlaybackSource::SearchResults { query } => {
                player_core::PlaybackListSource::SearchResults {
                    query: query.clone(),
                }
            }
            frontend::ImplicitPlaybackSource::Artist { locator, name } => {
                player_core::PlaybackListSource::Artist {
                    locator: locator.clone(),
                    name: name.clone(),
                }
            }
            frontend::ImplicitPlaybackSource::Album { locator, name } => {
                player_core::PlaybackListSource::Album {
                    locator: locator.clone(),
                    name: name.clone(),
                }
            }
            frontend::ImplicitPlaybackSource::Playlist { locator, name } => {
                player_core::PlaybackListSource::Playlist {
                    locator: locator.clone(),
                    name: name.clone(),
                }
            }
        },
        tracks: tracks.into(),
        current_index: list.current_index,
    })
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
            EngineAction::PlayUrisWithList {
                uris: vec![
                    "spotify:track:first".to_string(),
                    "spotify:track:second".to_string(),
                ],
                offset: 1,
                list: frontend::ImplicitPlaybackList {
                    source: frontend::ImplicitPlaybackSource::Album {
                        locator: "spotify:album:album".to_string(),
                        name: "Album".to_string(),
                    },
                    tracks: vec![
                        track_info(&playable("spotify:track:first")),
                        track_info(&playable("spotify:track:second")),
                    ],
                    current_index: 1,
                },
            }
        );
    }

    #[test]
    fn shutdown_wins_boot_stage_and_rejects_late_runtime() {
        // This is the exact interleaving that used to leak a booted Runtime:
        // shutdown observes an empty slot, then the blocking boot thread
        // completes. The terminal slot state makes that late hand-off fail.
        let slot = Mutex::new(FrontendSlot::<u8>::default());
        assert_eq!(slot.lock().unwrap().begin_shutdown(), None);
        assert_eq!(slot.lock().unwrap().stage(7), Err(7));
    }

    #[test]
    fn preboot_shutdown_cancels_retry_wait_and_onboarding_prompt() {
        let stop = Arc::new(AtomicBool::new(true));
        let (_paste_tx, paste_rx) = mpsc::channel();
        let (login_tx, _login_rx) = watch::channel(Snapshot::default());
        let onboarding = BootOnboarding {
            login_tx,
            pasted_url_rx: Mutex::new(paste_rx),
            stop: Arc::clone(&stop),
        };
        let (_retry_tx, retry_rx) = mpsc::channel();
        assert!(!wait_for_retry(&retry_rx, &stop));
        assert!(onboarding.prompt_line("redirect").is_err());
    }

    /// Fork state while idle-with-results and playing.
    fn idle_with_results() -> frontend::Snapshot {
        frontend::Snapshot {
            search_tracks: vec![track("a"), track("b")].into(),
            spotify_connected: true,
            audio_ready: true,
            ..Default::default()
        }
    }

    #[test]
    fn source_neutral_catalog_projections_reuse_only_unchanged_revisions() {
        let mut fork = idle_with_results();
        fork.search_query = Some("query".to_string());
        fork.search_revision = 7;
        let mut cache = TranslationCache::default();
        let first = map_snapshot(&fork, &mut cache);
        let first_rows = match &first.search {
            SearchState::Done { results, .. } => Arc::clone(&results.tracks),
            other => panic!("expected done search, got {other:?}"),
        };
        fork.position_ms = Some(10);
        let unchanged = map_snapshot(&fork, &mut cache);
        let unchanged_rows = match &unchanged.search {
            SearchState::Done { results, .. } => Arc::clone(&results.tracks),
            other => panic!("expected done search, got {other:?}"),
        };
        assert!(Arc::ptr_eq(&first_rows, &unchanged_rows));

        fork.search_revision = 8;
        fork.search_tracks = vec![track("changed")].into();
        let replaced = map_snapshot(&fork, &mut cache);
        let replaced_rows = match &replaced.search {
            SearchState::Done { results, .. } => Arc::clone(&results.tracks),
            other => panic!("expected done search, got {other:?}"),
        };
        assert!(!Arc::ptr_eq(&first_rows, &replaced_rows));
    }

    #[test]
    fn detail_and_library_projections_reuse_only_unchanged_revisions() {
        let mut fork = idle_with_results();
        fork.detail_revision = 3;
        fork.search_detail = Some(frontend::SearchDetail::Album {
            target_locator: Some("spotify:album:detail".to_string()),
            complete: true,
            tracks: vec![track("detail-a")].into(),
        });
        let mut cache = TranslationCache::default();
        let first = map_snapshot(&fork, &mut cache);
        let first_tracks = match first.search_detail.as_ref().unwrap() {
            SearchDetail::Album { tracks, .. } => Arc::clone(tracks),
            _ => unreachable!(),
        };
        fork.position_ms = Some(1);
        let reused = map_snapshot(&fork, &mut cache);
        let reused_tracks = match reused.search_detail.as_ref().unwrap() {
            SearchDetail::Album { tracks, .. } => Arc::clone(tracks),
            _ => unreachable!(),
        };
        assert!(Arc::ptr_eq(&first_tracks, &reused_tracks));

        fork.detail_revision = 4;
        fork.search_detail = Some(frontend::SearchDetail::Album {
            target_locator: Some("spotify:album:detail".to_string()),
            complete: true,
            tracks: vec![track("detail-b")].into(),
        });
        let replaced = map_snapshot(&fork, &mut cache);
        let replaced_tracks = match replaced.search_detail.as_ref().unwrap() {
            SearchDetail::Album { tracks, .. } => Arc::clone(tracks),
            _ => unreachable!(),
        };
        assert!(!Arc::ptr_eq(&first_tracks, &replaced_tracks));

        fork.library_target = Some(LibraryTarget::LikedSongs);
        fork.library_revision = 8;
        fork.library.liked_songs = Some(vec![track("library-a")].into());
        let _ = map_snapshot(&fork, &mut cache);
        let first_library = cache.library(&fork, LibrarySection::LikedSongs);
        let first_entries = match &first_library {
            LibraryState::Done { entries, .. } => Arc::clone(entries),
            other => panic!("expected library completion, got {other:?}"),
        };
        fork.position_ms = Some(2);
        let _ = map_snapshot(&fork, &mut cache);
        let reused_library = cache.library(&fork, LibrarySection::LikedSongs);
        let reused_entries = match &reused_library {
            LibraryState::Done { entries, .. } => Arc::clone(entries),
            other => panic!("expected library completion, got {other:?}"),
        };
        assert!(Arc::ptr_eq(&first_entries, &reused_entries));
        fork.library_revision = 9;
        fork.library.liked_songs = Some(vec![track("library-b")].into());
        let _ = map_snapshot(&fork, &mut cache);
        let replaced_library = cache.library(&fork, LibrarySection::LikedSongs);
        let replaced_entries = match &replaced_library {
            LibraryState::Done { entries, .. } => Arc::clone(entries),
            other => panic!("expected library completion, got {other:?}"),
        };
        assert!(!Arc::ptr_eq(&first_entries, &replaced_entries));
    }

    #[test]
    fn relayed_catalog_failure_keeps_playback_healthy() {
        let fork = frontend::Snapshot {
            notice: Some("offline".to_string()),
            notice_is_error: true,
            spotify_connected: true,
            audio_ready: true,
            playback: Some(frontend::PlaybackState {
                track: Some(track("playing")),
                is_playing: true,
                progress_ms: 5,
                shuffle: false,
                repeat: "off".to_string(),
                volume_percent: None,
                device: None,
            }),
            ..Default::default()
        };
        let snapshot = map_snapshot(&fork, &mut TranslationCache::default());
        assert_eq!(snapshot.login, LoginState::Ready);
        assert_eq!(snapshot.audio, AudioState::Ready);
        assert!(matches!(
            snapshot.playback,
            Some(PlaybackStatus { playable, device: PlaybackDevice::Remote, .. })
                if playable.locator == "spotify:track:playing"
        ));

        let mut native = fork;
        native.native_playback = true;
        assert!(matches!(
            map_snapshot(&native, &mut TranslationCache::default()).playback,
            Some(PlaybackStatus {
                device: PlaybackDevice::Native,
                ..
            })
        ));
    }

    #[test]
    fn empty_search_survives_unrelated_error_but_failed_retry_does_not() {
        let mut fork = frontend::Snapshot {
            search_query: Some("empty".into()),
            search_has_results_page: true,
            notice: Some("playback failed".into()),
            notice_is_error: true,
            ..Default::default()
        };
        let mut cache = TranslationCache::default();
        assert!(matches!(
            map_snapshot(&fork, &mut cache).search,
            SearchState::Done { results, .. } if results.tracks.is_empty()
        ));
        fork.search_query = Some("retry".into());
        fork.search_loading = true;
        fork.search_has_results_page = false;
        assert!(matches!(
            map_snapshot(&fork, &mut cache).search,
            SearchState::Loading { .. }
        ));
        fork.search_loading = false;
        fork.notice = Some("catalog unavailable".into());
        assert!(matches!(
            map_snapshot(&fork, &mut cache).search,
            SearchState::Failed { message, .. } if message == "catalog unavailable"
        ));
    }

    #[test]
    fn maps_fork_library_rows_and_keeps_unfetched_sections_loading() {
        let fork = frontend::Snapshot {
            library: frontend::LibrarySnapshot {
                liked_songs: Some(vec![track("liked")].into()),
                recently_played: Some(vec![track("recent")].into()),
                playlists: Some(
                    vec![frontend::PlaylistInfo {
                        uri: "spotify:playlist:mix".to_string(),
                        name: "Mix".to_string(),
                        owner: "me".to_string(),
                        track_count: 3,
                        id: Some("mix".to_string()),
                        owner_id: None,
                        collaborative: false,
                        public: Some(false),
                        image_url: None,
                    }]
                    .into(),
                ),
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
            } if matches!(entries.as_ref(), [LibraryEntry::Track { playable }]
                if playable.locator == "spotify:track:liked")
        ));

        let playlists = map_library(&fork, LibrarySection::Playlists);
        assert!(matches!(
            playlists,
            LibraryState::Done {
                section: LibrarySection::Playlists,
                entries,
                ..
            } if matches!(entries.as_ref(), [LibraryEntry::Playlist { id, name, track_count }]
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

        let loaded_with_error = frontend::Snapshot {
            library_target: Some(LibraryTarget::RecentlyPlayed),
            library: frontend::LibrarySnapshot {
                recently_played: Some(vec![track("recent")].into()),
                ..Default::default()
            },
            notice: Some("playback failed".to_string()),
            notice_is_error: true,
            ..Default::default()
        };
        assert!(matches!(
            map_library(&loaded_with_error, LibrarySection::RecentlyPlayed),
            LibraryState::Done { .. }
        ));

        let mut cache = TranslationCache::default();
        assert!(matches!(
            cache.library(&loaded_with_error, LibrarySection::RecentlyPlayed,),
            LibraryState::Done { .. }
        ));
        let cleared = frontend::Snapshot {
            library_target: Some(LibraryTarget::RecentlyPlayed),
            ..Default::default()
        };
        assert!(matches!(
            cache.library(&cleared, LibrarySection::RecentlyPlayed,),
            LibraryState::Loading {
                section: LibrarySection::RecentlyPlayed
            }
        ));

        assert!(matches!(
            map_library(
                &frontend::Snapshot {
                    notice: Some("catalog unavailable".to_string()),
                    notice_is_error: true,
                    ..Default::default()
                },
                LibrarySection::RecentlyPlayed
            ),
            LibraryState::Failed { message, .. } if message == "catalog unavailable"
        ));
    }
}
