#!/bin/bash
# Compile the shipping desktop art as an iOS AppIcon. Run before signing either bundle.
set -euo pipefail
bundle=$1
platform=$2
minimum=$3
mobile=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
iconset="$tmp/Neoism.xcassets/AppIcon.appiconset"
mkdir -p "$iconset"
cp "$mobile/AppIcon.appiconset/Contents.json" "$iconset/Contents.json"
xcrun --sdk macosx swift "$mobile/render-app-icon.swift" \
    "$mobile/../../../misc/macos/neoism.icns" "$iconset/AppIcon.png"
xcrun --sdk "$platform" actool --compile "$bundle" --platform "$platform" \
    --minimum-deployment-target "$minimum" --target-device iphone --target-device ipad \
    --product-type com.apple.product-type.application --app-icon AppIcon \
    --output-partial-info-plist "$tmp/icons.plist" "$tmp/Neoism.xcassets"
for key in CFBundleIcons 'CFBundleIcons~ipad'; do
    icons=$(plutil -extract "$key" xml1 -o - "$tmp/icons.plist")
    plutil -replace "$key" -xml "$icons" "$bundle/Info.plist"
done