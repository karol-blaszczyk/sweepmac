#!/usr/bin/env bash
# Build sweepmac.app (a menu-bar agent bundling all three binaries) and a
# sweepmac.dmg installer. macOS only — uses sips / iconutil / hdiutil.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP="dist/sweepmac.app"
ICONSET="dist/sweepmac.iconset"
BASE_PNG="dist/icon-1024.png"

echo "==> building release binaries (gui tray pkg)"
cargo build --release --features "gui tray pkg"

echo "==> assembling $APP"
rm -rf dist
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$ICONSET"
cp target/release/sweepmac target/release/sweepmac-gui target/release/sweepmac-tray \
    "$APP/Contents/MacOS/"
cp packaging/Info.plist "$APP/Contents/Info.plist"

echo "==> generating icon"
target/release/sweepmac-iconfile "$BASE_PNG" 1024
# Standard iconset sizes (1x and 2x).
for s in 16 32 128 256 512; do
    sips -z "$s" "$s" "$BASE_PNG" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
    d=$((s * 2))
    sips -z "$d" "$d" "$BASE_PNG" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/sweepmac.icns"

echo "==> building dmg"
# Stage the .app next to an Applications shortcut so users can drag-to-install.
STAGE="dist/dmg"
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname sweepmac -srcfolder "$STAGE" -ov -format UDZO dist/sweepmac.dmg >/dev/null
rm -rf "$STAGE"

echo "==> done:"
echo "    $APP"
echo "    dist/sweepmac.dmg"
