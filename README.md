# Rust Player

A minimal native Rust music-player experiment combining Comet's GPUI approach
with Spotatui's Spotify and native-playback foundation.

This repository currently contains a compile-tested UI shell. The play button
deliberately changes only local UI state: the fork's `frontend` runtime
module (a later milestone) is what will let this view drive real playback.

## Prerequisites

```sh
brew install pkgconf portaudio
```

Spotatui is built here with `streaming` unconditionally on, which selects
librespot's PortAudio backend on macOS via `pkg-config`. Without `pkgconf`
installed, `cargo check` fails clearly during the native feature's build
script with a `pkg-config` / `portaudio` not-found error rather than a linker
error deep in the build; that failure *is* the readiness check.

## Checks

```sh
cargo fmt --check
cargo check
cargo test
cargo run
```

The workspace consumes Spotatui through a sibling checkout of the private
fork at `../spotatui` (a path override documented in `Cargo.toml`).
Production pins the fork over Git at an exact revision once its `frontend`
API is public; the path dependency is only the local-development
convenience.

See [docs/IMPLEMENTATION_PLAN.md](docs/IMPLEMENTATION_PLAN.md) for the
authoritative roadmap. [docs/FEASIBILITY.md](docs/FEASIBILITY.md) preserves the
initial integration evidence.
