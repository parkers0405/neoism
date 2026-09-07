---
name: "Windows helper windows and connect readiness"
description: "Windows helper-window and /connect fixes implemented; monitored startup, actual-endpoint readiness; combined checks passed; native visual acceptance outstanding; .93 cancelled"
type: "bug"
scope: "project"
origin: "Windows launch/connect follow-up after cancelled v0.7.93"
created: "2026-09-07"
updated: "2026-09-07"
---

After v0.7.92 native ConPTY fix (remove CREATE_NO_WINDOW from pseudoconsole shell only), user reported Windows console-window storm plus /connect UI freeze and agent unavailable. Cancelled v0.7.93 Actions run 34080126441; confirmed completed/cancelled and no release published. Tag/main remain at .93 source; do not resume publication without request. Follow-up fixes implemented locally, uncommitted/unreleased.

Concrete window causes: adapter ConfigSourceService::snapshot -> migrate_project_config -> unhidden git rev-parse every snapshot BEFORE checking legacy neoism.json. Fixed check-before-spawn and hidden service-api helper for remaining config Git/workspace Git/icacls; update curl/updater and ACP pipe-backed direct child hidden; tasklist updater polling hidden. Ordinary background console processes need hiding flags; ConPTY shells must KEEP CREATE_NO_WINDOW removed (native tests showed needed for input). No /STACK changes.

/connect called provider/auth HTTP and8sec readiness sleeps synchronously on UI. Connect catalog/accounts/credential/OAuth/browser work now workers with completion notifier, loading/retry/cancellation/request tokens. Checks actual endpoint; joined health probes retain registered bearer. Windows worker COM initialization for browser launch. No timeout inflation. 227 desktop-agent tests passed incl14 connect regressions.

Startup previously accepted bound TCP as ready, while unbounded Tailscale CLI could block before agent supervisor launched; child-local ready signal was ignored by parent. New hidden service single weak-owner registry, stdout READY ack + daemon /health identity, nonfatal GUI3sec patience separate120sec boot deadline, capped retries, service logs config/log/workspace-service.log. Tailscale direct hidden child output/time bounded. Linked-agent initial readiness/retry/panic supervision. ensure_started_for_request(server)->Result checks actual health; rejects ownership of remote/proxy/wrong-port. Runs probe runtime on plain worker if caller already has Tokio; shutdown_background avoids hanging on DNS after deadline. Seven readiness tests incl current-thread/multithread/spawn_blocking passed; service/discovery targeted regressions also passed.

Parent strengthened scripts/windows-installed-gui-smoke.ps1: sample visible Neoism/console-helper HWNDs every100ms, require Agent v2 health and main responsiveness, capture diagnostics. Sampling can miss shorter-lived windows; startup is NOT real keyboard/composer/GPU acceptance. tests/windows_script_syntax.rs parses PS and compiles C# window probe through ConPTY pwsh under Wine (passed). Direct Wine pwsh -Command can silently exit0 without executing (sentinel exit43 exposes it); use ConPTY harness.

FINAL combined env -u CI cargo check -p neoism -p neoism-workspace-daemon --tests passed; Windows cargo xwin check same production crates --no-default-features --features wgpu passed; git diff --check passed. Adapter/service36 Linux-native tests passed. These checks are not native Windows visible-window or full browser acceptance, which remains outstanding. No local release builds, commit, push or restarted release.

Microsoft sources: learn.microsoft.com/en-us/windows/console/creating-a-pseudoconsole-session and /windows/win32/procthread/process-creation-flags. Prior .92 native GUI smoke checked neither extras nor Agent readiness and could pass these bugs; logs /tmp/neoism-v0.7.92-windows-evidence.
