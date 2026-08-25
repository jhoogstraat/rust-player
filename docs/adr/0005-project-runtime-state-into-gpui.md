# Project runtime state into GPUI

The frontend will follow Comet's state-projection approach: Spotatui remains authoritative and publishes immutable, source-neutral snapshots when meaningful state changes, while GPUI keeps only presentation state and derives continuously moving progress locally from timestamps. This avoids exposing or polling Spotatui's large mutable `App` and prevents playback timing from forcing full backend snapshots every frame.
