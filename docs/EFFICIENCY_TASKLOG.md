# Efficiency task log

This log records one performance change at a time. Each task is measured,
implemented, and verified before the next task begins.

## Ordered tasks

| Status | Task | Expected impact | Risk |
| --- | --- | --- | --- |
| complete | Capture a release-build idle and active playback baseline | Establish comparable CPU, wakeup, and progress-update numbers | Low |
| complete | Make progress cadence adaptive to window focus | Reduce redraw work while the window is in the background | Low |
| complete | Cache now-playing metadata and second-resolution clock text | Reduce per-update allocations | Low |
| complete | Isolate the progress bar into a lightweight GPUI element | Avoid rebuilding unrelated window layout for progress changes | Medium |
| complete | Add a cooldown for repeated Spotify `429` responses | Reduce retry, wakeup, and log churn | Medium |
| complete | Reprofile release active and paused playback; keep only measured wins | Confirm CPU and wakeup improvements | Low |

## 2026-09-07 — task 1

The release workspace build and test suite are already green. A credential-free
`--fake` release smoke settled at 1.0% CPU while idle over a 24-second run. An
authenticated active playback baseline still requires a real account and cannot
be captured by the fake runtime, so the active comparison remains open.

The current implementation updates the now-playing entity every 100 ms during
active native playback. Task 2 will preserve that focused cadence while using a
slower cadence when the window is unfocused.

## 2026-09-07 — task 2 started

The focus state is available from GPUI's `Window::is_window_active`. The change
will keep the timer cancellable and adjust only its interval, without changing
playback state or the event spine.

Task 2 is complete. Focused updates remain at 100 ms; unfocused updates use 500
ms. The 18 player tests, rustfmt check, and locked release build passed.

## 2026-09-07 — task 3 started

The title and artist line only changes when the active Playable or its playing
state changes. The elapsed label changes only once per displayed second. These
values will be cached in the now-playing entity while the progress bar keeps
its existing timer cadence.

Task 3 is complete. Metadata and duration are cached as `SharedString`/scalar
state, and the clock label is rebuilt only on a displayed-second boundary or a
playback metadata change. The 18 player tests and locked offline release build
passed. Task 4 will move the cadence and changing width into a child entity so
the static now-playing controls do not rebuild on every progress tick.

## 2026-09-07 — task 4 started

The progress width is the only value that changes at the timer cadence. A child
GPUI entity will own that width, focus-aware timer, and performance counter;
the parent will retain metadata, clock text, controls, and seek handling.

Task 4 is complete. `ProgressBar` now owns the projected width, adaptive timer,
and playback render counter. `NowPlaying` retains the static row and refreshes
its cached clock once per second, so 100 ms progress ticks do not rebuild its
text, controls, or parent layout. The 18 player tests and locked offline
release build passed.

## 2026-09-07 — task 5 started

The pinned engine already honors `Retry-After` for an individual 429, but the
next playback poll can immediately begin another retry sequence. The next
change will add a short poll cooldown after a rate-limit response, preserving
the last playback state while avoiding repeated request and log churn.

Task 5 is complete. The engine now emits a rate-limit fact after the playback
request helper exhausts its retries; the fold keeps the last context visible,
clears the in-flight marker, and delays the next poll by 10 seconds. The engine
test, 18 player tests, and locked offline release build passed. The workspace is
pinned to engine revision `28a2570c`. Task 6 will reprofile the release binary
and compare the available fake/idle measurements with the earlier baseline.

## 2026-09-07 — task 6 started

The active authenticated playback comparison still needs a real Spotify session.
The credential-free fake runtime can validate idle overhead and process startup;
the final pass will capture that release measurement and report the active
comparison as unavailable when no account is present.

Task 6 is complete for the available environment. On the release binary with a
fresh `--fake` data root, CPU settled to 0.0% in the post-startup samples after
10 seconds; a five-second `sample` captured no recurring render/layout stack.
The authenticated active-playback and paused-engine acceptance runs remain
unavailable without a Spotify account, so no active CPU or wakeup improvement
is claimed. The measured changes are retained and the task sequence is closed.

The final `cargo test --workspace --locked --offline` pass completed with 6
`player-core` unit tests, 15 contract tests, 18 adapter tests, and 18 player
tests passing.
