# Native Music Player Implementation Plan

This is the authoritative specification for version one. Domain language is
defined by the project glossary, and accepted architectural decisions remain
authoritative where this document omits their rationale. Claims about
Spotatui below were checked against `2b8a238` (0.41.0) and claims about Comet
against `cd6cb22`; file references point at those revisions.

## Problem Statement

The listener wants a small native music application written in Rust that can
authenticate with Spotify, search its catalog, and produce audio locally
without requiring Spotify's desktop client or a terminal interface. Existing
projects already solve the difficult halves: Comet demonstrates a polished
GPUI application structure, and Spotatui implements Spotify authentication,
catalog access, playback policy, and resilient native streaming. Neither can
be consumed unchanged without either importing unrelated application domains
or exposing private runtime state.

The first release must prove that these halves can be combined with little new
code while preserving a clean boundary for later Music Sources. It must not
become a rewrite of Spotatui, a copy of Comet, or a speculative plugin system.

## Solution

Build a macOS-first GPUI application around one in-process runtime. A pinned
private Spotatui fork owns Spotify authentication, Web API behavior, the
native cross-source queue, playback routing, and native audio through its
maintained librespot stack. The fork gains one narrow `frontend` module: an
opaque runtime handle that boots, publishes snapshots, applies its existing
`Action` vocabulary, ticks its existing driver, and shuts down. It gains
nothing else.

This repository owns the product: a small source-neutral contract
(`player-core`: commands, snapshot, runtime trait, fake), a thin adapter that
maps that contract onto the fork's `frontend` module (`player-spotatui`), and
the GPUI application (`apps/player`). GPUI observes immutable snapshots and
sends commands. It never sees Spotify, librespot, or Spotatui types.

Spotatui is already source-neutral inside: its `Action` enum, `plugin_api`
snapshots, `Source` enum, and native queue are shared by five sources. The
product contract therefore stays deliberately thin. It describes the Playback
Session; it does not implement a second one. A future experimental YouTube
Source is a separate version-two milestone.

## User Stories

1. As a first-time listener, I want to sign in to Spotify through my browser with the application window explaining each step, so that I never need a terminal to configure the application.
2. As a returning listener, I want my valid Spotify session restored, so that I do not authenticate on every launch.
3. As a listener whose credentials expired, I want a clear reauthentication action, so that I can recover without editing files.
4. As a listener, I want to search Spotify by text, so that I can find music to play.
5. As a listener, I want search results to identify title, artist, album, and duration, so that I can choose the intended Playable.
6. As a listener, I want stale searches replaced visibly by loading or error state, so that I never mistake old results for a current response.
7. As a listener, I want to start Native Playback from a search result, so that this application produces the audio itself.
8. As a listener, I want the active Playable displayed persistently, so that I know what is playing while browsing.
9. As a listener, I want to see current playback progress and duration, so that I know where I am in the Playable.
10. As a listener, I want to pause and resume playback, so that I control when audio is produced.
11. As a listener, I want to seek to an absolute position, so that I can move within the active Playable.
12. As a listener, I want next and previous controls, so that I can navigate the Playback Session.
13. As a listener, I want to change playback volume, so that I can control application audio without changing the system volume.
14. As a listener, I want to add a search result to the Queue, so that it plays later without interrupting the active Playable.
15. As a listener, I want to see the active and upcoming Playables, so that I understand the Playback Session order.
16. As a listener, I want to remove an upcoming Playable, so that unwanted music does not start.
17. As a listener, I want to clear upcoming Playables without stopping the active one, so that I can reset future playback safely.
18. As a listener, I want to move an upcoming Playable up or down, so that I can change what plays next.
19. As a listener, I want a queued Playable that cannot play to be reported when it is skipped, so that the Queue never drains silently.
20. As a listener without network access, I want last-known playback metadata to remain visible, so that the window explains its current state.
21. As a listener without network access, I want search failures shown with a retry action, so that stale catalog data is not presented as current.
22. As a listener, I want existing audio to continue when the catalog becomes temporarily unavailable, so that an API failure does not unnecessarily interrupt playback.
23. As a listener whose audio engine cannot start, I want catalog browsing to remain available with an actionable playback diagnostic, so that the whole application does not appear broken.
24. As a listener, I want the system-default audio output used automatically, so that no output configuration is required.
25. As a listener, I want an output-device failure to pause playback with a visible message instead of crashing, so that I can resume once the device settles.
26. As a keyboard user, I want every interactive control reachable and activatable by keyboard, so that the application is usable without a pointer.
27. As a listener, I want fixed shortcuts for search, play or pause, next, previous, and volume, so that frequent controls are quick.
28. As a macOS listener, I want closing the final window to leave playback running, so that closing the window does not accidentally stop music.
29. As a macOS listener, I want clicking the dock icon to reopen the window, so that I can return to the running Playback Session.
30. As a listener, I want explicit Quit to stop playback and flush state cleanly, so that the application releases its resources predictably.
31. As a listener, I want this application to keep its state separate from Spotatui's TUI, so that running one cannot corrupt the other.
32. As a listener, I want useful errors with recovery actions, so that failures do not require reading developer logs.
33. As a support engineer, I want a rotating diagnostic log, so that native audio and authentication failures can be investigated.
34. As a privacy-conscious listener, I want secrets redacted from logs, so that diagnostics do not expose tokens, OAuth codes, or authorization headers.
35. As a macOS user, I want a self-contained application bundle, so that installing Homebrew libraries is not required to run the player.
36. As a developer, I want Spotify behavior to remain owned by the Spotatui fork, so that authentication and playback recovery are not reimplemented here.
37. As a developer, I want GPUI to consume source-neutral commands and snapshots, so that adding another Music Source does not rewrite the UI.
38. As a developer, I want one runtime contract shared by the real and fake runtimes, so that UI behavior can be exercised without Spotify credentials or audio hardware.
39. As a future listener, I want changing the Active Music Source to leave current playback untouched, so that browsing never becomes an implicit transport command.
40. As a future listener, I want a Queue capable of retaining Playables from different Music Sources, so that changing sources does not destroy playback intent.
41. As a future listener, I want the experimental YouTube Source named accurately, so that I do not expect YouTube Music authentication or library synchronization.

## Implementation Decisions

### Product boundaries

- Version one supports Spotify as the only advertised Music Source and Native Playback as the only playback mode.
- macOS is the first supported platform. Linux follows, then Windows; neither blocks the first release.
- The main window has search and results as its content, persistent now-playing controls, and a Queue panel that can be shown and hidden. The panel is a plain column, not a floating popover, so version one needs no overlay, blur, or fork-only GPUI primitive.
- Sign-in replaces the content area until the runtime is ready.
- Settings contain only a Spotify sign-out action and the log location. Audio output is always the system default and is not shown.
- Catalog failure never stops audio that can continue. Spotatui already keeps librespot running while Web API calls fail; the application must simply not tear anything down on an API error.
- A queued Playable that cannot play is skipped with a visible message, which is Spotatui's existing behavior (`infra/queue/dispatch.rs`, "Cannot play"). Pausing the Queue with Retry, Skip, and Remove actions is deferred until a real failure pattern justifies changing the fork's queue routing.

### Runtime and ownership

- The workspace has three members: `crates/player-core` (contract), `crates/player-spotatui` (adapter), and `apps/player` (GPUI application and entry point). `apps/player` never imports the fork; `player-core` never imports anything heavier than `tokio::sync`.
- The private fork adds one `frontend` module and changes as little as possible elsewhere. Its public surface is Spotatui-shaped: `Runtime::boot(Options, Arc<dyn Onboarding>)`, `Runtime::subscribe() -> watch::Receiver<Snapshot>`, `Runtime::apply(Action) -> ActionOutcome`, `Runtime::shutdown()`, plus re-exports of `Action`, `ActionOutcome`, `Onboarding`, and the `plugin_api` snapshot types it composes. `App`, `IoEvent`, `Network`, rspotify, and librespot types stay private.
- The adapter lives in this repository, not in the fork, so the fork never depends on this product's types. The adapter maps `player-core` commands to `Action` variants and composes `player-core` snapshots from the fork's snapshot. It contains no playback logic.
- The fork's `Runtime` owns every task the terminal runner used to own: the IoEvent pump (`runtime/pump.rs`), the deferred native streaming startup and recovery (`runtime/startup.rs`), and a Tokio tick loop that calls `Driver::tick` every 250 ms (`core::user_config::DEFAULT_TICK_RATE_MILLISECONDS`). The tick loop keeps running while the window is closed; it is where OAuth tokens refresh and playback advances.
- Concretely, `runtime/startup.rs::launch_ui` is split at its `runner::start_ui` call: everything before it becomes service startup shared by both frontends, and the terminal runner's tick and quit sequence (`driver.dispatch_startup`, `driver.tick`, `driver.on_quit`, session persistence, `close_io_channel`) becomes the `frontend` tick loop and `shutdown`. `bootstrap::boot` takes an options struct instead of clap matches.
- The application builds the fork with `default-features = false, features = ["streaming"]`. Every other default feature is off on purpose: `tui`, `telemetry`, `scripting`, `discord-rpc`, `mpris`, `macos-media`, `windows-media`, `audio-viz-cpal`, and above all `self-update`, which would silently replace the running binary on launch (`runtime/bootstrap.rs`, top of `boot`).
- The runtime is embedded in the GPUI process. There is no daemon, local RPC server, secondary viewport, or headless mode.
- Production consumes the private fork through a Git dependency pinned to an exact revision. A documented local path override is allowed during coordinated development. The fork is neither vendored nor a submodule.

### Contract

- A `Playable` is a `Source` identity plus an opaque source-owned locator (`spotify:track:…` in version one) and display metadata (title, artists, album, duration). Identity is the pair; metadata never defines it. The application does not deduplicate recordings across sources.
- The `Source` enum has one variant in version one. There is no capability vocabulary, source registry, or per-source trait: with one source every capability is always present, and a contract with no second implementor is speculation.
- The snapshot is one value type that derives `Clone` and `PartialEq`:
  - `login`: `InProgress { message, wants_pasted_url }`, `Ready`, or `Expired { message }`.
  - `search`: `Idle`, `Loading { query }`, `Done { query, results }`, or `Failed { query, message }`. Completed results group tracks, artists, albums, and playlists; only track rows are playable or queueable.
  - `playback`: `None`, or the active Playable with `is_playing`, `position_ms`, `observed_at`, and `volume_percent`.
  - `queue`: the upcoming Playables in order.
  - `audio`: `Ready`, `Starting`, or `Unavailable { message }`.
  - `notice`: the current error message, if any, with whether it can be dismissed.
  These six fields cover every user story above. Catalog Availability is visible as `search.Failed` plus `notice`; Playback Health is `audio`. The five-state source lifecycle, separate availability axes, and capability lists from the earlier draft are not needed to render the version-one window.
- Commands express intent: `SubmitPastedLoginUrl`, `Reauthenticate`, `Search`, `Play(Playable)`, `Pause`, `Resume`, `Seek`, `Next`, `Previous`, `SetVolume`, `Enqueue(Playable)`, `RemoveQueued(index)`, `MoveQueued { index, up }`, `ClearQueue`, and `DismissNotice`.
- Commands are accepted or rejected synchronously; they do not report completion. `App::apply` is infallible and most arms only dispatch an `IoEvent`; failures arrive later as `App::api_error` and surface in `snapshot.notice`. The UI shows no optimistic state. It renders snapshots and may show a pending indicator until the next one arrives.
- The fork publishes a new snapshot when the runtime changes. Because `App` has no change notification, the `frontend` module uses one `tokio::sync::Notify` poked after every `apply`, every pump iteration (one added line in `pump.rs`), and every tick; a publisher task builds the snapshot from the existing builders (`plugin_api::playback_state`, `queue_snapshot`, `search_results_snapshot`, `App::playback_position_ms`) and calls `watch::Sender::send_if_modified`. Equality dedups the rest.
- Progress snapshots carry an authoritative `position_ms` and `observed_at`. While playing, GPUI extrapolates the visible position locally between snapshots, following Comet's projection approach, so a 250 ms tick is smooth on screen.
- Sign-in state before the runtime exists comes from the adapter's `Onboarding` implementation: `info` and `progress` texts become `login.InProgress.message`, and `prompt_line` sets `wants_pasted_url` and blocks the boot thread until `SubmitPastedLoginUrl` arrives. `boot` runs on a dedicated blocking thread because `Onboarding` is synchronous.

### Spotify configuration and sign-in

- Before booting, the application writes `client.yml` with `ClientConfig::init_default_spotify_config()` (the shared client ID and port 8989) and `streaming_device_name: "Rust Player"` if the file does not exist. The fork skips the source picker and the console telemetry prompt in `frontend` boots. The setup wizard's prompts are therefore never reached; the only remaining `prompt_line` is the manual redirect-URL paste when the callback listener cannot bind.
- First run performs two browser consents, both against `127.0.0.1:8989`: the Web API PKCE login (`core/auth.rs`) and, once the account is confirmed Premium, the librespot streaming login (`infra/player/streaming.rs::ensure_streaming_credentials_cached`). The window explains both steps. Both results are cached in the application's data root, so a relaunch needs neither.
- Native streaming requires Premium. A Free account boots with `audio: Unavailable` and a message; search still works.
- Reauthentication maps to Spotatui's in-app login flow (`Network::begin_spotify_login` and `complete_spotify_login`) through a small new `Action` in the fork.

### Native queue

- The Queue is Spotatui's `App::native_queue`. Nothing in this repository stores queue order.
- `Action::AddToQueue` targets Spotify's Web API queue, not the native queue, so the fork adds native-queue actions lifted from `tui/handlers/queue_menu.rs` and `App::add_track_to_native_queue`: `EnqueueNative(TrackInfo)`, `RemoveNativeQueued(index)`, `MoveNativeQueued { index, up }`, and `ClearNativeQueue`. They are the same `Vec` operations the terminal keys perform today, moved onto `App` so both frontends share them.
- Move up and move down replace drag reorder. They map to the existing swap semantics, are keyboard operable by construction, and need no drag-and-drop machinery.

### UI

- The application pins Comet's GPUI revision as a known-good build, but must not call fork-only APIs (backdrop blur, edge fade). Moving to an upstream GPUI release must remain a pin change.
- Version one extracts nothing from Comet as a milestone item. It borrows patterns by reading them: `gpui_tokio` bootstrap and `on_app_quit` shutdown in `crates/ui/src/lib.rs`, the `Tokio::spawn` boot plus `cx.spawn` state fold in `state.rs`, and the `on_reopen` dock behavior. Comet's `theme.rs` (2.5k lines) and `popover.rs` (1.5k lines) are far larger than the palette and panel version one needs; copying a snippet is allowed, adopting a module is not.
- GPUI ships no text input. The search field is a small single-line input written for this application (focus handle, cursor, selection-free editing, submit on Enter). This is version one's largest piece of new UI code and is scheduled explicitly.
- Every interactive element has a focus handle, an accessible label, and keyboard activation. Fixed application shortcuts cover search focus, play or pause, next, previous, and volume.
- Closing the final macOS window preserves the process and the runtime; dock reopen rebuilds the window around the same state. Explicit Quit calls `Runtime::shutdown` from `on_app_quit`.

### Data, security, and diagnostics

- The fork accepts a data root in `frontend::Options` and derives its config, cache, and state directories from it instead of `core/paths.rs`'s XDG lookup. The application passes `~/Library/Application Support/rust-player` on macOS. Nothing under Spotatui's own directories is read or written, including librespot's credential cache.
- The application installs the fork's panic hook (exposed from `frontend`) so audio-backend panics stay recoverable, and owns logging: the fork's `setup_logging` is not called. Logs rotate in the data root and pass through one redaction filter for bearer tokens, refresh tokens, `code=` query parameters, cookies, and authorization headers.
- The application persists only window geometry. Credentials, tokens, device identity, and runtime state belong to the fork under the data root.
- There is no telemetry, crash upload, or usage collection.

### Native audio and packaging

- Native playback uses the fork's pinned `spotatui-librespot-*` family and its macOS PortAudio backend. Developers install Homebrew `portaudio` and `pkgconf`; the readiness check fails clearly when either is missing.
- The system-default output is used because `SPOTATUI_STREAMING_AUDIO_DEVICE` is never set. When the output device changes underneath PortAudio, the backend reports an error, `Driver::tick` pauses native playback and sets a message (`core/driver/mod.rs`, top of `tick`), and Resume goes back through the fork's retrying sink start. Version one relies on exactly that; it does not add device following.
- The unsigned `.app` bundles `libportaudio` in `Contents/Frameworks` with its install name rewritten, and packaging verifies the dynamic-library closure has no Homebrew prefix.
- Signing, notarization, automatic updates, and public distribution are separate release gates.

### Minimal-code constraints

- Reuse Spotatui behavior through its highest safe seam; do not copy OAuth, Web API, playback ownership, queue routing, or librespot recovery code.
- Do not add a type, trait, or state to `player-core` that the version-one window does not render or send.
- Do not introduce factories, dynamic plugin loading, code generation, RPC, or abstractions with no version-one caller.
- Every milestone stops when its acceptance gate passes. Later-milestone infrastructure is not pulled forward.

## Delivery Plan

### Milestone 0 — Fork and foundation

Create the private fork from Spotatui `2b8a238`, make the initial commit of
this workspace, replace the `native-playback` feature with an unconditional
`streaming` dependency, install `pkgconf`, and add the fork's data-root
option.

Acceptance gate: `cargo check` and `cargo test` pass from a clean checkout of
both repositories with the fork built as
`default-features = false, features = ["streaming"]`, and the fork's headless
test leg still passes.

### Milestone 1 — Seam with audible proof

Add the fork's `frontend` module by splitting `launch_ui`, add the native
queue and reauthentication actions, and write a throwaway example binary in
the fork that boots with console onboarding, plays one explicitly supplied
`spotify:track:` URI, pauses, resumes, and shuts down.

Acceptance gate: on macOS, a real Premium account produces audible local audio
through `Runtime` with the `tui` feature off, snapshots update every tick, and
shutdown leaves no running task. The seam is designed from this working
example, not before it.

### Milestone 2 — Contract and fake

Define `player-core` from what Milestone 1 actually yields, write the adapter,
and write a scripted fake runtime. Replace the feasibility shell with a GPUI
window that renders the snapshot as text and sends commands.

Acceptance gate: `apps/player --fake` runs without credentials or audio
hardware, and the same window runs against the real adapter.

### Milestone 3 — Vertical slice

Sign-in surfaces, the search input, result rows, play from a result, and the
persistent now-playing bar with projected progress.

Acceptance gate: a new user completes both consents from the window, searches,
starts audio from a result, sees accurate metadata and progress, and relaunches
without authenticating again. No terminal interaction occurs.

### Milestone 4 — Transport and Queue

Pause, resume, seek, volume, next, previous, and the Queue panel with enqueue,
remove, move up or down, and clear.

Acceptance gate: every transport and Queue story works against the real
adapter, clearing the Queue leaves the active Playable playing, and no UI
state disagrees with the next snapshot.

### Milestone 5 — Failure and recovery

Offline presentation, search retry, expired-session reauthentication, audio
unavailable, skipped-item notices, persistent dismissible errors, redacted
rotating logs, and clean shutdown flush.

Acceptance gate: scripted fake scenarios cover each case without stale UI, and
the real adapter survives pulling the network cable mid-track with audio
continuing.

### Milestone 6 — Native product behavior

Keyboard focus and shortcuts for every control, close and reopen behavior,
Quit shutdown, and visual polish limited to what the slice already shows.

Acceptance gate: every control is keyboard operable; closing and reopening the
window preserves playback; Quit stops playback and leaves state readable on the
next launch.

### Milestone 7 — Self-contained bundle

Build the unsigned `.app`, bundle PortAudio, rewrite dynamic references, copy
notices, and document the real-account smoke test.

Acceptance gate: the bundle launches and produces audio on a clean macOS user
account without Homebrew PortAudio in its runtime path; formatting, clippy,
tests, dynamic-library inspection, and the manual smoke test pass.

### Milestone 8 — Experimental YouTube Source, version two

Advertise Spotatui's existing YouTube engine as a second `Source` variant,
map `Action::SelectSource` and `SearchActiveSource`, exercise mixed-source
Queue behavior, and surface `yt-dlp` failures honestly.

Acceptance gate: defined only when version two begins. No YouTube code or
dependency enters version one.

## Testing Decisions

- Tests assert externally visible commands, snapshots, and persistence outcomes rather than private fields or task structure.
- The fork tests its `frontend` module the way it tests `Action` today: each new action arm delegates to an existing `App` method, and the published snapshot is derived from the existing builders. The only credential-free test of the real runtime is that `boot` reaches the first `Onboarding` call and shuts down cleanly when it is cancelled.
- `player-core` has unit tests only for pure functions: progress projection and any snapshot-to-view derivation the UI extracts.
- The fake runtime drives UI-state tests: sign-in prompts, search transitions, offline catalog, audio unavailable, expired session, skipped Queue item, and shutdown. Rendering is compile-checked, not pixel-tested.
- CI never contains Spotify credentials and never claims to verify audible output.
- One documented manual macOS smoke test with a Premium account verifies both consents, audible playback, transport, Queue operations, relaunch, close and reopen, Quit, and the final bundle.
- Keep the suite minimal: one test per contract consequence, reusing Spotatui's coverage rather than duplicating it.

## Out of Scope

- Remote Playback and Spotify device selection.
- More than one advertised Music Source, and any capability or lifecycle abstraction that only a second source would use.
- Drag-and-drop Queue reorder, and pausing the Queue on a failed item with Retry, Skip, and Remove.
- YouTube, YouTube Music login, YouTube library synchronization, `yt-dlp`, and local YouTube playlists in version one.
- Local files, internet radio, Subsonic, podcasts, episodes, audiobooks, and video playback.
- Spotify library browsing, playlists, playlist editing, likes, recommendations, recently played, friends, listening parties, lyrics, and cover art.
- Queue persistence, shuffle, repeat modes, and save-Queue-as-playlist.
- Explicit audio-output selection and automatic output-device following.
- Customizable shortcuts and OS media-key or now-playing integration, even though the fork's `macos-media` feature exists; it depends on the frontend pumping `NSRunLoop` and is unverified under GPUI.
- Extracting Comet modules, floating popovers, backdrop blur, and any fork-only GPUI API.
- Plugins, Lua scripting, AI DJ, MCP control, telemetry, Discord presence.
- Offline catalog, downloads, and offline playback.
- Daemon mode, RPC, headless operation, multiple windows, multiple viewports, and tray mode.
- Credential import or shared files with the Spotatui TUI.
- Linux and Windows release validation before the macOS version-one gate.
- Signing, notarization, automatic updates, analytics, crash upload, and public release automation.
- Global recording identity or cross-source deduplication.

## Further Notes

- The feasibility shell proves that Comet's pinned GPUI revision and Spotatui's base crate resolve together and that a GPUI window launches. `cargo check` passes on the development machine today.
- The native feature check reaches PortAudio compilation and stops because `pkg-config` is missing; `brew install pkgconf` is a prerequisite task, not an application fix.
- The shared ncspot client ID is a single point of failure Spotatui already accepts; the fork's fallback-client-ID mechanism stays available but version one exposes no UI for it.
- Spotatui registers a Spotify Connect device on every launch; naming it "Rust Player" keeps it distinguishable from a concurrently running TUI.
- Google's supported YouTube integration disallows separated or background audio, so the version-two YouTube Source remains explicitly experimental and unofficial.
- The local path dependency on `../../oss/spotatui` is temporary. The production pin cannot be finalized until the private fork repository and its `frontend` module exist.

## Revision Notes

This revision replaced the earlier draft after checking it against the two
source trees. The material changes and their evidence:

- **Two OAuth consents, both in the browser.** `core/auth.rs` opens the system browser and listens on `127.0.0.1:8989`; `infra/player/streaming.rs` then runs a second, blocking librespot consent on the same port. The earlier "authentication inside the native window" story overpromised; story 1 now says what happens.
- **`boot` is a synchronous conversation.** `Onboarding::prompt_line` blocks, and `boot` also owns the client-ID wizard and a stdin telemetry prompt. Pre-seeding `client.yml` removes every prompt except the redirect-URL paste fallback.
- **The native queue had no actions.** `Action::AddToQueue` dispatches `IoEvent::AddItemToQueue`, the Web API queue. The TUI mutates `App::native_queue` directly in `tui/handlers/queue_menu.rs` (`remove`, `swap`, `drain`). The fork must lift those into `App` and `Action`; the plan now says so.
- **Commands do not complete.** `App::apply` returns `ActionOutcome::Applied` and dispatches; errors land in `App::api_error` later. "Command completion reports success or failure" was replaced by accept-or-reject plus `notice`.
- **Snapshot publication was unspecified.** `App` has no change notification; the `Notify` plus `send_if_modified` design is the smallest thing that is correct.
- **Failed queued items already skip with a message.** Pausing the Queue with three recovery actions would change the fork's routing, contradicting the reuse constraint; it moved out of scope.
- **Drag reorder contradicted keyboard operability** and needed drag machinery; move up or down maps to the existing swap.
- **Two owners of the queue.** ADR 4 read as if the application's Playback Session stored the queue while the runtime section made snapshots authoritative. The queue is `App::native_queue`; `player-core` only describes it. ADR 4 was clarified.
- **The adapter's home.** The earlier draft had the fork supply a `player-core` adapter, which would make the fork depend on this repository. The adapter now lives here and the fork's API stays Spotatui-shaped. ADRs 6 and 13 were clarified.
- **Overlapping status axes.** Source lifecycle, Catalog Availability, Playback Health, capabilities, and a recoverable error were five ways to say three things the window renders. The snapshot now has six fields.
- **Comet extraction was a cost, not a saving.** `theme.rs` and `popover.rs` together exceed 4k lines and the popover needs fork-only GPUI APIs. The Queue is a panel; nothing is extracted; the GPUI pin becomes replaceable. ADR 3 was clarified.
- **Feature flags.** Building the fork with its defaults would pull in `self-update` (replaces the binary on launch), `telemetry`, `macos-media`, `scripting`, and `discord-rpc`. The exact feature set is now stated.
- **Milestone order.** Defining the contract and fake before the seam existed would have fixed the contract on guesses. The seam is now proven audible first (old Milestones 1 and 2 merged), then the contract is derived from it.
