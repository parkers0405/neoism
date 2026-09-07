---
name: "False extra-window smoke failure and blocked Windows service startup"
description: "Native .94/.95 smoke false-positive: winit event HWND needs exact class/style filter, not rectangle alone; service startup fixed; corrected MSI validation passed natively"
type: "bug"
scope: "project"
origin: "Native .94/.95 failure and successful MSI revalidation34142431880"
created: "2026-09-07"
updated: "2026-09-07"
---

v0.7.94 run34083058638 and v0.7.95 run34126898861 both failed installed-MSI smoke on 'Winit Thread Event Target'. This is a winit internal HWND with WS_VISIBLE used for WM_PAINT/event dispatch, NOT an extra visible app window. Source neoism-window/src/platform_impl/windows/event_loop.rs create_event_target_window creates0x0 with NOACTIVATE|TRANSPARENT|LAYERED|TOOLWINDOW. IMPORTANT: GetWindowRect can STILL report positive area on native Windows; .95 proved a rectangle-only filter insufficient. Final detector excludes ONLY exact class plus full extended-style mask0x080800A0, logs infrastructure HWNDs/styles/geometry in window-census.jsonl, and chooses main window from non-infrastructure snapshot. Do not whitelist all neoism/tool/console windows. PowerShell regression checks ensure regular Neoism/ConsoleWindowClass/CASCADIA_HOSTING_WINDOW_CLASS are NOT excluded, and class without required styles is not excluded.

Native .94 evidence also exposed service probe treating all connect errors except ConnectionRefused as occupied. Fixed in .95: only established connection with invalid/no daemon health means occupied; a failed TCP connect permits launching the owned service (actual bind final, capped backoff retained). Six service tests and Linux/Windows production checks passed. .95 actual workspace-service.log confirms agent startup, database initialized, /v2/health200 and 'embedded Neoism Agent ready'. Its only smoke failure was window classifier false positive.

Final corrected validator commit0af7a0b6c. Added manual-only .github/workflows/validate-windows-msi.yml: download original existingMSI artifact, verify checksum/version, install, run current validator on UNMODIFIED existing binary, uninstall finally, upload full evidence. No rebuild/publish. Native run34142431880 on windows-latest PASSED all steps in1m2s using source34126898861 version0.7.95. Verifies150responsive samples, agentauto-start health, no additional app/console-helper windows observed. Not a claim of full keyboard/GPU/interactive /connect acceptance. Preparing normal .96 release with corrected validator; old tags untouched.

Evidence /tmp/neoism-v0.7.94-windows-evidence and /tmp/neoism-v0.7.95-windows-evidence. Nativepass artifact windows-msi-validation-34142431880-1 includes originalMSI (~80MB), downloading can hit TLS/timeouts; job success via gh run view/watch authoritative. Earlier cancelled .93 remains unpublished; .94/.95 drafts hidden.
