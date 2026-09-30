# Project catalog lists by revision

The source adapter owns catalog revisions: it assigns a new revision only when
an accepted catalog completion changes, and preserves a revision for the same
catalog data. `player-core` owns `PlaybackListProjector`, which uses that
revision to reuse or replace an unselected candidate Implicit Playback List.

GPUI owns presentation and retains one source-neutral projector for search,
detail, and library rows, replacing its per-surface catalog-list caches and
source-specific constructors. Clearing a loading or failed catalog projection
never changes the active Playback Session or its selected Implicit Playback
List.

Clarification (2026-09-30): the Playback Engine assigns catalog revisions and
publishes them on its snapshot. The adapter translates them into
`CatalogRevision` and keys its translation cache on them; it never assigns
one.
