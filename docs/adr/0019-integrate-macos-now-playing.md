# Integrate macOS Now Playing through the Playback Engine

Rust Player must appear in macOS's Now Playing interface so listeners can see
the active Playable and control Native Playback from the menu bar, Control
Center, media keys, and compatible accessories.

The pinned Spotatui fork already contains a `macos-media` adapter built on
Apple's MediaPlayer framework. It publishes metadata through
`MPNowPlayingInfoCenter` and routes play, pause, previous, next, and stop
events into the Playback Engine.

## Decision

Enable the fork's `macos-media` feature alongside `streaming`. Keep ownership
in the Playback Engine and its Event Spine:

```text
macOS MediaPlayer -> fork adapter -> Event Spine -> Playback Engine -> Snapshot
```

The application does not add a second macOS transport adapter, and
`player-core` does not gain OS-specific commands. Existing Queue, Implicit
Playback List, and transport rules remain authoritative in the engine.

The integration is runtime-owned rather than window-owned, so closing the
final GPUI window leaves Native Playback and macOS controls active. Metadata
comes from the active Playback Session and is cleared when playback stops or
the runtime shuts down. Remote Playback is not advertised.

## Compatibility gate

The adapter does not create or drive an `NSApplication`. Remote-command
registration, MediaPlayer updates, and `NSImage` construction are submitted to
the host process's main dispatch queue, which GPUI also uses. Artwork network
I/O and command sequencing stay off that queue. The adapter's worker is woken
and joined during teardown so a failed boot or re-auth retry cannot leave a
detached AppKit worker behind. No duplicate `NSApplication` or playback owner
is permitted.

## Consequences

- macOS builds gain the fork's MediaPlayer dependencies; other targets remain
  target-gated by the fork.
- The system surface receives title, artists, album, duration, position, and
  playing/paused state; Cover Art remains optional.
- Verification requires a manual macOS smoke test because CI cannot prove
  audible output or Control Center behavior.
