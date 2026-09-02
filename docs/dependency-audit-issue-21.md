# Playback Engine dependency reachability audit (#21)

Date: 2026-09-02
`rust-player`: `ed2a14e`
Playback Engine checkout: `spotatui` `bd39a10`
Cargo package: `spotatui` 0.41.0

This is an audit only. No product behavior, dependency pin, or sibling
checkout was changed. Re-run the complete evidence set with:

```sh
SPOTATUI_DIR=../spotatui scripts/audit_dependency_reachability.sh
```

The script uses `cargo tree --locked`, so it observes the committed lockfiles;
it does not write to either repository.

## Build shape and evidence

The shipped adapter is `player-spotatui`, whose workspace dependency selects
`spotatui` with `default-features = false, features = ["streaming"]`
(`Cargo.toml:26`). Its only runtime entry point is
`spotatui::frontend::Runtime` (`crates/player-spotatui/src/lib.rs`); the app
does not call `spotatui::run_cli`.

On this checkout, the locked tree reports:

| Graph | Unique package/version nodes |
| --- | ---: |
| `player-spotatui` leg (includes `player-core` and its Tokio) | 418 |
| sibling `spotatui --no-default-features --features streaming` | 417 |
| sibling `spotatui` defaults (comparison) | 454 |
| full `rust-player` app (includes GPUI) | 661 |

The 37-node default-vs-streaming difference is the expected opt-in surface:
self-update, Discord RPC, MPRIS, Windows/macOS media integrations, and their
transitives. None is selected by rust-player's dependency declaration. The
streaming feature activates the seven pinned librespot/protobuf crates in the
sibling manifest (`Cargo.toml:80-85,132`); target selection adds ALSA on Linux,
PortAudio on macOS, or Rodio on Windows (`Cargo.toml:91-117`).

## Reachability classification

“Retained” means reachable from the embedded runtime or required to compile a
reachable module. “Conditional” means reachable only under a feature or
platform not selected by the shipped graph. “Unreachable” means no path from
the shipped `frontend::Runtime` (a dependency may still be compiled because
the library exposes the old CLI or declares it unconditionally).

| Capability / dependency | Classification | Evidence and impact |
| --- | --- | --- |
| `spotatui-librespot-{core,connect,oauth,metadata,playback,protocol}` + `protobuf` | Retained (`streaming`) | `infra/player/{streaming,events}.rs` imports all librespot crates; `infra/network/library.rs:1650-1651` uses protobuf. Native Playback, auth/session recovery, and queueing depend on this set. Keep pinned in lockstep. |
| `rspotify`, async `reqwest`, `serde`, `serde_json`, `serde_yaml`, `dirs`, `url`, `rand`, `chrono`, `open`, `anyhow`, `log` | Retained | `runtime/frontend`, `runtime/startup`, `core/auth`, `infra/network`, config/state and migrations use these paths. They cover Native Playback, catalog access, persistence, and browser sign-in. Removing any is behavior-changing. |
| `backtrace` | Retained | `player-spotatui` calls `frontend::Runtime::install_panic_hook`; that delegates to `runtime/bootstrap.rs:110-151`, which records `Backtrace`. |
| `arboard` | Retained by construction; UI route not exposed | `core/app/construction.rs:106` always attempts `Clipboard::new`; `core/app/transport.rs:378-445` implements copy actions. The adapter's `EngineAction` mapping exposes no copy command, but removing the crate requires deleting/gating the App field and settings. It also carries platform clipboard build cost. |
| `keepawake` | Unreachable capability (dead field) | `App.keepawake` is initialized to `None` (`core/app/construction.rs:138`) and has no assignment or `KeepAwake::new` call; only config/settings plumbing remains (`core/app/mod.rs:339`, `core/user_config.rs`). Delete the field and obsolete config/settings in a compatibility-reviewed sibling change; unknown YAML keys are already tolerated. |
| `tokio-tungstenite` 0.30 direct edge | Unreachable direct edge | No source import exists; only `Cargo.toml:87` and changelog mention it. The streaming tree still has 0.28 through librespot, so deleting this declaration removes the duplicate direct 0.30 edge, not all websocket code. Verify the lockfile and license report after removal. |
| `futures` direct edge | Conditional; unreachable in shipped streaming build | Uses are confined to `infra/local/dispatch.rs` and `infra/subsonic/mod.rs`, both modules gated off unless `local-files`/`subsonic`. `rspotify` and librespot independently retain `futures`, so making this declaration optional may improve feature hygiene without removing the package. |
| `clap`, `clap_complete`, `fern` | Unreachable from rust-player; retained by public CLI | `runtime/mod.rs` and `runtime/cli.rs` build `run_cli`, shell completions, and CLI logging. `frontend::Runtime::boot` never calls that path. Gate `mod cli`, `run_cli`, `setup_logging`, and the console bin behind a sibling `cli` feature; preserve default CLI behavior for existing spotatui users. |
| `rspotify` `cli` feature → `webbrowser` | Unreachable from embedded runtime | `rspotify`'s `cli` feature only enables its `webbrowser` helper. Spotatui auth opens URLs with direct `open::that` (`core/auth.rs:405`, `infra/network/mod.rs:1143`); no `prompt_for_user` call is present. Remove `cli` from the rspotify feature list after checking the public CLI build, which drops the otherwise-unused `webbrowser` subtree. |
| `reqwest` `blocking` feature | Conditional CLI-only edge; package remains transitively | Direct blocking calls occur only in `cli/update.rs`, behind self-update. Streaming's librespot OAuth already enables `reqwest/blocking`, so removing the direct feature is hygiene with no current package-count win. Gate it with the CLI/self-update feature if upstream OAuth ever stops requiring it. |
| `tokio` `full` feature | Retained runtime, over-broad feature set | Embedded runtime uses `rt-multi-thread`, `sync`, `time`, `macros`, `net`, and `io-util` (redirect listener and event spine). `full` also enables unrelated Tokio capabilities. Replace it with the audited minimum in a sibling change, then run all target builds. |
| `unicode-width` | Conditional terminal-era validation | Only `core/user_config.rs:1305` validates one-column terminal icons. Config loading still reaches this check, so deletion changes malformed-config fallback behavior. Defer until terminal-only config keys are explicitly retired and migration tests exist. |

## Smallest safe follow-up plan

1. In the Playback Engine repository, delete the unreferenced direct
   `tokio-tungstenite` 0.30 declaration and remove `rspotify`'s `cli` feature;
   run the streaming build and CLI build separately to prove the compatibility
   boundary. Re-lock and inspect the duplicate websocket/webbrowser trees.
2. Make `futures` optional and add it only to `local-files`/`subsonic`; gate
   the CLI modules and `fern` behind an explicit `cli` feature while retaining
   the existing default feature for standalone spotatui releases.
3. Replace Tokio `full` with the minimum set observed above. Build the shipped
   rust-player target on Linux, macOS, and Windows feature selections because
   target-specific librespot backends differ.
4. Separately decide whether copy-to-clipboard is part of the Playback Engine
   contract. If not, remove/gate `arboard`; remove the never-instantiated
   `keepawake` field and its settings/config serialization in the same bounded
   cleanup. Preserve unknown-key parsing and add one migration test before
   deleting user-facing settings.
5. Leave `serde_yaml`, `unicode-width`, and the librespot/protobuf stack alone
   until a separate product/compatibility decision retires their behavior.

No licensing incompatibility was found: the workspace remains MIT, and these
recommendations only remove or gate existing third-party edges. A follow-up
implementation must regenerate `Cargo.lock`, review the upstream package
licenses in the resulting tree, and update rust-player's pinned engine revision
only after the sibling tests pass.
