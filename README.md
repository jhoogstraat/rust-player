# Rust Player

A small native macOS music player: a GPUI window over Spotify's catalog with
native audio playback, built on a private Spotatui fork. No terminal, no
Spotify desktop client.

## Layout

- `crates/player-core` — the source-neutral contract (snapshot, commands,
  runtime trait), a scripted fake runtime, and progress projection. Depends on
  nothing heavier than `tokio::sync`.
- `crates/player-spotatui` — adapter mapping the contract onto the fork's
  `frontend` module. No playback logic.
- `apps/player` — the GPUI application. Never imports the fork.
- The Spotatui fork is consumed from a sibling checkout at `../spotatui`
  (path override documented in `Cargo.toml`; production pins it over Git).

## Prerequisites

```sh
brew install pkgconf portaudio
```

Spotatui is built with `streaming` unconditionally on, which selects
librespot's PortAudio backend via `pkg-config`; without `pkgconf`, `cargo
check` fails clearly in that build script — that failure *is* the readiness
check.

## Run

```sh
cargo run -- --fake   # scripted runtime: no credentials, no audio hardware
cargo run             # real runtime; data root ~/Library/Application Support/rust-player
```

## Checks

```sh
cargo fmt --check
cargo check
cargo test
```

## Package (macOS)

```sh
scripts/package_app.sh          # unsigned .app with PortAudio bundled
docs/SMOKE_TEST.md              # the manual Premium-account pass
```

See [docs/IMPLEMENTATION_PLAN.md](docs/IMPLEMENTATION_PLAN.md) for the
authoritative roadmap and [CONTEXT.md](CONTEXT.md) for domain language.
