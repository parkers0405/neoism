# Neoism Mobile

The phone surface reuses Rune's agent transcript, activity indicator, composer controls, and workspace daemon connection. The Chats button toggles a sidebar with desktop's search and project grouping. Choose a device from the dropdown just below search; there is no separate workspace-selection step. Use **+ > Photo** in an existing chat to open the native picture picker. The app has no terminal, editor, or general-purpose file browser. It does not start a daemon on the phone.

On first launch, connect the Mac and phone to the same tailnet. From a local workspace in desktop Neoism, click the QR icon at the top right. Select **Scan QR Code** in Neoism Mobile; scanning the one-time code pairs immediately. In the Mac preview, select an image of the desktop QR code; in the iOS Simulator, paste the invite into **Pairing link** and select **Pair device**. Older device invites and password-protected Fast Servers still ask for their password. The phone remembers verified devices and retries the last connection on launch or after an interruption. **Choose device** is below search in Chats.

## Preview

To forget a saved system, open **Chats > Choose device**, tap the trash can to its right, and confirm **Forget system**. This removes the connection from this phone, not the chats or files on the host.

From `rune/app`, build the desktop binary and run the local browser viewer:

```sh
./build.sh build --locked --bin neoism-rune
python3 mobile/preview.py
```

Open [http://127.0.0.1:4173](http://127.0.0.1:4173). The viewer renders the real Rune UI with sample data, not a separate web chat client. Switch screens and sizes from the toolbar. After editing Rune code, rebuild the binary and click **Refresh render** without restarting the viewer.

In **Welcome**, click the wordmark or **Replay splash** to see the desktop wordmark effect rendered by Rune. **Settings** previews the phone's theme gallery. In the app, open **Chats > Settings** to apply any desktop palette; the choice is saved on the device, not on the host.

In **Chats > Settings > Backgrounds**, choose a picture file on the Mac preview or paste a picture copied from Photos on iOS. The page previews the picture behind sample chat text. **Show in chats** turns it off without forgetting the picture. Opacity, blur, pixelation, dithering, and fit are saved on this device.

To render a standalone screenshot:

```sh
./build.sh build --locked --bin neoism-rune
bin="${CARGO_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/neoism-rune-target}/debug/neoism-rune"
NEOISM_MOBILE=1 NEOISM_RUNE_OFFLINE=1 "$bin" --scene chat --size 390x844 --scale 2 --at 1 --snap /tmp/neoism-mobile-chat.png
```

Replace `chat` with `welcome`, `history`, or `models` to render another screen. For a fully interactive phone-sized Rune window on the Mac, run `NEOISM_MOBILE=1 NEOISM_RUNE_OFFLINE=1 "$bin" --scene chat --size 390x844`. Click the hamburger to test the drawer or focus the composer and type `/` to test the shared command picker. Omit `NEOISM_RUNE_OFFLINE` and `--scene chat` to test first-launch pairing against a shared workspace.

On Apple silicon, install `aarch64-apple-ios-sim` for the pinned Rust toolchain and run `bash mobile/simulator.sh booted` to build, install, and launch on the booted iOS Simulator. Open Simulator with `open -a Simulator`; no Xcode project is needed. To test with sample data, relaunch with `SIMCTL_CHILD_NEOISM_RUNE_OFFLINE=1 xcrun simctl launch --terminate-running-process booted ai.neoism.mobile --scene chat`. Native keyboard behavior still needs device verification.

## Physical iPhone

For TestFlight, use `bash mobile/device.sh --app-store BUILD_NUMBER /path/to/AppStore.mobileprovision 'Apple Distribution: ...'` from `rune/app`. Use an App Store Connect provisioning profile for `ai.neoism.mobile` and a build number not previously uploaded for this version. This produces an optimized, signed `.ipa` for Transporter and does not install it on a phone.

For on-device performance comparisons, pass `--release` as the first argument to `mobile/device.sh`. This builds an optimized app with the same bundle ID, so a development-signed install updates the app in place without restarting the host daemon.

Device-wide pairing uses the Mac's chat catalog and models across projects; provider sign-in stays on the Mac. Keep the one-time QR private: anyone who can reach the host and scan it before it expires can pair. If the desktop says quick pairing is unavailable, follow its daemon update or Tailscale instructions before generating a new code. Fast Server links are still supported as single-workspace invites.

Install the `aarch64-apple-ios` target for the pinned Rust toolchain. From `rune/app`, run `bash mobile/device.sh` to build an iPhoneOS bundle without provisioning, or `bash mobile/device.sh /path/to/development.mobileprovision 'Apple Development: …' DEVICE` to sign, install, and launch. The development profile must cover `ai.neoism.mobile`, the selected signing certificate, and the phone. Unlock the connected phone, trust the Mac, and enable Developer Mode. Use `xcrun devicectl list devices` to obtain its identifier. USB is for installation only; use the Fast Server QR code above to connect to the desktop workspace over the network.

Both native build scripts compile the app icon from the shipping `misc/macos/neoism.icns`; there is no separate mobile artwork to update. To check a bundle without installing or launching, use `assetutil --info "$bundle/Assets.car"` and `plutil -extract CFBundleIcons xml1 -o - "$bundle/Info.plist"`.

## Design references

- [T3 Code's public native client](https://github.com/pingdotgg/t3code/tree/main/apps/mobile): searchable sessions, a compact composer, and keyboard-aware layout. Use the inbox and composer patterns, not the thread header's desktop tool shortcuts.
- [Official OpenCode Mobile](https://opencode.ai/mobile): minimal thread layout and a short-lived host pairing link. The official product assets and host pairing code are public; a native phone client source tree was not verified.
- [Community OpenCode Mobile](https://github.com/alvarolorentedev/opencode-mobile): a separate source reference for keyboard avoidance and manual connection setup. Its permanent Terminal and Projects tabs are intentionally out of scope for Neoism Mobile.