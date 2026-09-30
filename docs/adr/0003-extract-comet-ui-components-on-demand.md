# Extract Comet UI components on demand

The application will use Comet's GPUI conventions and move only components it actually needs into its own GPUI code. Depending on all of `zeron-ui` would pull unrelated Comet domains into the player, while recreating the components would discard proven work; demand-driven extraction preserves reuse without accepting either cost.

Clarification (2026-08-25): version one has no demand. Its window needs a palette, a text input, and a plain Queue panel, all smaller than Comet's `theme` and `popover` modules, and the popover depends on fork-only GPUI APIs. Comet's GPUI revision is pinned as a known-good build only; the application must not call fork-only APIs so the pin stays replaceable. Comet is read for patterns (Tokio bridge, reopen, quit), not imported.

Exception (2026-09-30): `apps/player/src/macos_blur.rs` messages AppKit
directly. The pinned GPUI builds its blur view with the `Selection` material,
which macOS 27 draws without a backdrop blur, so the app switches that view to
`HUDWindow` after the window opens. Delete the file when the GPUI pin blurs on
macOS 27 by itself.
