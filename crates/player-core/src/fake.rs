//! A scripted [`Runtime`](crate::Runtime) with no credentials and no audio
//! hardware. It answers every command the window can send, so UI behavior
//! is exercisable end to end.

use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use crate::{
    AudioState, CatalogRevision, Command, LibraryEntry, LibrarySection, LibraryState, LoginState,
    Playable, PlaybackDevice, PlaybackStatus, SearchAlbum, SearchArtist, SearchPlaylist,
    SearchResults, SearchState, Snapshot,
};

/// How long a scripted search stays in `Loading` so the state is visible.
const SEARCH_DELAY: Duration = Duration::from_millis(150);
/// A command and the channel that acknowledges it; dropping the channel
/// rejects the command.
type CommandRequest = (Command, mpsc::Sender<()>);

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
        tracks: canned_tracks().into(),
        artists: vec![SearchArtist {
            locator: "spotify:artist:elo".to_string(),
            name: "Electric Light Orchestra".to_string(),
        }]
        .into(),
        albums: vec![SearchAlbum {
            locator: "spotify:album:outoftheblue".to_string(),
            name: "Out of the Blue".to_string(),
            artists: vec!["Electric Light Orchestra".to_string()],
        }]
        .into(),
        playlists: vec![SearchPlaylist {
            locator: "spotify:playlist:bluesky".to_string(),
            name: "Blue Sky Mix".to_string(),
            owner: "Rust Player".to_string(),
            track_count: 24,
        }]
        .into(),
    }
}

/// Canned rows per library section; the scripted library is static.
fn canned_library(section: LibrarySection) -> Vec<LibraryEntry> {
    match section {
        LibrarySection::LikedSongs => canned_tracks()
            .iter()
            .cloned()
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
            .map(|playable| LibraryEntry::Track { playable })
            .collect(),
        LibrarySection::RecentlyPlayed => canned_tracks()
            .into_iter()
            .map(|playable| LibraryEntry::Track { playable })
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

/// The scripted fake. One worker thread folds commands into the published
/// snapshot; a search or browse shows `Loading` for [`SEARCH_DELAY`] before
/// resolving to canned rows.
pub struct FakeRuntime {
    tx: watch::Sender<Snapshot>,
    commands: Mutex<Option<mpsc::Sender<CommandRequest>>>,
}

impl FakeRuntime {
    pub fn new() -> Self {
        // Land in Ready immediately: the fake has no sign-in step.
        let (tx, _rx) = watch::channel(Snapshot {
            login: LoginState::Ready,
            audio: AudioState::Ready,
            ..Snapshot::default()
        });
        let (commands, command_rx) = mpsc::channel::<CommandRequest>();
        let worker_tx = tx.clone();
        std::thread::Builder::new()
            .name("player-fake-runtime".into())
            .spawn(move || {
                let mut next_revision = 1;
                for (command, reply) in command_rx {
                    apply(&worker_tx, &mut next_revision, command, reply);
                }
            })
            .expect("spawn fake runtime thread");

        FakeRuntime {
            tx,
            commands: Mutex::new(Some(commands)),
        }
    }
}

impl Default for FakeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// Publish only when `change` actually changed the snapshot.
fn update(tx: &watch::Sender<Snapshot>, change: impl FnOnce(&mut Snapshot)) {
    tx.send_if_modified(|snapshot| {
        let before = snapshot.clone();
        change(snapshot);
        *snapshot != before
    });
}

fn apply(
    tx: &watch::Sender<Snapshot>,
    next_revision: &mut u64,
    command: Command,
    reply: mpsc::Sender<()>,
) {
    let mut revision = || {
        *next_revision += 1;
        CatalogRevision::new(*next_revision - 1)
    };
    match command {
        Command::Search(query) => {
            update(tx, |snap| {
                snap.search = SearchState::Loading {
                    query: query.clone(),
                }
            });
            // A command is acknowledged once its loading fact is folded. The
            // eventual Done fact is an independent worker completion.
            let _ = reply.send(());
            std::thread::sleep(SEARCH_DELAY);
            let needle = query.to_lowercase();
            let tracks: Vec<Playable> = canned_tracks()
                .into_iter()
                .filter(|p| {
                    needle.is_empty()
                        || p.title.to_lowercase().contains(&needle)
                        || p.artists_display().to_lowercase().contains(&needle)
                })
                .collect();
            let results = if tracks.is_empty() {
                SearchResults::default()
            } else {
                SearchResults {
                    tracks: tracks.into(),
                    ..canned_results()
                }
            };
            let done = SearchState::Done {
                query,
                revision: revision(),
                results,
            };
            update(tx, |snap| snap.search = done);
        }
        Command::Browse(section) => {
            update(tx, |snap| snap.library = LibraryState::Loading { section });
            let _ = reply.send(());
            std::thread::sleep(SEARCH_DELAY);
            let done = LibraryState::Done {
                section,
                revision: revision(),
                entries: canned_library(section).into(),
            };
            update(tx, |snap| snap.library = done);
        }
        Command::PlayFromList { ref list, index } if index >= list.tracks.len() => {}
        command => {
            update(tx, |snap| fold(snap, command));
            let _ = reply.send(());
        }
    }
}

fn fold(snap: &mut Snapshot, command: Command) {
    match command {
        Command::Search(_)
        | Command::Browse(_)
        | Command::OpenSearchTarget(_)
        | Command::SubmitPastedLoginUrl(_)
        | Command::Reauthenticate => {}
        Command::Play(playable) => {
            snap.implicit_queue = None;
            start_playback(snap, playable);
        }
        Command::PlayFromList { list, index } => {
            let mut list = (*list).clone();
            if let Some(playable) = list.tracks.get(index).cloned() {
                list.current_index = index;
                snap.implicit_queue = Some(list);
                start_playback(snap, playable);
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
                start_playback(snap, next);
            } else if let Some(next) = advance_implicit_queue(snap) {
                start_playback(snap, next);
            }
        }
        Command::Previous => {
            let previous = snap.implicit_queue.as_mut().and_then(|list| {
                let previous_index = list.current_index.checked_sub(1)?;
                list.current_index = previous_index;
                list.tracks.get(previous_index).cloned()
            });
            if let Some(previous) = previous {
                start_playback(snap, previous);
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

impl crate::Runtime for FakeRuntime {
    fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.tx.subscribe()
    }

    fn command(&self, command: Command) -> bool {
        let (reply, acknowledged) = mpsc::channel();
        let sent = self
            .commands
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|commands| commands.send((command, reply)).is_ok());
        sent && acknowledged.recv().is_ok()
    }

    fn shutdown(&self) {
        // Dropping the sender ends the worker thread.
        self.commands.lock().unwrap().take();
    }
}
