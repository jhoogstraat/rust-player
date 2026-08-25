# Maintain a private Spotatui fork

The application will consume a small private Spotatui fork that exposes an opaque frontend runtime, rather than copying its internals or proposing the API upstream. This keeps authentication, Spotify API behavior, playback routing, and librespot recovery owned by Spotatui while giving this project a stable integration seam it can evolve independently.
