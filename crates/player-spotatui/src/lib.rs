//! The adapter that maps the source-neutral contract onto the Spotatui
//! fork's `frontend` module. It contains no playback logic: commands become
//! fork `Action`s, and contract snapshots are composed from the fork's
//! published snapshot. The application crate never imports the fork.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::watch;

use player_core::{
    AudioState, Command, LoginState, Notice, Playable, PlaybackStatus, Runtime, SearchState,
    Snapshot, Source,
};
use spotatui::frontend::{self, Action, Onboarding};

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
struct BootOnboarding {
    login_tx: watch::Sender<Snapshot>,
    pasted_url_rx: Mutex<Option<std::sync::mpsc::Receiver<String>>>,
}

impl BootOnboarding {
    fn in_progress(&self, message: String, wants_pasted_url: bool) {
        let _ = self.login_tx.send_if_modified(|current| {
            let next = LoginState::InProgress {
                message,
                wants_pasted_url,
            };
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
        self.in_progress(prompt.to_string(), true);
        let rx = self
            .pasted_url_rx
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| anyhow::anyhow!("paste-URL prompt already answered"))?;
        match rx.recv() {
            Ok(url) => Ok(format!("{url}\n")),
            Err(_) => Err(anyhow::anyhow!("sign-in cancelled")),
        }
    }

    fn pick_sources(
        &self,
        _options: &[frontend::Source],
    ) -> anyhow::Result<Option<Vec<frontend::Source>>> {
        // Embedded boots never run the first-run picker.
        Ok(None)
    }
}

struct StageShared {
    frontend: Mutex<Option<frontend::Runtime>>,
    last_query: Mutex<Option<String>>,
}

pub struct SpotatuiPlayer {
    tx: watch::Sender<Snapshot>,
    stage: Mutex<Option<Arc<StageShared>>>,
    /// Delivers the manual redirect-URL paste to the blocked boot prompt.
    paste_url_tx: Mutex<Option<std::sync::mpsc::Sender<String>>>,
}

/// Connect to the real runtime. Returns immediately; sign-in progress flows
/// through the returned channel while `boot` runs on a dedicated blocking
/// thread (its `Onboarding` is synchronous).
pub fn connect(options: ConnectOptions) -> Arc<SpotatuiPlayer> {
    let (tx, _rx) = watch::channel(Snapshot::default());
    let (paste_url_tx, paste_url_rx) = std::sync::mpsc::channel::<String>();
    let player = Arc::new(SpotatuiPlayer {
        tx,
        stage: Mutex::new(None),
        paste_url_tx: Mutex::new(Some(paste_url_tx)),
    });

    let login_tx = player.tx.clone();
    let onboarding: Arc<dyn Onboarding> = Arc::new(BootOnboarding {
        login_tx,
        pasted_url_rx: Mutex::new(Some(paste_url_rx)),
    });

    let player_for_boot = Arc::downgrade(&player);
    let tx_for_boot = player.tx.clone();
    std::thread::Builder::new()
        .name("player-boot".into())
        .spawn(move || {
            frontend::Runtime::install_panic_hook();
            match frontend::Runtime::boot(frontend::Options::new(options.data_root), onboarding) {
                Ok(runtime) => {
                    let runtime_handle = runtime.handle();
                    let shared = Arc::new(StageShared {
                        frontend: Mutex::new(Some(runtime)),
                        last_query: Mutex::new(None),
                    });
                    if let Some(player) = player_for_boot.upgrade() {
                        *player.stage.lock().unwrap() = Some(Arc::clone(&shared));
                    }
                    // Relay on the runtime's own reactor: map every published fork
                    // snapshot into the contract.
                    let relay_tx = tx_for_boot.clone();
                    let relay_shared = Arc::clone(&shared);
                    let mut rx = shared
                        .frontend
                        .lock()
                        .unwrap()
                        .as_ref()
                        .expect("runtime just stored")
                        .subscribe();
                    runtime_handle.spawn(async move {
                        loop {
                            if rx.changed().await.is_err() {
                                break;
                            }
                            let fork_snapshot = rx.borrow_and_update().clone();
                            let query = relay_shared.last_query.lock().unwrap().clone();
                            let next = map_snapshot(&fork_snapshot, query.as_deref());
                            let _ = relay_tx.send_if_modified(|current| {
                                if *current == next {
                                    false
                                } else {
                                    *current = next;
                                    true
                                }
                            });
                        }
                    });
                }
                Err(error) => {
                    log::error!("[boot] runtime boot failed: {error:#}");
                    let _ = tx_for_boot.send_if_modified(|current| {
                        let next = Snapshot {
                            login: LoginState::Expired {
                                message: format!("Could not start Spotify: {error:#}"),
                            },
                            audio: AudioState::Unavailable {
                                message: "Playback engine unavailable".to_string(),
                            },
                            ..Snapshot::default()
                        };
                        if *current == next {
                            false
                        } else {
                            *current = next;
                            true
                        }
                    });
                }
            }
        })
        .expect("spawn boot thread");

    player
}

impl SpotatuiPlayer {
    fn with_frontend<T>(&self, f: impl FnOnce(&frontend::Runtime) -> T) -> Option<T> {
        let stage = self.stage.lock().unwrap();
        let guard = stage.as_ref()?.frontend.lock().unwrap();
        guard.as_ref().map(f)
    }

    fn remember_query(&self, query: &str) {
        if let Some(stage) = self.stage.lock().unwrap().as_ref() {
            *stage.last_query.lock().unwrap() = Some(query.to_string());
        }
    }
}

impl Runtime for SpotatuiPlayer {
    fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.tx.subscribe()
    }

    fn command(&self, command: Command) -> bool {
        match command {
            Command::SubmitPastedLoginUrl(url) => self
                .paste_url_tx
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|tx| tx.send(url).ok())
                .is_some(),
            Command::Search(query) => {
                self.remember_query(&query);
                self.with_frontend(|runtime| runtime.apply(Action::SearchActiveSource(query)))
                    .is_some()
            }
            other => self
                .with_frontend(|runtime| dispatch_command(runtime, other))
                .is_some(),
        }
    }

    fn shutdown(&self) {
        if let Some(stage) = self.stage.lock().unwrap().take() {
            let frontend = stage.frontend.lock().unwrap().take();
            if let Some(runtime) = frontend {
                let _ = runtime.shutdown();
            }
        }
    }
}

fn dispatch_command(runtime: &frontend::Runtime, command: Command) {
    let action = match command {
        Command::Reauthenticate => Action::BeginSpotifyLogin,
        Command::Play(playable) => Action::PlayUris {
            uris: vec![playable.locator],
            offset: None,
        },
        Command::Pause => Action::Pause,
        Command::Resume => Action::Play,
        Command::Seek(position_ms) => {
            Action::SeekTo(u32::try_from(position_ms).unwrap_or(u32::MAX))
        }
        Command::Next => Action::NextTrack,
        Command::Previous => Action::PreviousTrack,
        Command::SetVolume(percent) => Action::SetVolume(percent.min(100)),
        Command::Enqueue(playable) => Action::EnqueueNative(track_info(&playable)),
        Command::RemoveQueued(index) => Action::RemoveNativeQueued(index),
        Command::MoveQueued { index, up } => Action::MoveNativeQueued { index, up },
        Command::ClearQueue => Action::ClearNativeQueue,
        // The fork has no dedicated dismiss: an empty zero-TTL notify expires on
        // the next tick, which is exactly a dismissal. Empty notices are filtered
        // out of mapped snapshots.
        Command::DismissNotice => Action::Notify(String::new(), 0),
        Command::Search(_) | Command::SubmitPastedLoginUrl(_) => return,
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

fn map_snapshot(fork: &frontend::Snapshot, last_query: Option<&str>) -> Snapshot {
    let now = Instant::now();

    let login = if fork.spotify_connected {
        LoginState::Ready
    } else if fork.notice_is_error {
        LoginState::Expired {
            message: fork
                .notice
                .clone()
                .unwrap_or_else(|| "Session expired".into()),
        }
    } else {
        LoginState::InProgress {
            message: fork.notice.clone().unwrap_or_else(|| "Connecting…".into()),
            wants_pasted_url: false,
        }
    };

    let search = if fork.search_loading {
        SearchState::Loading {
            query: last_query.unwrap_or_default().to_string(),
        }
    } else if !fork.search_tracks.is_empty() {
        SearchState::Done {
            query: last_query.unwrap_or_default().to_string(),
            results: fork
                .search_tracks
                .iter()
                .filter_map(playable_from_track)
                .collect(),
        }
    } else if fork.notice_is_error && last_query.is_some() && fork.notice.is_some() {
        SearchState::Failed {
            query: last_query.unwrap().to_string(),
            message: fork.notice.clone().unwrap(),
        }
    } else {
        SearchState::Idle
    };

    let playback = fork.playback.as_ref().and_then(|state| {
        Some(PlaybackStatus {
            playable: playable_from_track(state.track.as_ref()?)?,
            is_playing: state.is_playing,
            position_ms: fork.position_ms.unwrap_or(state.progress_ms),
            observed_at: now,
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
            message: fork.notice.clone().unwrap_or_else(|| {
                "Native audio unavailable. Browsing still works; restart to retry.".to_string()
            }),
        }
    };

    let notice = fork
        .notice
        .as_ref()
        .filter(|message| !message.trim().is_empty())
        .map(|message| Notice {
            message: message.clone(),
            dismissible: true,
        });

    Snapshot {
        login,
        search,
        playback,
        queue,
        audio,
        notice,
    }
}
