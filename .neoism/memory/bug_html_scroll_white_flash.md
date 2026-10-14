---
name: "HTML preview white flash on scroll stop fixed"
description: "Servo hide/show removed offscreen iframe; reactivation published white scene. Keep compositor scene resident; real-pixel 12-cycle regression passes."
type: "bug"
scope: "project"
origin: "User report, real-worker reproduction and pixel-level regression test"
created: "2026-10-07"
updated: "2026-10-07"
---

User reported white flash on every scroll stop after prior cached-frame/header/timeline fixes (2026-10-07). FIXED in neoism-frontend/servo-runtime/src/engine.rs; debug worker rebuilt, existing target/debug/neoism-servo-runtime symlink points to it. Desktop binary unchanged; currently running app must reopen to start new helper.
Cause actually reproduced by live_worker.py: desktop scroll suppression/occlusion causes document.visible false→true. Servo WebView.hide() asynchronously removes its iframe from offscreen compositor root. Reactivation could paint/read EMPTY root scene, yielding opaque white (255,255,255,255) with valid key/generation/revision/size metadata. Simply removing forced dirty and waiting frame_ready did NOT fix it.
Correct fix keeps each view iframe resident in its DEDICATED offscreen compositor scene; logical visibility gates CPUpaint/readback and public animation scheduling/framepublication only. No repeated destructive compositor hide/show on scroll, no document reload/context recreation, no pixel-rejection heuristic, no hardcoded background/theme override. DOM state retained.
Regression runtime/tests/live_worker.py now tests12 idle/rapid hidden→visible scroll cycles; ALL480000rasterbytes unchanged across EVERYpublishedframe. Hidden rAF stops publication/scheduling and resumeswhenactive. Legitimate white theme stillrendered withDOMstateintact. Counter/text,themechanges,resize,networkdenial(localserver0requests),cleanstdout/shutdown pass onMesa/llvmpipe.5runtimeunit tests pass. Parent independentlyexecuted DISPLAY=:0 WAYLAND_DISPLAY=wayland-1 python3 neoism-frontend/servo-runtime/tests/live_worker.py; fullpass captured/tmp/neoism-servo-white-flash-live.log. Old target/live-worker*.result.log files predatefix; don't cite them forwhite regression, usecurrentlog/currentharness.
Related previous UI bugs in feature_inline_html_artifacts.md: cachedrender independentofinteractiongating, rastergeometry notfloatingtranslation invalidation, headerless fixed320slot and rawHTML source-line height estimates fixed. Normaldesktopdefaults include servo-artifacts. Workingcommand NEOISM_SERVO_ARTIFACTS=1 ./target/debug/neoism; sourceworkeractualdebugbinarybuilt~1.6GBwithsymbols,app~742MB. SeparateFreeTypegraph; noOSsandbox, trustedoptin remains. User insistsstayServofornow; noCEFswitch. Avoid proposingdiagnostics-only whenuserasksworkingbehavior. No --releasebuild/noextraGUIwindow forfix.
