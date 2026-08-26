# Morph the Spotatui fork into the playback engine

An architecture review traced the application's idle CPU burn to a heartbeat the
product does not need: the fork's terminal frontend runs a 250 ms tick loop that
rings a change bell unconditionally, so an idle app rebuilds, relays, and
re-renders its full state four times per second, forever. We decided to keep none
of that machinery: the private Spotatui fork is morphed into this product's own
playback engine. The terminal UI and every capability only it consumed (themes,
lyrics scrolling, animations, party mode, scripting hooks, remote-control layers)
are deleted; what survives is sign-in/session, streaming playback with recovery,
catalog browsing (search, library, queue), and persistence — plus the multi-source
dispatch skeleton, which stays because YouTube Source remains a planned Music
Source.

The replacement architecture is event-sourced: one typed event stream carries
playback events (native librespot events for position), Web API responses, auth
transitions, timer firings, and command side effects; one fold task consumes it,
owns authoritative Playback Session state, and publishes deduplicated immutable
snapshots behind the unchanged `player-core::Runtime` interface. Commands are the
sole input vocabulary (`Command` from day one; the fork's `Action` enum dies in
the rewrite rather than being carried through). Time becomes explicit instead of
polled: token refresh, load watchdogs, and message expiry become individually
armed timers, and visible position is projected by GPUI via
`request_animation_frame()`. Staged deletion-first across four green-compiling
stages (delete TUI capabilities → land event spine and measure → dissolve the
shared-state mutex into domain owners → reshape the interface and rename), with
an acceptance gate of under 1% CPU while signed in, paused, and idle.

## Status

Supersedes [ADR 0002](0002-maintain-a-private-spotatui-fork.md) — once the morph
completes there is no fork relationship left to maintain; the code becomes this
repository's own engine. Extends
[ADR 0005](0005-project-runtime-state-into-gpui.md): position projection remains
local to the window, now fed by native position events instead of tick-derived
snapshots. ADR 0015's path-dependency pin remains in force only until stage 4.

## Considered options

- **Quiet the tick** (mutation predicates + heartbeat backstop): keeps the poll
  and its predicate list as permanent tax; rejected.
- **Keep the shared state model, feed it better**: smaller change, but keeps the
  presentation-shaped state inside the module; rejected once the TUI was declared
  disposable.
- **Big-bang rewrite**: rejected in favour of four independently shippable
  stages, each compiling against the running app, with a measurement gate after
  the event spine lands.
