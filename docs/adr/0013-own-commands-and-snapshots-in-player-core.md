# Own commands and snapshots in player core

`player-core` owns the small source-neutral command and snapshot contracts consumed by GPUI, and the adapter in this repository maps them to the fork's existing actions and snapshot builders. The fork exposes boot, subscription, action application, and shutdown while keeping `App`, `IoEvent`, Spotify models, and librespot types private, accepting a thin mapping layer to keep the product boundary independent of its first execution engine.

Clarification (2026-08-25): the contract is derived from the working seam, not designed ahead of it, and contains only what the version-one window renders or sends. Commands are accepted or rejected synchronously; failures surface in the next snapshot, because `App::apply` is infallible and reports errors through `api_error`.
