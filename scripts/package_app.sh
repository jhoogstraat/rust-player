#!/bin/zsh
# Build the unsigned Rust Player .app for macOS and verify it is
# self-contained: no dynamic library in the bundle may reference a Homebrew
# prefix. Signing and notarization are separate release gates (out of scope).
#
# Usage: scripts/package_app.sh
#
# The bundle runs the real runtime. To exercise the bundle without Spotify,
# launch it with the scripted fake:  open "target/pkg/Rust Player.app" --args --fake

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

APP_NAME="Rust Player"
BUNDLE_ID="dev.rustplayer.app"
BUILD_ROOT="$ROOT/target/pkg"
APP_DIR="$BUILD_ROOT/Rust Player.app"
CONTENTS="$APP_DIR/Contents"

echo "==> building release binary"
cargo build --release -p rust-player

echo "==> locating libportaudio"
PORTAUDIO="$(brew --prefix portaudio)/lib/libportaudio.2.dylib"
[[ -f "$PORTAUDIO" ]] || { echo "error: libportaudio not found — brew install portaudio" >&2; exit 1; }

echo "==> assembling bundle"
rm -rf "$APP_DIR"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Frameworks" "$CONTENTS/Resources"

BIN="$CONTENTS/MacOS/rust-player"
cp "$ROOT/target/release/rust-player" "$BIN"
cp "$ROOT/docs/SMOKE_TEST.md" "$CONTENTS/Resources/" 2>/dev/null || true

cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>$APP_NAME</string>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleExecutable</key><string>rust-player</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/apps/player/Cargo.toml" | head -1)</string>
    <key>LSMinimumSystemVersion</key><string>10.15</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

echo "==> bundling libportaudio"
# Copy first, then rewrite the copy's own install name (-id) and every
# reference to it in the executable — never mutate the Homebrew original.
cp "$PORTAUDIO" "$CONTENTS/Frameworks/"
install_name_tool -id "@rpath/libportaudio.2.dylib" \
  "$CONTENTS/Frameworks/libportaudio.2.dylib"
install_name_tool -change "$PORTAUDIO" "@rpath/libportaudio.2.dylib" "$BIN"
# The fork's PortAudio backend loads it through librespot-playback as well.
for dylib in $(otool -L "$BIN" | awk '{print $1}' | grep -E '\.(dylib|so)$' || true); do
  if [[ "$dylib" == *portaudio* && -f "$CONTENTS/Frameworks/$(basename "$dylib")" ]]; then
    install_name_tool -change "$dylib" "@rpath/$(basename "$dylib")" "$BIN"
  fi
done
install_name_tool -add_rpath "@executable_path/../Frameworks" "$BIN"

codesign --force --deep --sign - "$APP_DIR" >/dev/null 2>&1 || true

echo "==> verifying no Homebrew paths in the bundle's dynamic closure"
FAIL=0
while IFS= read -r dylib_path; do
  [[ -z "$dylib_path" ]] && continue
  while IFS= read -r dep; do
    if [[ "$dep" == *"/opt/homebrew"* || "$dep" == *"/usr/local/Homebrew"* ]]; then
      echo "  HOMEBREW LEFT: $dylib_path -> $dep"; FAIL=1
    fi
  done < <(otool -L "$dylib_path" | awk 'NR>1 {print $1}')
done < <(find "$APP_DIR" -type f \( -perm -111 -o -name "*.dylib" \) -print)

if [[ $FAIL -ne 0 ]]; then
  echo "error: bundle still references Homebrew paths" >&2
  exit 1
fi

echo "==> notices"
cat > "$CONTENTS/Resources/LICENSES.txt" <<'NOTICES'
Rust Player
- GPUI (Apache-2.0) — https://github.com/wingleeio/zed
- Spotatui fork & librespot stack (MIT/Apache-2.0) — see the fork repository
- PortAudio (MIT) — https://github.com/PortAudio/portaudio
NOTICES

echo
echo "bundle ready: $APP_DIR"
echo "next: manual smoke test per docs/SMOKE_TEST.md"
