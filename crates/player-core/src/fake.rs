//! A scripted [`Runtime`](crate::Runtime) with no credentials and no audio
//! hardware. It answers every command the window can send, so UI behavior
//! is exercisable end to end.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use crate::{AudioState, Command, LoginState, Playable, PlaybackStatus, SearchState, Snapshot};

/// How long a scripted search stays in `Loading` so the state is visible.
const SEARCH_DELAY: Duration = Duration::from_millis(150);

fn canned_results() -> Vec<Playable> {
    vec![
        Playable {
            source: crate::Source::Spotify,
            locator: "spotify:track:4uLU6hMCjMI75M1A2tKUQC".to_string(),
            title: "Mr. Blue Sky".to_string(),
            artists: vec!["Electric Light Orchestra".to_string()],
            album: "Out of the Blue".to_string(),
            duration_ms: 302_000,
        },
        Playable {
            source: crate::Source::Spotify,
            locator: "spotify:track:6Y2SdcH4DoWU1RdBFRxNPL".to_string(),
            title: "Dreamweaver".to_string(),
            artists: vec!["The Scripted Fake".to_string()],
            album: "Test Signals".to_string(),
            duration_ms: 214_000,
        },
    ]
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
        let results = canned_results()
            .into_iter()
            .filter(|p| {
                needle.is_empty()
                    || p.title.to_lowercase().contains(&needle)
                    || p.artists_display().to_lowercase().contains(&needle)
            })
            .collect();
        state.lock().unwrap().search = SearchState::Done { query, results };
        publish(state, tx);
        return;
    }

    let mut snap = state.lock().unwrap();
    match command {
        Command::Search(_) => unreachable!("handled above"),
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
