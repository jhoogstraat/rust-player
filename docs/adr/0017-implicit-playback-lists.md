# Treat browsable playback order as an implicit queue

Playback from a browsable list installs that list as the Playback Session's
Implicit Playback List and starts at the selected item. The explicit Queue is
separate and has priority: its head plays next whenever it is non-empty, then
playback resumes at the next item in the Implicit Playback List.

The list is source-labelled and retained in the source-neutral snapshot so the
window can show the exact order being followed without coupling browsing state
to playback. Selecting an item from a different list replaces the implicit
list; enqueue, remove, move, and clear operations affect only the explicit
Queue.

The Playback Engine receives the complete ordered URI list plus the selected
offset. This reuses its existing end-of-track and native-queue ownership paths,
including album, artist, and playlist context playback. The adapter owns the
translation between the product's list metadata and the engine's playback
request.

The implicit list is limited to the tracks currently resolved by the catalog
surface. Paginated surfaces must expose contiguous fetched pages before they
are offered as a complete playable list; silently treating one visible page as
the whole source would truncate playback.
