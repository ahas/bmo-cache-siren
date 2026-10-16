#!/bin/bash
# Build the cache-siren overlay: swiftc -> dist/CacheSiren.app (LSUIElement, ad-hoc signed; no TCC needed) -> relaunch.
set -euo pipefail
cd "$(dirname "$0")"
BUNDLE_ID="local.cache-siren"
APP="../dist/CacheSiren.app"
mkdir -p ../dist
echo "▸ swiftc…"
swiftc -O -framework Cocoa -framework WebKit main.swift -o /tmp/CacheSiren.bin
rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS"
cp /tmp/CacheSiren.bin "$APP/Contents/MacOS/CacheSiren"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>CacheSiren</string>
  <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
  <key>CFBundleName</key><string>cache-siren</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --sign - "$APP" >/dev/null 2>&1 && echo "▸ ad-hoc signed"
UID_="$(id -u)"
if launchctl print "gui/$UID_/${BUNDLE_ID}.app" >/dev/null 2>&1; then
  launchctl kickstart -k "gui/$UID_/${BUNDLE_ID}.app"; echo "▸ kickstarted launchd app"
else
  pkill -x CacheSiren 2>/dev/null || true; sleep 0.3; open "$APP"; echo "▸ opened $APP"
fi
echo "✅ built $APP"
