//! A scripted [`Runtime`](crate::Runtime) with no credentials and no audio
//! hardware. It answers every command the window can send, so UI behavior
//! is exercisable end to end.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use crate::{
    ActionOutcome, AudioState, CatalogRevision, Command, LibraryEntry, LibrarySection,
    LibraryState, LoginState, Playable, PlaybackDevice, PlaybackList, PlaybackListProjector,
    PlaybackStatus, SearchAlbum, SearchArtist, SearchPlaylist, SearchResults, SearchState,
    Snapshot,
};

/// How long a scripted search stays in `Loading` so the state is visible.
const SEARCH_DELAY: Duration = Duration::from_millis(150);
type CommandReply = mpsc::Sender<Option<ActionOutcome>>;
type CommandRequest = (Command, CommandReply);

fn canned_track(name: &str, artist: &str, album: &str, duration_ms: u64, id: &str) -> Playable {
    Playable {
        source: crate::Source::Spotify,
        locator: format!("spotify:track:{id}"),
        title: name.to_string(),
        artists: vec![artist.to_string()],
        album: album.to_string(),
        duration_ms,
    }
}

fn canned_tracks() -> Vec<Playable> {
    vec![
        canned_track(
            "Mr. Blue Sky",
            "Electric Light Orchestra",
            "Out of the Blue",
            302_000,
            "4uLU6hMCjMI75M1A2tKUQC",
        ),
        canned_track(
            "Dreamweaver",
            "The Scripted Fake",
            "Test Signals",
            214_000,
            "6Y2SdcH4DoWU1RdBFRxNPL",
        ),
    ]
}

fn canned_results() -> SearchResults {
    SearchResults {
        tracks: canned_tracks(),
        artists: vec![SearchArtist {
            locator: "spotify:artist:elo".to_string(),
            name: "Electric Light Orchestra".to_string(),
        }],
        albums: vec![SearchAlbum {
            locator: "spotify:album:outoftheblue".to_string(),
            name: "Out of the Blue".to_string(),
            artists: vec!["Electric Light Orchestra".to_string()],
        }],
        playlists: vec![SearchPlaylist {
            locator: "spotify:playlist:bluesky".to_string(),
            name: "Blue Sky Mix".to_string(),
            owner: "Rust Player".to_string(),
            track_count: 24,
        }],
    }
}

/// Canned rows per library section; the scripted library is static.
fn canned_library(section: LibrarySection) -> Vec<LibraryEntry> {
    // "Recently played" rows carry real Unix-millis stamps so relative-time
    // rendering has something truthful to chew on.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    match section {
        LibrarySection::LikedSongs => canned_tracks()
            .into_iter()
            .chain([
                canned_track(
                    "Nightdrive",
                    "Neon Script",
                    "Chrome Hours",
                    256_000,
                    "3nL9wCcKvOo7VpQ0fake01",
                ),
                canned_track(
                    "Slow Tide",
                    "Harbor Lights",
                    "Undertow",
                    198_000,
                    "3nL9wCcKvOo7VpQ0fake02",
                ),
            ])
            .map(|playable| LibraryEntry::Track {
                playable,
                played_at_ms: None,
            })
            .collect(),
        LibrarySection::RecentlyPlayed => canned_tracks()
            .into_iter()
            .enumerate()
            .map(|(i, playable)| LibraryEntry::Track {
                played_at_ms: Some(now_ms.saturating_sub((i as u64 + 1) * 3_600_000)),
                playable,
            })
            .collect(),
        LibrarySection::Playlists => vec![
            LibraryEntry::Playlist {
                id: "fake:playlist:focus".to_string(),
                name: "Deep Focus".to_string(),
                track_count: 42,
            },
            LibraryEntry::Playlist {
                id: "fake:playlist:drive".to_string(),
                name: "Night Drive".to_string(),
                track_count: 28,
            },
            LibraryEntry::Playlist {
                id: "fake:playlist:discovered".to_string(),
                name: "Discovered Weekly".to_string(),
                track_count: 30,
            },
        ],
    }
}

/// The scripted fake. Commands mutate a plain state struct on one worker
/// thread and republish; a search shows `Loading` for [`SEARCH_DELAY`]
/// before resolving to canned results.
pub struct FakeRuntime {
    tx: watch::Sender<Snapshot>,
    commands: Mutex<Option<mpsc::Sender<CommandRequest>>>,
    state: Arc<Mutex<Snapshot>>,
    projector: Arc<Mutex<PlaybackListProjector>>,
    stop: Arc<AtomicBool>,
}

impl FakeRuntime {
    pub fn new() -> Self {
        // Land in Ready immediately: the fake has no sign-in step.
        let initial = Snapshot {
            login: LoginState::Ready,
            audio: AudioState::Ready,
            ..Snapshot::default()
        };
        let (tx, _rx) = watch::channel(initial.clone());
        let (commands, command_rx) = mpsc::channel::<CommandRequest>();
        let state = Arc::new(Mutex::new(initial));
        let projector = Arc::new(Mutex::new(PlaybackListProjector::default()));
        let next_revision = Arc::new(AtomicU64::new(1));
        let stop = Arc::new(AtomicBool::new(false));

        let worker_state = Arc::clone(&state);
        let worker_tx = tx.clone();
        let worker_stop = Arc::clone(&stop);
        let worker_projector = Arc::clone(&projector);
        let worker_revision = Arc::clone(&next_revision);
        std::thread::Builder::new()
            .name("player-fake-runtime".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Relaxed) {
                    match command_rx.recv_timeout(Duration::from_millis(100)) {
                        Ok((command, reply)) => apply(
                            &worker_state,
                            &worker_tx,
                            &worker_projector,
                            &worker_revision,
                            command,
                            reply,
                        ),
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .expect("spawn fake runtime thread");

        FakeRuntime {
            tx,
            commands: Mutex::new(Some(commands)),
            state,
            projector,
            stop,
        }
    }

    /// Force one exact snapshot (scripted scenarios for failure states).
    pub fn script_snapshot(&self, snapshot: Snapshot) {
        let mut projector = self.projector.lock().unwrap();
        if matches!(
            snapshot.search,
            SearchState::Loading { .. } | SearchState::Failed { .. }
        ) || matches!(
            snapshot.library,
            LibraryState::Loading { .. } | LibraryState::Failed { .. }
        ) {
            projector.clear();
        } else if matches!(snapshot.search, SearchState::Done { .. }) {
            projector.project_search(&snapshot.search);
        } else if matches!(snapshot.library, LibraryState::Done { .. }) {
            projector.project_library(&snapshot.library);
        }
        drop(projector);
        *self.state.lock().unwrap() = snapshot;
        publish(&self.state, &self.tx);
    }

    /// The completed catalog candidate held by the fake's projector.
    pub fn candidate_list(&self) -> Option<Arc<PlaybackList>> {
        self.projector.lock().unwrap().candidate()
    }
}

impl Default for FakeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn apply(
    state: &Mutex<Snapshot>,
    tx: &watch::Sender<Snapshot>,
    projector: &Mutex<PlaybackListProjector>,
    next_revision: &AtomicU64,
    command: Command,
    reply: mpsc::Sender<Option<ActionOutcome>>,
) {
    if let Command::Search(query) = command {
        projector.lock().unwrap().clear();
        state.lock().unwrap().search = SearchState::Loading {
            query: query.clone(),
        };
        publish(state, tx);
        // A command is acknowledged once its loading fact is folded. The
        // eventual Done fact is an independent worker completion.
        let _ = reply.send(Some(ActionOutcome::Applied));
        std::thread::sleep(SEARCH_DELAY);
        let needle = query.to_lowercase();
        let results: Vec<Playable> = canned_results()
            .tracks
            .into_iter()
            .filter(|p| {
                needle.is_empty()
                    || p.title.to_lowercase().contains(&needle)
                    || p.artists_display().to_lowercase().contains(&needle)
            })
            .collect();
        let results = if results.is_empty() {
            SearchResults::default()
        } else {
            SearchResults {
                tracks: results,
                ..canned_results()
            }
        };
        let done = SearchState::Done {
            query,
            revision: CatalogRevision::new(next_revision.fetch_add(1, Ordering::Relaxed)),
            results,
        };
        projector.lock().unwrap().project_search(&done);
        state.lock().unwrap().search = done;
        publish(state, tx);
        return;
    }

    if let Command::Browse(section) = command {
        projector.lock().unwrap().clear();
        state.lock().unwrap().library = LibraryState::Loading { section };
        publish(state, tx);
        let _ = reply.send(Some(ActionOutcome::Applied));
        std::thread::sleep(SEARCH_DELAY);
        let done = LibraryState::Done {
            section,
            revision: CatalogRevision::new(next_revision.fetch_add(1, Ordering::Relaxed)),
            entries: canned_library(section),
        };
        projector.lock().unwrap().project_library(&done);
        state.lock().unwrap().library = done;
        publish(state, tx);
        return;
    }

    let mut snap = state.lock().unwrap();
    let accepted = !matches!(&command, Command::PlayFromList { list, index }
        if list.tracks.is_empty() || *index >= list.tracks.len());
    let auth_accepted = match &command {
        Command::SubmitPastedLoginUrl(_) => matches!(
            &snap.login,
            LoginState::InProgress {
                wants_pasted_url: true,
                ..
            }
        ),
        Command::Reauthenticate => {
            matches!(&snap.login, LoginState::Ready | LoginState::Expired { .. })
        }
        _ => true,
    };
    let queue_accepted = match &command {
        Command::Enqueue(playable) => !playable.locator.starts_with("radio:"),
        _ => true,
    };
    if !accepted || !auth_accepted {
        let _ = reply.send(None);
        return;
    }
    let preboot_retry = matches!(
        (&command, &snap.login),
        (Command::Reauthenticate, LoginState::Expired { .. })
    );
    let enqueue = matches!(&command, Command::Enqueue(_));
    match command {
        Command::Search(_) | Command::Browse(_) => unreachable!("handled above"),
        Command::OpenSearchTarget(_) => {}
        Command::SubmitPastedLoginUrl(_) | Command::Reauthenticate => {
            snap.login = LoginState::Ready;
        }
        Command::Play(playable) => {
            snap.implicit_queue = None;
            start_playback(&mut snap, playable);
        }
        Command::PlayFromList { list, index } => {
            let mut list = (*list).clone();
            if let Some(playable) = list.tracks.get(index).cloned() {
                list.current_index = index;
                snap.implicit_queue = Some(list);
                start_playback(&mut snap, playable);
            }
        }
        Command::Pause => {
            if let Some(p) = snap.playback.as_mut() {
                p.is_playing = false;
                p.position_ms = crate::project_position(p, Instant::now());
                p.observed_at = Instant::now();
            }
        }
        Command::Resume => {
            if let Some(p) = snap.playback.as_mut() {
                p.is_playing = true;
                p.observed_at = Instant::now();
            }
        }
        Command::Seek(position_ms) => {
            if let Some(p) = snap.playback.as_mut() {
                p.position_ms = position_ms;
                p.observed_at = Instant::now();
            }
        }
        Command::Next => {
            if !snap.queue.is_empty() {
                let next = snap.queue.remove(0);
                start_playback(&mut snap, next);
            } else if let Some(next) = advance_implicit_queue(&mut snap) {
                start_playback(&mut snap, next);
            }
        }
        Command::Previous => {
            let previous = snap.implicit_queue.as_mut().and_then(|list| {
                let previous_index = list.current_index.checked_sub(1)?;
                list.current_index = previous_index;
                list.tracks.get(previous_index).cloned()
            });
            if let Some(previous) = previous {
                start_playback(&mut snap, previous);
            } else if let Some(p) = snap.playback.as_mut() {
                p.position_ms = 0;
                p.observed_at = Instant::now();
            }
        }
        Command::SetVolume(percent) => {
            if let Some(p) = snap.playback.as_mut() {
                p.volume_percent = Some(percent.min(100));
            }
        }
        Command::Enqueue(playable) if queue_accepted => snap.queue.push(playable),
        Command::Enqueue(_) => {}
        Command::RemoveQueued(index) => {
            if index < snap.queue.len() {
                snap.queue.remove(index);
            }
        }
        Command::MoveQueued { index, up } => {
            let len = snap.queue.len();
            if up && index > 0 && index < len {
                snap.queue.swap(index - 1, index);
            } else if !up && index + 1 < len {
                snap.queue.swap(index, index + 1);
            }
        }
        Command::ClearQueue => snap.queue.clear(),
        Command::DismissNotice => snap.notice = None,
    }
    drop(snap);
    publish(state, tx);
    let outcome = if preboot_retry {
        ActionOutcome::Accepted
    } else if enqueue {
        ActionOutcome::Queued {
            accepted: usize::from(queue_accepted),
        }
    } else {
        ActionOutcome::Applied
    };
    let _ = reply.send(Some(outcome));
}

fn start_playback(snapshot: &mut Snapshot, playable: Playable) {
    let volume_percent = snapshot
        .playback
        .as_ref()
        .and_then(|playback| playback.volume_percent)
        .or(Some(80));
    snapshot.playback = Some(PlaybackStatus {
        playable,
        device: PlaybackDevice::Native,
        is_playing: true,
        position_ms: 0,
        observed_at: Instant::now(),
        volume_percent,
    });
}

fn advance_implicit_queue(snapshot: &mut Snapshot) -> Option<Playable> {
    let list = snapshot.implicit_queue.as_mut()?;
    let next_index = list.current_index.checked_add(1)?;
    let next = list.tracks.get(next_index).cloned()?;
    list.current_index = next_index;
    Some(next)
}

fn publish(state: &Mutex<Snapshot>, tx: &watch::Sender<Snapshot>) {
    let snapshot = state.lock().unwrap().clone();
    tx.send_if_modified(|current| {
        if *current == snapshot {
            false
        } else {
            *current = snapshot;
            true
        }
    });
}

impl crate::Runtime for FakeRuntime {
    fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.tx.subscribe()
    }

    fn command(&self, command: Command) -> Option<ActionOutcome> {
        let (reply, result) = mpsc::channel();
        let commands = self.commands.lock().unwrap();
        commands.as_ref()?.send((command, reply)).ok()?;
        drop(commands);
        result.recv().ok().flatten()
    }

    fn shutdown(&self) {
        // End the worker thread; the process is on its way out.
        self.stop.store(true, Ordering::Relaxed);
        self.commands.lock().unwrap().take();
    }
}
