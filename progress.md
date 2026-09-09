# Progress

Updated: 2026-09-09

## Current status

The playback responsiveness work is implemented and locally verified. Pending
playback now also stops local now-playing clock wakeups while waiting and
restarts them when the 15-second fallback clears the request. The engineering
queue is unblocked; release-only checks are recorded separately below.

### Completed

- Added immediate `Starting…` / `Changing track…` feedback when a playable track is selected.
- Added pending-playback handling with generation tracking and a one-shot 15-second timeout.
- Covered library rows, search/detail rows, track context menus, keyboard/actions, and transport buttons.
- Hid stale progress and clock values while a requested playback change is pending.
- Moved current-playback polling off the serial transport lane so slow network polling and retries cannot block play/next actions.
- Limited playback polling to one concurrent request and preserved client/config snapshots across login changes.
- Added `scripts/ralph_codex.sh`, a bounded Codex loop driven by this file and
  configured for the local repository, sibling dependency checkout, and
  `gpt-5.6-luna` at `xhigh` reasoning effort.
- Updated the pending now-playing transition so timeout fallback resumes the
  local clock task and new pending requests stop it.

## Verification

- Root `cargo check`: passed.
- Root `cargo test --workspace --all-targets`: passed.
- Focused now-playing unit tests: passed (3 tests).
- Sibling `spotatui-player-performance` check with streaming/macOS media features: passed.
- Sibling library tests with streaming/macOS media features: passed (607 tests).
- Sibling `cargo check --no-default-features`: passed.
- `git diff --check`: passed in both worktrees.
- This iteration: `cargo fmt --all -- --check`, focused now-playing tests,
  root `cargo check`, and root `cargo test --workspace --all-targets` passed.
- This iteration: `cargo fmt --all -- --check` and `cargo test --workspace
  --all-targets` passed; `cargo run -- --fake` launched successfully and was
  intentionally stopped after launch.
- `git diff --check` passed before the implementation checkpoint was committed.

## Deferred release gates

- The full manual GUI smoke test with a real Premium account, audio output,
  and a usable display remains a release gate. The user has now authorized the
  check, but this environment cannot provide GUI control or display capture;
  the fake runtime launch and automated tests cover the available paths.
- The sibling `spotatui-player-performance` checkout remains a coordinated
  local override documented in `.cargo/config.toml` and `HANDOVER.md`. Its
  publication and production pin update are release work, not a blocker for
  this checkpoint.
- The unrelated `architecture-review-20260901-232316.html` artifact remains
  preserved and outside the implementation commit.

## Open topics

No actionable engineering topics remain.

## Worktree note

The existing dirty-worktree changes were preserved. The implementation batch
and this ledger are committed in the current checkpoint. The unrelated HTML
artifact and sibling worktree remain untouched.
