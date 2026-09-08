---
name: "Background task ghost resurrected from history"
description: "GUI resurrected old release-monitor launch after authoritative empty runtime snapshot; fixed epoch/revision authority and family event scope, 56 tests passed"
type: "bug"
scope: "project"
origin: "User stale-running-task report on0.7.96"
created: "2026-09-08"
updated: "2026-09-08"
---

Reproduced stale running-background-task GUI after latest final-check job job_07e60c21b0011057jStjdMhO1t completedexit0. Parent DID receive runtimecompletion and persistedcompletionmessage exists. Live /v2/sessions/<root>/runtime returned runningBackgroundTasks:[] but GUI history/completion-card rescan resurrected old unmatched launch job_07cee06f3001sWyAZrsDWxC1rG ('Monitor remaining Windows release job with resilient status polling'). Subsequent same-revision snapshots rejected, ghost persisted. Old monitor final exit unknown; launch history is not evidence of current runningstate.

Fix: shared BackgroundTaskAuthority tracks epoch/revision/livejobIDs including authoritativeempty lists; transcript rescans can't overwrite it. Active titles derive fromlive IDs, not unmatched launches. Retired epochs and stalerevisions rejected. Root/child switches preservefamily authority; reconnect hydration processesbackgroundjobrevisions independently fromexecution/statusrevisions. Server background_job publishes family-wide jobs acrossworkspaces (not onlyworkspace that finishedjob) and targetsfamilyroot. Completionnotification/carddedupe maintained. Desktop/shared/wasmupdated; daemonproxytestonly.

56focusedtests passed (29shared,19desktop,6server,2proxy), reproduced count1instead0 pre-fix. Linuxruntime/UIchecks --tests,wasm,Windowsproduction+sharedtesttargetcrosscheck passed. No native WindowsGUI runtime validation. Main sharedfiles/LSP/Git modifications preserved; no commits/release peruser. Installed0.7.96 continuesoldbehavior untilnewbuilddeployed. Readlivejobmetadata via/v2 endpoints; don't use obsolete/session paths (404). Do not expose encrypted reasoningmetadata when debuggingagentstreaming.
