# Model music sources explicitly

Spotify is the first Music Source, not the product boundary. The application will expose source capabilities through a small source abstraction so the GPUI layer does not depend on Spotify types; this costs a little structure now but prevents Spotify-specific concepts from defining every future screen and action.
