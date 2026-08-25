# Centralize playback sessions

Playback is owned by one source-neutral Playback Session rather than by each Music Source. A source resolves its content into Playables, while the session owns the active Playback Device, queue, and transport state; this makes future source handoff and mixed-source playback possible without forcing every source to reproduce playback policy.

Clarification (2026-08-25): in version one the Spotatui runtime realizes the session. Its `native_queue` is the Queue and its driver is the transport. `player-core` describes the session through commands and snapshots; it never stores queue order or transport state of its own, so there is exactly one owner.
