# Maintain a private Spotatui fork

> **Superseded by [ADR 0016](0016-morph-the-spotatui-fork-into-the-playback-engine.md)**: the fork is being morphed into this product's own playback engine; there is no fork relationship left to maintain once the morph completes. This decision is retained for the historical record of how the integration started.

The application will consume a small private Spotatui fork that exposes an opaque frontend runtime, rather than copying its internals or proposing the API upstream. This keeps authentication, Spotify API behavior, playback routing, and librespot recovery owned by Spotatui while giving this project a stable integration seam it can evolve independently.
