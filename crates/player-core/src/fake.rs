//! A scripted [`Runtime`](crate::Runtime) with no credentials and no audio
//! hardware. It answers every command the window can send, so UI behavior
//! is exercisable end to end.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use crate::{AudioState, Command, LoginState, Playable, PlaybackStatus, SearchState, Snapshot};

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

struct Inner {
    snapshot: Snapshot,
}

/// The scripted fake. Commands mutate a plain state struct and republish;
/// search resolves to canned results after a short delay so loading state
/// is visible.
pub struct FakeRuntime {
    tx: watch::Sender<Snapshot>,
    commands: mpsc::Sender<Command>,
    inner: Arc<Mutex<Inner>>,
}

impl FakeRuntime {
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(Snapshot::default());
        let (commands, command_rx) = mpsc::channel::<Command>();
        let inner = Arc::new(Mutex::new(Inner {
            snapshot: Snapshot::default(),
        }));

        let publisher_inner = Arc::clone(&inner);
        let publisher_tx = tx.clone();
        std::thread::Builder::new()
            .name("player-fake-runtime".into())
            .spawn(move || {
                loop {
                    match command_rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(command) => apply(&publisher_inner, &publisher_tx, command),
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    publish(&publisher_inner, &publisher_tx);
                }
            })
            .expect("spawn fake runtime thread");

        // Land in Ready immediately: the fake has no sign-in step.
        {
            let mut state = inner.lock().unwrap();
            state.snapshot.login = LoginState::Ready;
            state.snapshot.audio = AudioState::Ready;
        }
        publish(&inner, &tx);

        FakeRuntime {
            tx,
            commands,
            inner,
        }
    }

    /// Force one exact snapshot (scripted scenarios for failure states).
    pub fn script_snapshot(&self, snapshot: Snapshot) {
        let mut state = self.inner.lock().unwrap();
        state.snapshot = snapshot;
        drop(state);
        publish(&self.inner, &self.tx);
    }
}

impl Default for FakeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for FakeRuntime {
    fn drop(&mut self) {
        // Dropping the sender ends the publisher thread's recv_timeout loop.
        let _ = self.commands.send(Command::DismissNotice).is_ok();
    }
}

fn apply(inner: &Mutex<Inner>, tx: &watch::Sender<Snapshot>, command: Command) {
    let mut state = inner.lock().unwrap();
    let snap = &mut state.snapshot;
    match command {
        Command::SubmitPastedLoginUrl(_) | Command::Reauthenticate => {
            snap.login = LoginState::Ready;
        }
        Command::Search(query) => {
            snap.search = SearchState::Loading { query };
            // Resolve below, after the lock is released.
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
            if index >= len {
                return;
            }
            if up && index > 0 {
                snap.queue.swap(index - 1, index);
            } else if !up && index + 1 < len {
                snap.queue.swap(index, index + 1);
            }
        }
        Command::ClearQueue => snap.queue.clear(),
        Command::DismissNotice => snap.notice = None,
    }

    // A search resolves shortly after it starts; do it inline here (the
    // publisher thread sleeps on its next poll anyway).
    if let SearchState::Loading { query } = snap.search.clone() {
        let results = canned_results()
            .into_iter()
            .filter(|p| {
                query.is_empty()
                    || p.title.to_lowercase().contains(&query.to_lowercase())
                    || p.artists_display()
                        .to_lowercase()
                        .contains(&query.to_lowercase())
            })
            .collect::<Vec<_>>();
        snap.search = SearchState::Done { query, results };
    }

    drop(state);
    publish(inner, tx);
}

fn publish(inner: &Mutex<Inner>, tx: &watch::Sender<Snapshot>) {
    let snapshot = inner.lock().unwrap().snapshot.clone();
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

    fn shutdown(self) {
        // Dropping the command sender stops the publisher thread.
    }
}
