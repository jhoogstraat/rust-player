#!/usr/bin/env bash
set -eu

# Reproduce issue #21 without mutating either checkout. Cargo may populate its
# shared cache, but no manifest, lockfile, or source file is written.
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SPOTATUI_DIR="${SPOTATUI_DIR:-$ROOT_DIR/../spotatui}"

echo "rust-player: $(git -C "$ROOT_DIR" rev-parse HEAD)"
echo "spotatui:   $(git -C "$SPOTATUI_DIR" rev-parse HEAD)"
echo "cargo:      $(cargo --version)"
echo

echo "== rust-player shipped engine leg (locked, default-features=false, streaming) =="
cargo tree --locked -p player-spotatui --depth 1 --edges normal
echo "unique package/version nodes: $(cargo tree --locked -p player-spotatui --prefix none --format '{p}' --edges normal | sort -u | wc -l | tr -d ' ')"
echo

echo "== sibling spotatui streaming graph (locked, no defaults) =="
(cd "$SPOTATUI_DIR" && cargo tree --locked --no-default-features --features streaming --depth 1 --edges normal)
echo "unique package/version nodes: $(cd "$SPOTATUI_DIR" && cargo tree --locked --no-default-features --features streaming --prefix none --format '{p}' --edges normal | sort -u | wc -l | tr -d ' ')"
echo

echo "== sibling spotatui default graph (comparison only) =="
(cd "$SPOTATUI_DIR" && cargo tree --locked --depth 1 --edges normal)
echo "unique package/version nodes: $(cd "$SPOTATUI_DIR" && cargo tree --locked --prefix none --format '{p}' --edges normal | sort -u | wc -l | tr -d ' ')"
echo

echo "== feature edges relevant to the audit =="
(cd "$SPOTATUI_DIR" && cargo tree --locked --no-default-features --features streaming --edges features | rg 'spotatui feature|rspotify feature|reqwest feature "blocking"|tokio feature "full"' || true)
echo

echo "== direct source reachability probes (sibling checkout) =="
(cd "$SPOTATUI_DIR" && for needle in 'tokio_tungstenite|tokio-tungstenite' 'futures::' 'keepawake|KeepAwake' 'webbrowser' 'reqwest::blocking' 'unicode_width'; do
  echo "-- $needle"
  rg -n "$needle" src Cargo.toml || echo "(no source/manifest match)"
done)
