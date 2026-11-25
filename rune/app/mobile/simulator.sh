#!/bin/bash
# Build a development .app bundle for Apple silicon's iOS Simulator.
set -euo pipefail
cd "$(dirname "$0")/.."
target=aarch64-apple-ios-sim
output="${CARGO_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/neoism-rune-target}"
bundle="$output/$target/debug/Neoism Mobile.app"
./build.sh build --locked --target "$target" --bin neoism-rune
mkdir -p "$bundle"
cp "$output/$target/debug/neoism-rune" "$bundle/neoism-rune"
cp mobile/Info.plist "$bundle/Info.plist"
minimum=$(xcrun vtool -show-build "$bundle/neoism-rune" | awk '$1 == "minos" { print $2; exit }')
plutil -insert MinimumOSVersion -string "${minimum:?Missing simulator deployment target}" "$bundle/Info.plist"
bash mobile/bundle-icon.sh "$bundle" iphonesimulator "$minimum"
codesign --force --sign - "$bundle"
printf '%s\n' "$bundle"
if [[ $# -gt 0 ]]; then
    xcrun simctl install "$1" "$bundle"
    xcrun simctl launch "$1" ai.neoism.mobile
fi