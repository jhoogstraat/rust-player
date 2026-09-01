use std::sync::Arc;
use std::time::{Duration, Instant};

use player_core::{
    AudioState, Command, LibraryEntry, LibrarySection, LibraryState, LoginState, Playable,
    PlaybackList, PlaybackListSource, PlaybackStatus, Runtime, SearchState, Snapshot, Source,
    project_position,
};

/// The contract crate itself stays runtime-free; tests drive async watch
/// APIs through a throwaway single-thread reactor.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

fn playable() -> Playable {
    Playable {
        source: Source::Spotify,
        locator: "spotify:track:4uLU6hMCjMI75M1A2tKUQC".to_string(),
        title: "Test".to_string(),
        artists: vec!["Artist".to_string()],
        album: "Album".to_string(),
        duration_ms: 200_000,
    }
}

fn status(is_playing: bool) -> PlaybackStatus {
    PlaybackStatus {
        playable: playable(),
        is_playing,
        position_ms: 10_000,
        observed_at: Instant::now(),
        volume_percent: Some(70),
    }
}

#[test]
fn projection_advances_from_position_and_observed_at_while_playing() {
    let mut status = status(true);
    status.position_ms = 30_000;
    status.observed_at = Instant::now();

    std::thread::sleep(Duration::from_millis(60));
    let now = Instant::now();
    std::thread::sleep(Duration::from_millis(20));
    let later = Instant::now();
    let at_now = project_position(&status, now);
    let at_later = project_position(&status, later);

    assert!(
        (50..=120).contains(&(at_now - status.position_ms)),
        "projected delta {}",
        at_now - status.position_ms
    );
    assert!(at_later >= at_now + 15, "advances with time: {at_later}");
}

#[test]
fn projection_is_frozen_while_paused() {
    let status = status(false);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(project_position(&status, Instant::now()), 10_000);
}

#[test]
fn projection_clamps_to_duration_and_never_regresses() {
    let mut status = status(true);
    status.playable.duration_ms = 10_500;
    status.position_ms = 10_400;
    status.observed_at = Instant::now().checked_sub(Duration::from_secs(60)).unwrap();
    assert_eq!(project_position(&status, Instant::now()), 10_500);

    status.playable.duration_ms = 200_000;
    let earlier = status.observed_at - Duration::from_millis(500);
    assert_eq!(project_position(&status, earlier), 10_400);
}

#[test]
fn default_snapshot_starts_in_progress_and_starting() {
    let snap = Snapshot::default();
    assert_eq!(
        snap.login,
        LoginState::InProgress {
            message: "Starting…".to_string(),
            wants_pasted_url: false
        }
    );
    assert_eq!(snap.audio, AudioState::Starting);
    assert!(snap.projected_position_ms(Instant::now()).is_none());
}

#[test]
fn fake_runtime_answers_commands_with_snapshots() {
    block_on(async {
        let runtime = player_core::fake::FakeRuntime::new();
        let mut rx = runtime.subscribe();
        assert_eq!(rx.borrow().login, LoginState::Ready);

        assert!(runtime.command(Command::Search("blue".to_string())));
        // Loading is published first and stays visible long enough to see.
        let loading = rx
            .wait_for(|snap| matches!(snap.search, SearchState::Loading { .. }))
            .await
            .unwrap();
        assert_eq!(
            loading.search,
            SearchState::Loading {
                query: "blue".to_string()
            }
        );
        drop(loading);
        let done = rx
            .wait_for(|snap| matches!(snap.search, SearchState::Done { .. }))
            .await
            .unwrap();
        match &done.search {
            SearchState::Done { results, .. } => {
                assert_eq!(results.tracks.len(), 1);
                assert_eq!(results.tracks[0].title, "Mr. Blue Sky");
                assert_eq!(results.artists.len(), 1);
                assert_eq!(results.albums.len(), 1);
                assert_eq!(results.playlists.len(), 1);
            }
            other => panic!("expected Done, got {other:?}"),
        }
        drop(done);

        runtime.command(Command::Play(playable()));
        rx.changed().await.unwrap();
        let snap = rx.borrow().clone();
        assert!(snap.is_playing());
        assert_eq!(snap.playback.as_ref().unwrap().volume_percent, Some(80));

        runtime.command(Command::Enqueue(playable()));
        rx.changed().await.unwrap();
        assert_eq!(rx.borrow().queue.len(), 1);
        runtime.command(Command::MoveQueued {
            index: 0,
            up: false,
        });
        runtime.command(Command::RemoveQueued(0));
        rx.changed().await.unwrap();
        assert!(rx.borrow().queue.is_empty());

        runtime.shutdown();
    });
}

#[test]
fn fake_runtime_prioritizes_explicit_queue_then_resumes_implicit_list() {
    block_on(async {
        let runtime = player_core::fake::FakeRuntime::new();
        let mut rx = runtime.subscribe();
        let first = playable();
        let mut second = playable();
        second.locator = "spotify:track:second".to_string();
        second.title = "Second".to_string();
        let mut queued = playable();
        queued.locator = "spotify:track:queued".to_string();
        queued.title = "Queued".to_string();

        let list = PlaybackList {
            source: PlaybackListSource::LikedSongs,
            tracks: vec![first.clone(), second.clone()].into(),
            current_index: 0,
        };
        assert!(runtime.command(Command::PlayFromList {
            list: Arc::new(list),
            index: 0,
        }));
        rx.wait_for(|snap| snap.playback.as_ref().map(|p| &p.playable) == Some(&first))
            .await
            .unwrap();

        assert!(runtime.command(Command::Enqueue(queued.clone())));
        rx.wait_for(|snap| snap.queue == [queued.clone()])
            .await
            .unwrap();

        assert!(runtime.command(Command::Next));
        rx.wait_for(|snap| snap.playback.as_ref().map(|p| &p.playable) == Some(&queued))
            .await
            .unwrap();
        assert_eq!(
            rx.borrow().implicit_queue.as_ref().unwrap().current_index,
            0
        );

        assert!(runtime.command(Command::Next));
        rx.wait_for(|snap| snap.playback.as_ref().map(|p| &p.playable) == Some(&second))
            .await
            .unwrap();
        let snapshot = rx.borrow().clone();
        assert!(snapshot.queue.is_empty());
        assert_eq!(snapshot.implicit_queue.unwrap().current_index, 1);

        runtime.shutdown();
    });
}

#[test]
fn search_without_hits_still_reports_done_with_empty_results() {
    block_on(async {
        let runtime = player_core::fake::FakeRuntime::new();
        let mut rx = runtime.subscribe();

        runtime.command(Command::Search("zzzz-no-match".to_string()));
        let done = rx
            .wait_for(|snap| matches!(snap.search, SearchState::Done { .. }))
            .await
            .unwrap();
        match &done.search {
            SearchState::Done { results, .. } => {
                assert!(results.tracks.is_empty());
                assert!(results.artists.is_empty());
                assert!(results.albums.is_empty());
                assert!(results.playlists.is_empty());
            }
            other => panic!("expected Done, got {other:?}"),
        }
        drop(done);
        runtime.shutdown();
    });
}

#[test]
fn fake_runtime_browses_every_library_section() {
    block_on(async {
        let runtime = player_core::fake::FakeRuntime::new();
        let mut rx = runtime.subscribe();
        assert_eq!(rx.borrow().library, LibraryState::Idle);

        for section in [
            LibrarySection::LikedSongs,
            LibrarySection::RecentlyPlayed,
            LibrarySection::Playlists,
        ] {
            assert!(runtime.command(Command::Browse(section)));
            let loading = rx
                .wait_for(|snap| {
                    matches!(snap.library, LibraryState::Loading { section: s } if s == section)
                })
                .await
                .unwrap();
            assert_eq!(loading.library, LibraryState::Loading { section });
            drop(loading);
            let done = rx
                .wait_for(|snap| {
                    matches!(snap.library, LibraryState::Done { section: s, .. } if s == section)
                })
                .await
                .unwrap();
            match &done.library {
                LibraryState::Done {
                    section: done_section,
                    entries,
                } => {
                    assert_eq!(*done_section, section);
                    assert!(!entries.is_empty(), "section {section:?}");
                    // Track sections carry Playables; the playlists section
                    // carries playlist rows.
                    if section == LibrarySection::Playlists {
                        assert!(
                            entries
                                .iter()
                                .all(|e| matches!(e, LibraryEntry::Playlist { .. }))
                        );
                    } else {
                        assert!(
                            entries
                                .iter()
                                .all(|e| matches!(e, LibraryEntry::Track { .. }))
                        );
                    }
                }
                other => panic!("expected Done for {section:?}, got {other:?}"),
            }
            drop(done);
        }
        runtime.shutdown();
    });
}
