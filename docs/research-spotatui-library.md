# Spotatui library data for Rust Player

Research date: 2026-08-26. The local Spotatui checkout is `../spotatui`, at
commit [`6fef473`](https://github.com/jhoogstraat/spotatui/commit/6fef473)
(`origin/main`, v0.41.0 as declared in [`Cargo.toml`](https://github.com/jhoogstraat/spotatui/blob/6fef473/Cargo.toml#L1-L10)). The repository has no existing research-note
convention, so this note lives under `docs/`.

## Implementation status

The minimal integration described below is now implemented in the working
tree: Spotatui's frontend publishes optional liked-song, recently-played, and
playlist rows; Rust Player maps those rows into its existing `LibraryState`;
and `Command::Browse` dispatches the liked/recent actions while using the
startup playlist cache. Recently-played timestamps, explicit pagination
commands, and playlist drill-down remain intentionally out of scope.

## Executive finding

Spotatui already implements all three Spotify reads internally. At the
research baseline its published `spotatui::frontend` seam did not publish any
library data; the implementation adds that missing `LibrarySnapshot` seam to
the working tree. The baseline public frontend snapshot contained playback,
queue, catalog-search, connection, notice, and audio fields only ([baseline
`Snapshot`](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/runtime/frontend/mod.rs#L22-L46);
baseline snapshot builder](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/runtime/frontend/mod.rs#L345-L364)).
The frontend module deliberately hides `App`, `IoEvent`, `Network`, and
rspotify types ([module contract](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/runtime/frontend/mod.rs#L1-L9)).

The fork does define `Action::OpenLibrary(LibraryTarget)` and
`Action::LoadMore(ListTarget::SavedTracks)` ([action definitions](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/action/mod.rs#L180-L200), [targets](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/action/mod.rs#L323-L369)),
but these actions do not make the resulting `App` state observable through
`frontend::Snapshot`. Moreover, `frontend` currently re-exports `Action`,
`ActionOutcome`, `Onboarding`, and a few snapshot types, but not
`LibraryTarget`, `ListTarget`, or `OpenTarget`; an external adapter therefore
cannot conveniently construct the library/open/pagination actions even though
the variants exist internally ([frontend re-exports](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/runtime/frontend/mod.rs#L14-L18)).
Before this implementation Rust Player handled `Command::Browse` by
publishing `LibraryState::Failed` with “not available from this source yet”
([baseline adapter](../crates/player-spotatui/src/lib.rs#L250-L267)); the
adapter now maps the new frontend library snapshot instead.

This fits Rust Player’s ADR: the application intentionally consumes a small,
opaque Spotatui frontend runtime rather than importing Spotatui internals
([ADR 0002](adr/0002-maintain-a-private-spotatui-fork.md)).

Rust Player’s UI is already shaped for these results: each sidebar click sends
`Command::Browse(LibrarySection)` ([sidebar handler](../apps/player/src/sidebar.rs#L118-L123));
the library renderer consumes `Snapshot::library` and renders track rows with
optional `played_at_ms`, or playlist rows ([renderer](../apps/player/src/library.rs#L16-L54)).
The source-neutral contract defines `LibraryEntry::Track` (including
`played_at_ms`) and `LibraryEntry::Playlist` ([contract](../crates/player-core/src/lib.rs#L64-L100)).

## Existing internal flows

### Why Spotatui's own TUI can render these rows

The terminal UI is compiled inside the Spotatui crate, so it can import the
private `crate::core` modules and the `App` state directly. Its runner owns the
shared `Arc<Mutex<App>>`, locks it for each frame, and passes `&App` to the UI
renderer ([runner loop](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/tui/runner.rs#L328-L447)).
The route renderer then selects the appropriate table: liked songs use the
generic `TrackTable` route, recently played uses its own table renderer, and
both read the corresponding `App` collections directly ([route dispatch](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/tui/ui/mod.rs#L57-L87), [song table](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/tui/ui/tables.rs#L509-L568), [recent table](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/tui/ui/tables.rs#L648-L680)).
The playlist sidebar similarly formats `app.all_playlists` ([playlist sidebar](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/tui/ui/library.rs#L33-L180)).
Selecting a library row resolves a `LibraryTarget` and applies the internal
action directly; the network event later mutates the same `App`, so the next
frame sees the fetched rows ([library handler](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/tui/handlers/library.rs#L42-L57)).

Therefore “no public API” means “no supported `frontend`/embedding API for an
external crate,” not “the TUI cannot access the data.” Rust Player is an
external consumer of `spotatui::frontend`, so it only sees fields deliberately
copied into `frontend::Snapshot`; the implementation now publishes the library
snapshot through that seam.

### Liked songs (Spotify saved tracks)

* The internal library action is `App::open_library_section(LibraryTarget::LikedSongs)`.
  It clears the saved-track view, dispatches `IoEvent::GetCurrentSavedTracks(None)`,
  and opens the track-table route ([`open_library_section`](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/app/library.rs#L157-L199)).
* The network handler calls `GET /v1/me/tracks` as `Page<rspotify::model::SavedTrack>`.
  It sends `limit = self.large_search_limit` (50 by default) and an optional
  offset ([handler](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/network/library.rs#L971-L989);
  default limit](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/network/mod.rs#L381-L399)).
  On success it maps each saved item’s `track` to source-neutral `TrackInfo`,
  inserts its base-62 id into `liked_song_ids_set`, stores the page in
  `app.library.saved_tracks`, bumps the saved-tracks generation, and starts a
  prefetch for the next offset when `next` exists ([success/prefetch path](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/network/library.rs#L992-L1016)).
* The source-neutral page shape retains `items`, `offset`, `limit`, `total`,
  `next`, and `previous` ([`Paged<T>`](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/pagination.rs#L18-L48)).
  `plugin_api::saved_tracks_snapshot` flattens all pages fetched so far in
  library order ([snapshot helper](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/plugin_api.rs#L668-L677)).
* `Action::LoadMore(ListTarget::SavedTracks)` advances this paginated list by
  calling `get_current_user_saved_tracks_next()` ([action dispatch](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/action/apply.rs#L146-L149);
  next-page logic](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/app/library.rs#L286-L307)).

Spotify’s authoritative endpoint description calls this “a list of the songs
saved in the current Spotify user's ‘Your Music’ library”, requires the
`user-library-read` scope, and limits `limit` to 1–50; `offset` is used for the
next page ([Spotify Get User’s Saved Tracks](https://developer.spotify.com/documentation/web-api/reference/get-users-saved-tracks#request)).
The response item includes `added_at` and a nested track object
([response fields](https://developer.spotify.com/documentation/web-api/reference/get-users-saved-tracks#response)).
Spotatui currently maps only the nested track into `TrackInfo`; the
`added_at` timestamp is not carried by `TrackInfo`.

### Recently played songs

* `App::open_library_section(LibraryTarget::RecentlyPlayed)` dispatches
  `IoEvent::GetRecentlyPlayed` and pushes the recently-played route immediately
  ([library action](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/app/library.rs#L166-L176)).
* `UserNetwork::get_recently_played` calls
  `GET /v1/me/player/recently-played` with only `limit = self.large_search_limit`
  ([request](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/network/user.rs#L255-L263);
  default limit](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/network/mod.rs#L381-L399)).
  It parses `CursorBasedPage<PlayHistory>`, maps each `PlayHistory.track` to
  `TrackInfo`, stores the mapped cursor page in `app.recently_played.result`,
  applies the configured sort, bumps the recently-played generation, and (for
  the navigational variant) pushes the route ([success path](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/network/user.rs#L264-L280)).
* `plugin_api::recently_played_snapshot` returns the current cursor page’s
  `TrackInfo` items or an empty vector ([helper](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/plugin_api.rs#L701-L709)).
  The cursor page retains `next`, `cursors.after`, and optional `total`
  ([`CursorPaged<T>`](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/pagination.rs#L50-L80)),
  but no public `Action` or `IoEvent` currently requests a next recently-played
  page; `GetRecentlyPlayed` always performs the one `limit` request
  ([event dispatch](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/network/mod.rs#L756-L761)).

Spotify documents that this endpoint returns the current user’s recently played
tracks (not podcast episodes), requires `user-read-recently-played`, accepts
`limit` 1–50, and paginates with timestamp cursors `after`/`before`; `next` and
`cursors.after` identify a following page ([Spotify Get Recently Played Tracks](https://developer.spotify.com/documentation/web-api/reference/get-recently-played#request)).
Each item contains a track, `played_at` timestamp, and optional playback
context ([response item](https://developer.spotify.com/documentation/web-api/reference/get-recently-played#response)).
Spotatui currently discards `played_at` while mapping to `TrackInfo`, so a
Rust Player `LibraryEntry::Track.played_at_ms` value would require an added
frontend-facing field/type or a second mapping path.

### User playlists

* Playlists are not one of the current `LibraryTarget` variants (the enum has
  Discover, RecentlyPlayed, Friends, Stats, LikedSongs, Albums, Artists,
  Podcasts, and feature-gated rows only; see [enum](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/action/mod.rs#L351-L369)).
  The Spotify playlist sidebar is backed by `App::all_playlists` and its folder
  projection; `GetPlaylists` is dispatched during connected startup
  ([startup dispatch](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/driver/mod.rs#L665-L690)).
* `LibraryNetwork::get_current_user_playlists` calls
  `GET /v1/me/playlists?limit=50&offset=0`, maps each
  `SimplifiedPlaylist` to `PlaylistInfo`, publishes page 1 immediately, and
  starts a detached completion task ([initial request/mapping](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/network/library.rs#L731-L779)).
* `finish_playlists_fetch` follows offset pages (50 at a time) while `next` is
  present, appends mapped playlists to `all_playlists`, and finally publishes
  the complete list (or keeps the previous complete list if pagination fails)
  ([background pagination](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/network/library.rs#L574-L638);
  publish](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/network/library.rs#L679-L705)).
* A playlist maps to `PlaylistInfo` with URI, name, owner display name (owner id
  fallback), track count, playlist id, owner id, collaborative/public flags,
  and first image URL ([domain type](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/plugin_api.rs#L94-L115);
  mapper](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/plugin_api.rs#L517-L537)).
  `plugin_api::playlists_snapshot` returns the flattened `all_playlists` list
  ([helper](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/plugin_api.rs#L663-L666)).
* Internally, opening a playlist’s tracks uses
  `Action::Open(OpenTarget::Playlist { id, from_search: false })`; this target
  is not currently re-exported by `frontend`. The action calls
  `open_playlist_tracks`, which clears the table, pushes the route, and
  dispatches `GetPlaylistItems` for `playlists/{id}/items`
  ([action arm](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/action/apply.rs#L150-L163);
  open/fetch flow](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/app/playlist_pages.rs#L81-L106)).

Spotify defines `GET /v1/me/playlists` as the list of playlists owned or
followed by the current user. `playlist-read-private` is required for private
playlists; `playlist-read-collaborative` additionally includes collaborative
playlists. `limit` is 1–50 and `offset` is the index for the next page
([Spotify Get Current User’s Playlists](https://developer.spotify.com/documentation/web-api/reference/get-a-list-of-current-users-playlists#request);
[playlist scope rules](https://developer.spotify.com/documentation/web-api/concepts/playlists#reading-a-playlist)).
The response’s simplified playlist object includes id, name, owner, images,
public/collaborative flags, and an `items.total` track count
([response fields](https://developer.spotify.com/documentation/web-api/reference/get-a-list-of-current-users-playlists#response)).

## Authentication and event gating

Spotatui’s PKCE OAuth client requests all scopes needed by these three reads:
`user-library-read`, `user-read-recently-played`, `playlist-read-private`, and
`playlist-read-collaborative` are present in its static scope list
([OAuth scope list](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/auth.rs#L21-L38);
client construction](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/core/auth.rs#L212-L240)).
Spotify’s scope reference confirms `user-library-read` gates saved tracks,
`user-read-recently-played` gates recently played, and the playlist scopes gate
private/collaborative playlist listing ([Spotify scopes](https://developer.spotify.com/documentation/web-api/concepts/scopes#user-library-read)).

Every Spotify-bound `IoEvent` passes the network auth gate: if no Spotify client
exists, Spotatui shows “Spotify not connected…” and returns; otherwise it calls
`ensure_authentication_fresh(false)` before dispatching the event
([gate](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/network/mod.rs#L557-L598)).
Rust Player’s `frontend::Runtime::boot` runs the shared bootstrap and
authentication, then publishes immutable snapshots via a Tokio watch channel
([boot/subscribe](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/runtime/frontend/mod.rs#L111-L120);
[subscribe](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/runtime/frontend/mod.rs#L212-L215)).

## February 2026 Web API changes to keep in mind

Spotify’s first-party changelog says the old user-playlist endpoint
`GET /users/{user_id}/playlists` was removed in favor of `GET /me/playlists`,
and the old playlist-track path `GET /playlists/{id}/tracks` was removed in favor
of `GET /playlists/{id}/items`. It also lists `GET /me/tracks` and
`GET /me/player/recently-played` as still available
([February 2026 endpoint changes](https://developer.spotify.com/documentation/web-api/references/changes/february-2026#changes-to-endpoints);
[endpoints still available](https://developer.spotify.com/documentation/web-api/references/changes/february-2026#endpoints-still-available)).
The fork already uses `/me/playlists` and `/playlists/{id}/items` in its read
paths ([playlist list](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/network/library.rs#L753-L779);
[playlist items](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/network/library.rs#L849-L866)).

The same changelog renames playlist response fields from `tracks` to `items`
(`tracks.tracks` → `items.items`, and `tracks.tracks.track` →
`items.items.item`), which matches the fork’s `SimplifiedPlaylist.items.total`
and `PlaylistItem.item` mapping ([field changes](https://developer.spotify.com/documentation/web-api/references/changes/february-2026#changes-to-fields)).
Save/check mutations moved to `PUT`/`DELETE /me/library` and
`GET /me/library/contains`; the fork’s liked-state and save helpers already use
those library endpoints ([changelog](https://developer.spotify.com/documentation/web-api/references/changes/february-2026#changes-to-endpoints);
[fork library helpers](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/network/library.rs#L302-L364)).

## Existing scripting API (useful precedent, not exposed to Rust Player)

Spotatui’s first-party scripting documentation confirms that playlist data is
available as a cached `spotatui.playlists()` read and that asynchronous
`get_playlists`, `get_saved_tracks`, and `get_recently_played` reads request fresh
Spotify data and invoke a one-shot callback later ([cached reads](https://github.com/jhoogstraat/spotatui/blob/6fef473/docs/scripting.md#L141-L187);
[async reads](https://github.com/jhoogstraat/spotatui/blob/6fef473/docs/scripting.md#L189-L205)).
The scripting engine wires those data requests to `GetPlaylists`,
`GetCurrentSavedTracks`, and `GetRecentlyPlayedSilent`, then serializes the
same `plugin_api` snapshots ([request dispatch](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/scripting/engine.rs#L575-L608);
[cache refresh](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/scripting/engine.rs#L864-L890);
[Lua function registration](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/scripting/api.rs#L320-L345)).
This scripting API is separate from `frontend::Runtime`; it therefore cannot be
called from the GPUI adapter, but
their “fetched so far” semantics are a good model for a new frontend snapshot
field.

## Integration implications for the sidebar

1. Keep the existing `Command::Browse(LibrarySection)` contract in Rust Player,
   but add library data to Spotatui’s public frontend seam. The smallest useful
   addition is a source-neutral `frontend::LibrarySnapshot` carrying:
   * saved-track rows (`TrackInfo`) plus page metadata/`has_more`;
   * recently-played rows with `played_at` (the current `TrackInfo` mapping loses
     this field) and cursor metadata; and
   * playlist rows (`PlaylistInfo`) or a reduced playlist DTO.
2. Add the corresponding fields to `frontend::Snapshot` and populate them in
   `build_snapshot`; otherwise `Runtime::subscribe()` can never observe the
   successful internal requests. Use the existing generation/bell mechanism:
   network handlers mutate `App`, then the frontend snapshot publisher rebuilds
   and equality-deduplicates ([publisher](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/runtime/frontend/mod.rs#L145-L175)).
3. Re-export the action target types (or add frontend-specific browse actions),
   then route `Command::Browse(LikedSongs/RecentlyPlayed)` to
   `runtime.apply(Action::OpenLibrary(...))` once the added snapshot fields are
   available. For playlists, either add a `LibraryTarget::Playlists` action or
   expose the already-fetched `playlists_snapshot`; playlists currently have no
   `LibraryTarget` variant and are loaded by startup `GetPlaylists` instead.
4. If the GPUI playlist row should drill into tracks, send
   `Action::Open(OpenTarget::Playlist { id, from_search: false })`. For liked
   tracks, map each row’s URI/title/artists/album/duration to the existing
   `player_core::Playable` ([playable fields](../crates/player-core/src/lib.rs#L24-L43));
   for recently played, preserve the API’s `played_at` timestamp as
   `played_at_ms`.
5. The API’s 50-item cap means the liked-songs list must use
   `Action::LoadMore(ListTarget::SavedTracks)` (or a new frontend equivalent)
   for infinite scroll ([Spotify saved-track pagination](https://developer.spotify.com/documentation/web-api/reference/get-users-saved-tracks#request)).
   Spotatui already prefetches saved-track pages ([prefetch worker](https://github.com/jhoogstraat/spotatui/blob/6fef473/src/infra/network/library.rs#L80-L140)). The
   recently-played endpoint is cursor-based and currently fetched only once;
   add an explicit cursor action if the tab must scroll beyond 50 entries
   ([Spotify recently-played cursors](https://developer.spotify.com/documentation/web-api/reference/get-recently-played#request)).
