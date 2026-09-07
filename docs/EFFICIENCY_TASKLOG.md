# Efficiency task log

This log records one performance change at a time. Each task is measured,
implemented, and verified before the next task begins.

## Ordered tasks

| Status | Task | Expected impact | Risk |
| --- | --- | --- | --- |
| complete | Capture a release-build idle and active playback baseline | Establish comparable CPU, wakeup, and progress-update numbers | Low |
| complete | Make progress cadence adaptive to window focus | Reduce redraw work while the window is in the background | Low |
| complete | Cache now-playing metadata and second-resolution clock text | Reduce per-update allocations | Low |
| **in progress** | Isolate the progress bar into a lightweight GPUI element | Avoid rebuilding unrelated window layout for progress changes | Medium |
| pending | Add a cooldown for repeated Spotify `429` responses | Reduce retry, wakeup, and log churn | Medium |
| pending | Reprofile release active and paused playback; keep only measured wins | Confirm CPU and wakeup improvements | Low |

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
