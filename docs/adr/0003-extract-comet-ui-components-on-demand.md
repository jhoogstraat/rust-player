# Extract Comet UI components on demand

The application will use Comet's GPUI conventions and move only components it actually needs into its own GPUI code. Depending on all of `zeron-ui` would pull unrelated Comet domains into the player, while recreating the components would discard proven work; demand-driven extraction preserves reuse without accepting either cost.

Clarification (2026-08-25): version one has no demand. Its window needs a palette, a text input, and a plain Queue panel, all smaller than Comet's `theme` and `popover` modules, and the popover depends on fork-only GPUI APIs. Comet's GPUI revision is pinned as a known-good build only; the application must not call fork-only APIs so the pin stays replaceable. Comet is read for patterns (Tokio bridge, reopen, quit), not imported.
