#!/bin/bash
# Build a native iPhone bundle. With a development profile and signing identity, sign it.
set -euo pipefail
build_dir=debug
build_args=()
store_build=
if [[ ${1:-} == --app-store ]]; then
    store_build=${2:-}
    if [[ $# != 4 || ! "$store_build" =~ ^[1-9][0-9]{0,3}$ || -z ${3:-} || -z ${4:-} ]]; then
        printf 'Usage: bash mobile/device.sh --app-store BUILD_NUMBER PROFILE SIGNING_IDENTITY\n' >&2
        exit 2
    fi
    shift 2
fi
if [[ ${1:-} == --release || -n "$store_build" ]]; then
    build_dir=release
    build_args+=(--release)
    if [[ ${1:-} == --release ]]; then shift; fi
fi
if [[ $# != 0 && $# != 2 && $# != 3 ]]; then
    printf 'Usage: bash mobile/device.sh [--release] [PROFILE SIGNING_IDENTITY [DEVICE]]\n' >&2
    exit 2
fi
profile=${1:-}
if [[ -n "$profile" && "$profile" != /* ]]; then profile="$PWD/$profile"; fi
cd "$(dirname "$0")/.."
target=aarch64-apple-ios
output="${CARGO_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/neoism-rune-target}"
bundle="$output/$target/$build_dir/Neoism Mobile.app"
if [[ -n "$store_build" ]]; then bundle="$output/$target/release/app-store/Payload/Neoism.app"; fi
plist="$bundle/Info.plist"
bundle_id=ai.neoism.mobile
if [[ -n "$profile" ]]; then
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    security cms -D -i "$profile" -o "$tmp/profile.plist"
    team=$(/usr/libexec/PlistBuddy -c 'Print :Entitlements:com.apple.developer.team-identifier' "$tmp/profile.plist")
    allowed=$(/usr/libexec/PlistBuddy -c 'Print :Entitlements:application-identifier' "$tmp/profile.plist")
    app_id="$team.$bundle_id"
    case "$app_id" in $allowed) ;; *) printf 'The profile does not cover %s.\n' "$bundle_id" >&2; exit 1 ;; esac
    debug_allowed=$(/usr/libexec/PlistBuddy -c 'Print :Entitlements:get-task-allow' "$tmp/profile.plist" 2>/dev/null || true)
    plutil -create xml1 "$tmp/entitlements.plist"
    plutil -insert application-identifier -string "$app_id" "$tmp/entitlements.plist"
    /usr/libexec/PlistBuddy -c "Add :com.apple.developer.team-identifier string $team" "$tmp/entitlements.plist"
    if [[ -n "$store_build" ]]; then
        if [[ "$debug_allowed" == true || "$allowed" != "$app_id" ]] ||
            /usr/libexec/PlistBuddy -c 'Print :ProvisionedDevices' "$tmp/profile.plist" >/dev/null 2>&1 ||
            [[ $(/usr/libexec/PlistBuddy -c 'Print :ProvisionsAllDevices' "$tmp/profile.plist" 2>/dev/null || true) == true ]]; then
            printf 'Use an App Store Connect distribution profile for %s.\n' "$bundle_id" >&2
            exit 1
        fi
    else
        if [[ "$debug_allowed" != true ]] || ! /usr/libexec/PlistBuddy -c 'Print :ProvisionedDevices:0' "$tmp/profile.plist" >/dev/null; then
            printf 'Use an iOS development provisioning profile.\n' >&2
            exit 1
        fi
        plutil -insert get-task-allow -bool YES "$tmp/entitlements.plist"
    fi
fi
export IPHONEOS_DEPLOYMENT_TARGET=16.0
export SDKROOT=$(xcrun --sdk iphoneos --show-sdk-path)
./build.sh build --locked --target "$target" --bin neoism-rune ${build_args[@]+"${build_args[@]}"}
mkdir -p "$bundle"
cp "$output/$target/$build_dir/neoism-rune" "$bundle/neoism-rune"
cp mobile/Info.plist "$plist"
plutil -replace CFBundleSupportedPlatforms -json '["iPhoneOS"]' "$plist"
binary_build=$(xcrun vtool -show-build "$bundle/neoism-rune")
minimum=$(awk '$1 == "minos" { print $2; exit }' <<< "$binary_build")
sdk=$(xcrun --sdk iphoneos --show-sdk-version)
binary_sdk=$(awk '$1 == "sdk" { print $2; exit }' <<< "$binary_build")
if [[ "$minimum" != "$IPHONEOS_DEPLOYMENT_TARGET" || "$binary_sdk" != "$sdk" ]]; then
    printf 'Unexpected Mach-O deployment target or SDK (minimum %s, SDK %s; expected %s and %s).\n' "$minimum" "$binary_sdk" "$IPHONEOS_DEPLOYMENT_TARGET" "$sdk" >&2
    exit 1
fi
plutil -insert MinimumOSVersion -string "$minimum" "$plist"
bash mobile/bundle-icon.sh "$bundle" iphoneos "$minimum"
if [[ -n "$store_build" ]]; then plutil -replace CFBundleVersion -string "$store_build" "$plist"; fi
if [[ -n "$store_build" ]]; then
    sdk_build=$(xcrun --sdk iphoneos --show-sdk-build-version)
    xcode=$(xcodebuild -version)
    xcode_version=$(awk '/^Xcode / { split($2, v, "."); printf "%02d%01d%01d", v[1], v[2], v[3] }' <<< "$xcode")
    xcode_build=$(awk '/^Build version / { print $3 }' <<< "$xcode")
    plutil -insert DTPlatformName -string iphoneos "$plist"
    plutil -insert DTPlatformVersion -string "$sdk" "$plist"
    plutil -insert DTPlatformBuild -string "$sdk_build" "$plist"
    plutil -insert DTSDKName -string "iphoneos$sdk" "$plist"
    plutil -insert DTSDKBuild -string "$sdk_build" "$plist"
    plutil -insert DTXcode -string "${xcode_version:?Missing Xcode version}" "$plist"
    plutil -insert DTXcodeBuild -string "${xcode_build:?Missing Xcode build}" "$plist"
    plutil -insert BuildMachineOSBuild -string "$(sw_vers -buildVersion)" "$plist"
    xcrun --sdk iphoneos ibtool --minimum-deployment-target "$minimum" --target-device iphone --target-device ipad \
        --compile "$bundle/LaunchScreen.storyboardc" mobile/LaunchScreen.storyboard
    plutil -remove UILaunchScreen "$plist"
    plutil -insert UILaunchStoryboardName -string LaunchScreen "$plist"
fi
if [[ -n "$profile" ]]; then
    cp "$profile" "$bundle/embedded.mobileprovision"
    codesign --force --sign "$2" --entitlements "$tmp/entitlements.plist" "$bundle"
    codesign --verify --strict "$bundle"
    if [[ -n "$store_build" ]]; then
        ipa="${bundle%/Payload/Neoism.app}/Neoism-$store_build.ipa"
        ditto -c -k --norsrc --keepParent "$(dirname "$bundle")" "$ipa"
        printf '%s\n' "$ipa"
        exit 0
    fi
    if [[ $# == 3 ]]; then
        xcrun devicectl device install app --device "$3" "$bundle"
        xcrun devicectl device process launch --device "$3" "$bundle_id"
    fi
else
    rm -f "$bundle/embedded.mobileprovision"
    printf 'Built without a provisioning profile; not installable on an iPhone yet.\n' >&2
fi
printf '%s\n' "$bundle"