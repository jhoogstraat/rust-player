# Pin the Spotatui fork as a separate dependency

The private Spotatui fork remains a separate repository and production builds consume its private Git URL at an exact revision. Coordinated development may use a documented local path override, but this repository will neither vendor nor submodule the fork; deliberate pin updates preserve reproducibility without coupling the two repositories' histories.

Clarification (2026-09-30): the dependency is built with `default-features = false` and only the `streaming` and `macos-media` features. The engine's other defaults (`telemetry`, `discord-rpc`, `mpris`, `self-update`, `windows-media`) stay off on purpose; the application must never update itself or report usage through the engine.
