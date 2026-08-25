# Separate UI, player core, and source adapters

The application will consist of a GPUI-only application crate, a source-neutral `player-core`, and a source adapter over the private Spotatui fork. GPUI never imports Spotatui or source-specific types. The adapter advertises only Spotify in version one but may advertise Spotatui's existing YouTube engine as a second Music Source later; this preserves working source code without making Spotatui's internal model the product's public core.

Clarification (2026-08-25): the adapter (`crates/player-spotatui`) lives in this repository. The fork exposes a Spotatui-shaped `frontend` module and never depends on this product's types, which keeps the dependency one-directional and the fork product-agnostic.
