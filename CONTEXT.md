# Music Player

This context describes how people browse music from one or more providers and
choose where it plays.

## Language

**Music Source**:
A provider of browsable music and the capabilities available for interacting with it. Spotify is the first Music Source, not the definition of one.
_Avoid_: Streaming service, provider, backend

**Active Music Source**:
The Music Source currently selected for browsing and search.
_Avoid_: Current provider, selected service

**YouTube Source**:
An experimental Music Source for finding and playing public YouTube videos without representing a YouTube Music account or its library.
_Avoid_: YouTube Music, YouTube Music integration

**Playback Device**:
An endpoint capable of producing music audio for the listener.
_Avoid_: Player, client, speaker

**Native Playback**:
Playback for which this application is the active Playback Device and produces the audio locally.
_Avoid_: Local playback, embedded playback

**Remote Playback**:
Playback controlled by this application while another Playback Device produces the audio.
_Avoid_: Connect playback, external playback

**Playable**:
A piece of source-owned content resolved far enough that a Playback Session can queue or start it.
_Avoid_: Track, media item, song

**Playback Session**:
The listener's current playback activity, including its active Playable, queue, transport state, and Playback Device.
_Avoid_: Player, playback state, audio session

**Queue**:
An ordered sequence of source-scoped Playables scheduled within a Playback Session. A Queue may contain Playables from different Music Sources.
_Avoid_: Playlist, play order

**Catalog Availability**:
Whether a Music Source can currently answer browsing, search, and metadata requests.
_Avoid_: Connection status, source health

**Playback Health**:
Whether the active Playback Session can continue producing audio independently of Catalog Availability.
_Avoid_: Source status, connection status
