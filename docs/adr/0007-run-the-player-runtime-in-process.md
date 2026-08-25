# Run the player runtime in process

The first release will embed `player-core` and its source adapters in the GPUI process, using Comet's Tokio/GPUI task bridge and graceful shutdown pattern. Comet's daemon and RPC topology solves multi-viewport and headless requirements this product does not have, so copying it would add a second process boundary before there is a consumer for it.
