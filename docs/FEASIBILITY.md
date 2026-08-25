# Comet UI + Spotatui playback feasibility

Sources inspected on 2026-08-25:

- Comet `cd6cb22` at `../../oss/comet`
- Spotatui `2b8a238` (`0.41.0`) at `../../oss/spotatui`

## Result

The combination is technically feasible. GPUI and Spotatui resolve in one
Cargo graph. The base application compiles and links. Enabling Spotatui's
maintained librespot stack reaches its expected macOS PortAudio system build;
on the test machine it stops because `pkg-config` is not installed (PortAudio
itself is installed). This is a host prerequisite, not a Rust dependency
conflict. The remaining code blocker is API visibility, not incompatible
runtimes, dependencies, or state models.

It is not currently possible to build the real player by consuming the two
crates unchanged:

- Comet's `zeron-ui` publishes components, but the crate depends on Zeron's
  engine, RPC, document, harness, syntax, update, and theme crates. Pulling the
  whole crate into a small player would compile most of Comet.
- Spotatui's crate root exports only `run_cli`. `App`, `Action`, `Boot`, the
  event pump, `Network`, and plugin snapshots are behind private top-level
  modules or `pub(super)` runtime boundaries.

The shell in `apps/player` therefore uses Comet's exact GPUI fork/revision and
its basic shell conventions, while linking Spotatui only as a compatibility
probe. It does not pretend that a local play/pause toggle is real playback.

## What is reusable

### From Comet

Comet's workspace split is useful: an app binary owns startup while UI code is
kept outside domain logic. Its GPUI startup is in `crates/ui/src/lib.rs`, and
the reusable-looking visual modules are `theme`, `typography`, `motion`,
`frost`, `popover`, `loaders`, and `icons`.

For a small player, importing `zeron-ui` itself is the wrong dependency seam.
Extract only components actually used into a small GPUI-only crate, starting
with none and moving a component when the player needs it. The initial shell
needs only GPUI primitives.

### From Spotatui

Spotatui is already structured for another frontend:

- `core/action/mod.rs` defines a serializable, rspotify-free `Action` enum and
  explicitly calls it the contract for future frontends.
- `core/plugin_api.rs` contains serializable `PlaybackState`, track, device,
  queue, library, search, and lyrics snapshots.
- `runtime/bootstrap.rs` builds all authenticated state into `Boot`.
- `runtime/pump.rs` is documented as the event pump every frontend drives.
- `core/driver` owns ticking rather than the Ratatui runner.
- `infra/player` owns native playback through Spotatui's pinned librespot fork.
- The existing `gui` feature and `spotatui-gui` binary are explicit placeholders
  for this work.

This is substantially safer and shorter than reimplementing OAuth, API pacing,
playback ownership, queue routing, recovery, and Spotify Connect behavior.

## Smallest production design

The least-code path is to land a narrow frontend API in Spotatui and make this
GPUI app its first consumer. The public surface should expose one opaque
runtime handle, not Spotatui's 168-field `App`:

```rust
pub struct PlayerRuntime { /* private */ }

impl PlayerRuntime {
    pub async fn boot(onboarding: Arc<dyn Onboarding>) -> Result<Self>;
    pub async fn snapshot(&self) -> PlayerSnapshot;
    pub async fn dispatch(&self, action: Action) -> ActionOutcome;
    pub async fn tick(&self, viewport: Viewport);
    pub async fn shutdown(self);
}
```

`PlayerSnapshot` can compose the existing plugin snapshot types. `dispatch`
must call the existing `App::apply`; it must not expose raw `IoEvent`, because
that would bypass Spotatui's playback-ownership rules. The handle owns the
event pump and deferred streaming startup so a frontend cannot accidentally
omit either.

Once that seam exists, the GPUI view needs only:

1. a Tokio/GPUI bridge;
2. snapshot refresh/subscription;
3. controls that send existing `Action` variants;
4. the small subset of Comet components actually visible in the design.

## Risks and constraints

- Native Spotify audio requires Spotatui's `streaming` feature, its pinned
  `spotatui-librespot-* = 0.8.3` family, and a supported Spotify account. On
  macOS its PortAudio backend also requires `pkg-config`/Homebrew `pkgconf` and
  PortAudio development metadata.
- Spotatui has multiple playback owners. A GUI must use `Action`/`App::apply`
  rather than driving librespot or the Web API directly.
- Comet pins a fork of GPUI for custom blur/edge-fade behavior. Using its visual
  components means accepting that git dependency until those APIs are upstream.
- OAuth onboarding is currently terminal-oriented in the default implementation,
  although the core already accepts an `Onboarding` trait. A GPUI implementation
  can replace the prompts without changing auth logic.
- The local path dependency is suitable only for this study. A real repository
  needs a Spotatui release/fork exposing the runtime handle.

## Decision

Proceed, but do not copy Spotatui's internals or depend on all of `zeron-ui`.
First extract the opaque runtime API in Spotatui; then connect this shell and
move only the Comet components the resulting UI actually uses.
