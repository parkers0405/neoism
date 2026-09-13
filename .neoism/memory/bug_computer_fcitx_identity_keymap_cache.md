---
name: "Computer use Fcitx identity-cache restoration: FIXED and live verified"
description: "FIXED + live verified: fresh baseline VK breaks Hyprland/Fcitx identity cache; exact URL+Unicode,84 paired keys through IME"
type: "bug"
scope: "project"
origin: "User live browser failure, upstream source, native tests and isolated compositor verification"
created: "2026-09-12"
updated: "2026-09-12"
---

Root cause source-verified on installed Hyprland0.56.2 efb5099 + Fcitx5.1.21: ordinary seat propagation handles repeated maps, BUT InputMethodV2.cpp CInputMethodKeyboardGrabV2::sendKeyboardData returns if keyboard identity == m_lastKeyboard. Neoism text VK sends overlay through IME grab; Fcitx mirrors it to persistent hl-virtual-keyboard-fcitx5. Same-VK original-map restoration never reaches the grab; Fcitx retains overlay and teardown can select it. Neoism connection roundtrips cannot acknowledge Fcitx's separate connection.

FIXED linux_text.rs Wire::restore: after held-key releases, create fresh baseline-only VK, publish original map, destroy previous VK, send neutral original-group modifiers, ACK, destroy replacement, ACK. No dummy keypress/clipboard/device reset. Retiring VK retained through panic cleanup. Fresh observer must receive canonical original map then50ms with no map events, within500ms. Native sync/observation uses deadline-aware poll instead unbounded roundtrip. Preflight checks cancellation, unknown temporary maps remain fail-closed. Preflight vs post-input errors separated; latter no false No input sent.

Deterministic linux_keyboard_tests models injector identities, IME identity+map cache, and Fcitx return queue one sync behind. Same-device restore and replacement-without-modifiers fail; destruction retains Fcitx map. Cancellation/panic/ACK and native parser tests retained, socket-pair timeout/cancel tests added. Harness29pass/2ignored; integrated45pass/2ignored; final desktop+server cargo check --tests passed pre-existing warnings only.

LIVE VERIFIED 2026-09-12: NEOISM_LIVE_KEYBOARD_TEST=1 python3 scripts/check-computer-keyboard-live.py --render-node /dev/dri/renderD129 --nested --trace-wayland. Exact production Ctrl+l -> URL -> Return -> Ctrl+l -> multilingual Unicode -> Return passed in1.60s, neutral/released keys and restored baseline at checkpoints. Private Fcitx trace proves84presses/84releases,115modifiers,9keymaps through actual IME grab. Logs /tmp/nk-live-4rmi36t0 including sources.sha256 and ime-evidence.txt. Launcher isolates runtime/home/config/dbus/input and uses render-only DRM; outer compositor may present host window but second client sandbox hides host Wayland socket. Pure headless no allocator on Aquamarine0.14; AQ_DRM_DEVICES render node doesn't initialize KMS allocator, nested Wayland works. Important test correction: text-input-v3 Done is edit-batch boundary, NOT activation ack; Fcitx can grab without sending Done. Launcher requires actual grab event evidence rather than false readiness gate.

Two earlier direct-host attempts stopped BEFORE input (already-held synthetic keys249/251/253, then focus loss). Never reset existing Fcitx or release unrelated keys. Native live test owns dedicated xdg surface, text-input-v3 receiver and per-action synchronized focus guard; no automatic refocusing. Existing desktop ghost state is separate from new code. User owns rebuild/relaunch; running target/debug/neoism Sep12 10:56 remains old, no desktop build/restart performed.
