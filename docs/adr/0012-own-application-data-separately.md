# Own application data separately

The application will use its own data directory and require its own initial Spotify login rather than sharing Spotatui's mutable files. It reuses the fork's persistence behavior and formats, but isolating credentials, runtime state, queue recovery, and device identity prevents concurrent TUI and GUI processes from racing one authority; credential import is deferred until demand justifies it.
