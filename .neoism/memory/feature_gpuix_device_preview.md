---
name: "GPUIX device preview (optional native 3D)"
description: "Literal T3 sidebar rewrite, real native capture proofs, book presentation, drag fix, optional build and production launch workflow"
type: "feature"
scope: "project"
origin: "T3 screenshot visual-match revision"
created: "2026-11-01"
updated: "2026-11-01"
---

# Optional GPUIX device panel: literal T3 visual target

User strongly rejected generic slabs AND the settings-heavy panel. The supplied screenshot `pin-1650034-1-3fde3a6f5b931c65.png` is authoritative: large side-by-side open-book Duo, neutral almost-black stage, quiet right icon rail, no permanent back-header/status footer/forms/text-button toolbar. Do not claim visual parity from model-only/headless tests. Compare the complete production component. All geometry/artwork is original; never copy T3's unlicensed Apple GLBs, logos, or textures into Neoism.

## Current implementation

Build flag `NEOISM_GPUIX_DEVICE_PREVIEW=1`, default OFF. `scripts/device-preview-build.ts` replaces `src/device/feature.tsx` before graph traversal: ordinary packages exclude device UI/catalog/transport. Display build uses separately pinned `native/patches/gpuix-device-preview.patch` + device-preview-pin.json, separate optional source/target/stamp. New `native/patches/gpui-device-window-capture.patch` + device-window-capture-pin.json are included only in optional identity through `DisplayPin.deviceWindowCapture`; default source/artifact unchanged. Build source script applies base platform patch then capture patch, allowing only verified hashes.

Model IDs: iphone-18-pro, iphone-18-pro-max, iphone-duo, ipad-pro-13-m5. Catalog/finishes/exact simulator matching in src/device/models.ts. Pro defaults burgundy; Duo white titanium; tablet graphite. Native original indexed meshes, broad Pro camera shelves/triangular lenses/inset rear glass/rails/glass/ports/buttons, tablet landscape keyboard/stand/keys/trackpad, Duo inner/cover/hinge. Linear GGX/Smith/Schlick, exact sRGB conversions, analytic softboxes; no global Reinhard.32deg conventional/36degDuo FOV;~88.5%fit,4xMSAA,1536max render edge (raised from blurry768), same GPUI GPU/queue/compositor. Geometry/texture/bindings cached; no normal-frame readback/browser/second rendering device/idle animation timer.

Important rig distinction: asset-only T3 viewer without live setScreen rests portrait, but production physical presentation rolls open Duo into landscape/book. UI default `duoBook:true`,90deg opening,dead-front. Native default duoBook:false preserves portrait clamshell; book leaves pre-Z-quarter-turn geometry. `preview:true` only while no device selected shows original abstract wave art, not fabricated OS or failed-stream fallback. Raw1125x1600 inner frames quarter-turn into book once;1600x1125 frames don't double-rotate; picks map actual source UV. Conventional orientation from confirmed metadata rotates whole geometry/UV upright; Duo/keyboard generic Rotate is disabled. Closed book cover pose foldPI,yawPI/2,pitch0; portrait closed foldPI,yaw0,pitch-PI/2. `interaction.ts faceDevice` handles both.

UI: src/device/DevicePanel.tsx,DeviceChrome.tsx,DeviceIcon.tsx,DeviceViewport.tsx.30px small model tab, neutral black/light stage,56px rail zone/~46px pills,28px buttons/16px original SVG outlines, contextual compact menus. No permanent back/status footer. More opens left of its actual click, bottom-aligned. Tools holds connection settings; model title holds model/finish menu. Default preferred panel width540 when space allows. WorkspaceTabs accessory must be OUTSIDE workspace-tab-scroll (was incorrectly inside clipped scroller; fixed). Alt+Shift+D toggle; Alt-arrowfocus. Per-workspace ui.devicePreviewPanels; Git precedence. Only focused screen forwards text, never controls/menus. Close frees streams/decoder/GPU but never powers down simulator.

## Native API and regressions

Renderer createDeviceViewport/updateDeviceViewport/putDeviceFrame/clearDeviceFrame/dropDeviceViewport; host device_viewport viewportId/frameRevision. hitDeviceViewport(id,x,y,w,h)->JSON null or{x,y,display}; facingDeviceViewport->inner|cover. Options include model,color RGB,keyboard,duoBook,preview,display,orientation,yaw,pitch,hinge fold radians,zoom(.5..2),flat. Picking intersects same visible triangles, handles bars/occlusion/hinge/accessory; do not recreate model geometry inJS.

Critical runtime fix: orbit used to cancel its own gesture because createEffect reading props.pose.model/etc subscribed to the entire pose object; onPose cleared drag, then move wrote drag.x (crash). Use memoized interactionGeometry scalar key + memoized inputReady, cancel only true changes; capture `const gesture=drag` before callbacks. Continuous multi-move App regression exists. Full-window Anchored default wrongly painted#1A1A1A over whole app; optional anchored.rs now fallback fill only `!declares_fill&&!fill_window`, no transparent quad workaround.

Bulk automation_bounds used a future-frame UI query and timed out2s; now clones published painted_bounds like element_bounds. Linux captureScreenshot wired to UI command calling Window.render_to_image on last completed full scene without on_next_frame. Optional Zed patch ungates capture methods, forwards Wayland/X11 to SAME WgpuRenderer/device/atlas, normalizes BGRA/RGBA, strips row padding, max4096edge. Only explicit capture allocates/readbacks; no test-support scheduling. Existing isolated GPUI capture unit could not execute due uncached tree-sitter dependency; ACTUAL live full-panel PNG capture now passed including known burgundy swatch RGB and unchanged device pixels outside open menu.

## Transport

host.ts/protocol.ts/ffmpeg.ts: explicit existing expo-device-hub URL, serve-sim AVCC+taggedHID, serve-emuH264+JSON. No helper/SDK installs or implicit boot. iOS requires Mac+Xcode host. ffmpeg lazy software decode,max1280 output edge,never upscale,1filter thread,latest-frame coalescing,bounds/timeouts/abort cleanup. Env NEOISM_DEVICE_HUB_URL initial URL,NEOISM_DEVICE_FFMPEG executable. No WebRTC/physical-device fallback/screenshot polling.

Duo session.setDisplay uses physical0x10faceup/facedown request,0x90correlatedack,0x82election. Authoritative screenId1cover,3inner. Require ack+election+fresh restarted video before input; end old touch/reject stale epochs; reject/timeout closes uncertain session. Parent clears native frame on switch/election, follows only supported partly-open (>20,<180) resting faces on orbit RELEASE, not everymove. Non-Duo visual override ignores native mesh inner-label versus actual elected cover, mapping to live source correctly.

session.rotate/setOrientation: iOS documented0x07, matching config confirms,5s nonfatal timeout; Android policy isn't advertised as real capture rotation. Appearance Android /api/uimode GET/POST night readback only. Explicit screenshot POST /api/screenshot returns bounded PNG; UI writes unique ~/Downloads file on user click.404/405/501disable capability without stopping video. Devicescreenshots and windowautomationcapture differ.

## Verification / run

55 relevant Bun tests passed,9 native GPU/model/pick/color/cache tests passed,tsc passed. Scripts preview-device-panel.tsx/capture-device-panel.ts render the PRODUCTION component in native window (focus:false); full PNGs artifacts/device-preview/panel-*.png. Injected renderer MUST call initAsync and startFrameLoop explicitly: Solid render injected mode skips both. Fixture uses createWindowSize because Hyprland tiles regardless requested size. Screenshot pixel scale from PNGheight/panelheight, not requested width (window can be wider). Driver asserts popup preserves underlying model and burgundy swatch RGBA; all8 captures passed. Actual T3 private references in /tmp/neoism-device-reference, rendered with unmodified viewers, Chromium ONLY for comparison. T3 panel reference rail is actual component but outer CSS reconstructed; user screenshot supersedes it.

User explicitly authorized production build+launch. Command from frontend: NEOISM_GPUIX_DEVICE_PREVIEW=1 NEOISM_GPUIX_WORKSPACE_DAEMON=/home/parkersettle/.local/bin/neoism-workspace-daemon ./node_modules/.bin/bun scripts/build.ts --reuse-core. Existing installed daemon is needed because packaging default artifacts/target activation candidates absent. Never replace/stop original user's app/daemon. Build emits dist/neoism and optimized hashed .node. Default workflow still no release builds unless user asks.

Hyprland here uses NEW LUA dispatch (legacy focuswindow/fullscreen strings fail). --maximized / native zoom is ignored by tiling. To expand ONLY owned preview: inspect hyprctl clients -j, pick exact PID/address; `hyprctl dispatch 'hl.dsp.focus({window="address:..."})'`, then `hyprctl dispatch 'hl.dsp.window.fullscreen_state({internal=1,client=1,window="address:..."})'`. This produced1256x674 app,540px device panel,469x540 viewport. Source launch.ts/main.tsx now respect GPUIX_BACKGROUND=1 for focus:false during checks; native compositor may override. Use owned exact addresses; never act on original app. Latest production preview was left running maximized on Duo book, with an owned loopback launcher-status controller at127.0.0.1:39057 (runtime transient, don't assume it exists later).
