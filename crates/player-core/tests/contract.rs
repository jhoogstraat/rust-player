use std::time::{Duration, Instant};

use player_core::{Playable, PlaybackDevice, PlaybackStatus, Source, project_position};

fn status(is_playing: bool) -> PlaybackStatus {
    PlaybackStatus {
        playable: Playable {
            source: Source::Spotify,
            locator: "spotify:track:4uLU6hMCjMI75M1A2tKUQC".to_string(),
            title: "Test".to_string(),
            artists: vec!["Artist".to_string()],
            album: "Album".to_string(),
            duration_ms: 200_000,
        },
        device: PlaybackDevice::Native,
        is_playing,
        position_ms: 10_000,
        observed_at: Instant::now(),
        volume_percent: Some(70),
    }
}

#[test]
fn projection_is_frozen_while_paused() {
    let status = status(false);
    let later = status.observed_at + Duration::from_secs(5);
    assert_eq!(project_position(&status, later), 10_000);
}

#[test]
fn projection_advances_clamps_to_duration_and_never_regresses() {
    let mut status = status(true);
    let observed_at = status.observed_at;
    assert_eq!(
        project_position(&status, observed_at + Duration::from_millis(250)),
        10_250
    );

    status.playable.duration_ms = 10_500;
    assert_eq!(
        project_position(&status, observed_at + Duration::from_secs(60)),
        10_500
    );

    let earlier = observed_at.checked_sub(Duration::from_millis(500)).unwrap();
    assert_eq!(project_position(&status, earlier), 10_000);
}
