//! A scripted [`Runtime`](crate::Runtime) with no credentials and no audio
//! hardware. It answers every command the window can send, so UI behavior
//! is exercisable end to end.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use crate::{
    AudioState, Command, LibraryEntry, LibrarySection, LibraryState, LoginState, Playable,
    PlaybackStatus, SearchAlbum, SearchArtist, SearchPlaylist, SearchResults, SearchState,
    Snapshot,
};

/// How long a scripted search stays in `Loading` so the state is visible.
const SEARCH_DELAY: Duration = Duration::from_millis(150);

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
    commands: mpsc::Sender<Command>,
    state: Arc<Mutex<Snapshot>>,
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
        let (commands, command_rx) = mpsc::channel::<Command>();
        let state = Arc::new(Mutex::new(initial));
        let stop = Arc::new(AtomicBool::new(false));

        let worker_state = Arc::clone(&state);
        let worker_tx = tx.clone();
        let worker_stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("player-fake-runtime".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Relaxed) {
                    match command_rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(command) => apply(&worker_state, &worker_tx, command),
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .expect("spawn fake runtime thread");

        FakeRuntime {
            tx,
            commands,
            state,
            stop,
        }
    }

    /// Force one exact snapshot (scripted scenarios for failure states).
    pub fn script_snapshot(&self, snapshot: Snapshot) {
        *self.state.lock().unwrap() = snapshot;
        publish(&self.state, &self.tx);
    }
}

impl Default for FakeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn apply(state: &Mutex<Snapshot>, tx: &watch::Sender<Snapshot>, command: Command) {
    if let Command::Search(query) = command {
        state.lock().unwrap().search = SearchState::Loading {
            query: query.clone(),
        };
        publish(state, tx);
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
        state.lock().unwrap().search = SearchState::Done { query, results };
        publish(state, tx);
        return;
    }

    if let Command::Browse(section) = command {
        state.lock().unwrap().library = LibraryState::Loading { section };
        publish(state, tx);
        std::thread::sleep(SEARCH_DELAY);
        state.lock().unwrap().library = LibraryState::Done {
            section,
            entries: canned_library(section),
        };
        publish(state, tx);
        return;
    }

    let mut snap = state.lock().unwrap();
    match command {
        Command::Search(_) | Command::Browse(_) => unreachable!("handled above"),
        Command::OpenSearchTarget(_) => {}
        Command::SubmitPastedLoginUrl(_) | Command::Reauthenticate => {
            snap.login = LoginState::Ready;
        }
        Command::Play(playable) => {
            snap.playback = Some(PlaybackStatus {
                playable,
                is_playing: true,
                position_ms: 0,
                observed_at: Instant::now(),
                volume_percent: Some(80),
            });
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
                snap.playback = Some(PlaybackStatus {
                    playable: next,
                    is_playing: true,
                    position_ms: 0,
                    observed_at: Instant::now(),
                    volume_percent: snap.playback.as_ref().and_then(|p| p.volume_percent),
                });
            }
        }
        Command::Previous => {
            if let Some(p) = snap.playback.as_mut() {
                p.position_ms = 0;
                p.observed_at = Instant::now();
            }
        }
        Command::SetVolume(percent) => {
            if let Some(p) = snap.playback.as_mut() {
                p.volume_percent = Some(percent.min(100));
            }
        }
        Command::Enqueue(playable) => snap.queue.push(playable),
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

    fn command(&self, command: Command) -> bool {
        self.commands.send(command).is_ok()
    }

    fn shutdown(&self) {
        // End the worker thread; the process is on its way out.
        self.stop.store(true, Ordering::Relaxed);
    }
}
