# Manual Smoke Test — Rust Player on macOS

One documented manual pass with a real Premium account. Run it after
`scripts/package_app.sh` (or `cargo run --release`) whenever a release is cut.
CI never verifies audible output; this document is the only place that does.

## Setup

- A macOS user account **without** Homebrew PortAudio in its runtime path
  (the bundle carries its own copy).
- Spotify Premium credentials available in the browser.

## The pass

1. **Launch.** Double-click `Rust Player.app`. A window opens; no terminal
   interaction occurs at any point.
2. **Sign-in (first run only).** The window explains sign-in while two browser
   consents complete (Web API PKCE, then librespot streaming). If the callback
   listener could not bind, the window shows a paste field — paste the
   redirect URL and press Enter. Relaunching needs neither consent again.
3. **Search.** Type a query, press Enter: loading appears, then rows with
   title, artists, album, and duration. A second search visibly replaces the
   first results.
4. **Play.** Click a row: audio starts locally within a couple of seconds.
   The now-playing bar shows title/artists and progress advances smoothly and
   matches the Spotify app.
5. **Transport.** Pause, resume, next, previous, volume up/down all reflect
   in the bar via the next snapshot. Space toggles playback when no text
   field has focus; ⌘→ / ⌘← skip; ⌘↑ / ⌘↓ change volume.
6. **Queue.** “+ Queue” on several rows adds without interrupting playback;
   the panel lists them; ↑ ↓ move items; ✕ removes one; Clear upcoming leaves
   the active track playing and empties the panel.
7. **Keyboard.** Tab reaches every control; ⌘F focuses search; ⌘K toggles the
   queue panel.
8. **Close & reopen.** Close the window: audio keeps playing and the process
   stays alive. Click the dock icon: the window returns around the same state.
9. **Quit.** ⌘Q stops playback and exits cleanly. Relaunch restores state
   without re-authenticating.
10. **Offline catalog (optional but encouraged).** Pull the network mid-track:
    audio continues; a search attempt fails with a visible Retry that clears
    on success once the network returns.
11. **Logs.** `~/Library/Application Support/rust-player/logs/player.log`
    exists, rotates, and contains no tokens or OAuth codes.

## Recording results

Note the date, the app version (`CFBundleShortVersionString`), and any step
that failed. A release gate passes only when every step above passes.
