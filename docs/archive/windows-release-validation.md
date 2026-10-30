# Archived pre-GPUIX Windows release notes

These notes describe the retired winit/MSI release pipelines. Their workflow,
script, and artifact names are historical evidence, not instructions for this
branch. Current Windows checks are documented in
[Windows validation](../../scripts/windows-validation.md).

---

# Windows release validation

The normal `release-neoism.yml` tag workflow blocks publication on native
release-profile terminal tests and installed-production MSI startup checks.
No separate dispatch, secret, or environment approval is required.

Download `neoism-windows-x86_64` (candidate MSI + SHA256) and
`windows-validation-<run-id>-<attempt>` from the Actions run, including failures.
A candidate from a failed run is **not validated for release**. Evidence includes
shell paths/versions, test output, installed/packaging hashes and signing status,
MSI logs, GUI stdout/stderr/config logs, window-response samples, application
crash events, and a desktop screenshot (or a screenshot failure explanation).
Signing remains conditional on the workflow's existing signing credentials.

## Manual native GUI acceptance still outstanding

Automated startup checks a responsive visible window, Agent auto-start health,
and a census of additional Neoism/console-helper windows at each sample. Very
short-lived windows between samples can escape detection. The headless
composer test does not exercise the real keyboard dispatcher or GPU rendering.
Before declaring native GUI acceptance, an operator must install the candidate
MSI on native Windows, record its hash, and use the **actual keyboard** in a
PowerShell terminal to run `ls`, `Write-Output 'acceptance-output'`,
`Write-Error 'acceptance-error'`, `Start-Sleep 3`, and
`Read-Host 'acceptance-input'` followed by a typed answer. Check visible output,
error status, running/completed state, input handling, and responsiveness; retain
screenshots/results. This is an operator checklist, not a new Actions approval
mechanism. No blind SendKeys or process-alive result substitutes for acceptance.
A startup stack overflow must fail and be diagnosed, never patched with editbin
or a changed production stack reserve.

## Runner-loss investigation (stable .126/.127, 2026-10-16)

**Root cause remains unproven.** Run [38961220065, attempt 1, Windows job 117176500597](https://github.com/parkers0405/neoism/actions/runs/38961220065/attempts/1) entered MSI smoke at 22:15:46 UTC and was marked failed at 23:02:13 (46m27s later). The check annotation says the hosted runner lost communication; its smoke step still has no completion timestamp and evidence upload remained pending. The job-log API returned HTTP 404 / `BlobNotFound`. The reported last streamed product-version line is not a durable trace of the operation that followed it. A 12-minute step timeout and `always()` upload require a functioning runner; neither guarantees recovery/evidence after a runner disconnect. No evidence here distinguishes VM loss, runner termination, resource exhaustion, network loss, or a script hang followed by an independent runner failure.

Comparison evidence:

- [Stable .126 retry 2, job 116684346730](https://github.com/parkers0405/neoism/actions/runs/38788489057/attempts/2): smoke 22:52:47–22:54:25 UTC (98s), version printed 22:53:02.780; retained managed worker log 22:53:06.440–22:53:27.587, portable 22:53:31.943–22:53:50.722. The remaining GUI/cleanup/uninstall completed before transcript close at 22:54:25.415. Thus a silent interval after the version line also occurs on success.
- [Nightly .126-nightly.20261015.2, job 116554307664](https://github.com/parkers0405/neoism/actions/runs/38773254527): smoke 18:06:04–18:07:33 UTC (89s). Both use the same 0.0.1 fixture recipe: candidate executables/web, with a marker appended to fixture `neoism.exe`, followed by candidate installation. Stable/nightly differ in executable full version; MSI uses the numeric version without the prerelease suffix. Stable retry passing contradicts a deterministic stable-version-only failure, but does not rule out an intermittent product problem.
- Both successful jobs used GHA sccache and npm cache hits; final Rust cache-hit counts were 185/214 (stable retry) versus 87/214 (nightly). Rust cache excludes `target/`. These are observations, not evidence of a cache-induced smoke failure. Failed-run final cache statistics are unavailable.

Code-path audit: the version check is followed by registry checks, formerly another `cargo metadata`, shortcut/PATH checks, native managed/portable worker fixtures, installed GUI smoke, then uninstall in `finally`. Native fixtures run the production PowerShell worker directly with owner PIDs 0 and relaunch 0; they do **not** invoke the CLI updater bootstrap. Production `Stop-TargetStack` only kills four exact Neoism executable names whose immediate directory equals the selected target; it has no tree/global image kill. The CLI helper uses a separate process group, hidden console, and null stdio, not breakaway/job-object termination. The audited fast-hosting job object assigns only its newly spawned child, not the runner/parent. No `$pid` assignment collision or demonstrated runner/ancestor-kill path was found in the audited scripts. This audit does not prove every possible GUI startup path safe.

### Hardening for the next build, not a claim to have fixed runner death

- Supply mandatory `-ExpectedVersion` from the already verified `NEOISM_RELEASE_VERSION`; remove the redundant, unbounded post-install Cargo invocation. Keep exact binary/full-version and numeric MSI registry checks.
- Replace cleanup's unbounded `Start-Process -Wait` with a 10s taskkill wait, at most 5s to terminate a hung taskkill by handle, and a 5s target-exit check. Retain the target's process handle against PID reuse; reject self/nonpositive IDs. Cleanup still fails if the target survives, and taskkill's status does not contaminate `LASTEXITCODE`.
- Bound redirected-output draining to 5s instead of waiting indefinitely for descendant-inherited pipes. The blocking .NET EOF wait runs on a background task; the validation thread only waits for that deadline. An output-drain timeout is failure, not permission to accept partial version output. Retain stdout/stderr and timeout receipts, even if cleanup itself fails.
- Timestamp MSI, hash/signature, registry/version/PATH, native worker mode, GUI, cleanup, and subprocess start/exit phases. Before smoke, capture process IDs/parents/start times/paths, memory/disk headroom, image/PowerShell versions, recent reboot/resource-exhaustion events, and MSI hashes; upload an immutable `windows-pre-msi-<run>-<attempt>` checkpoint before installation. Capture a post-MSI snapshot when execution reaches `finally`. Diagnostic collection errors are recorded explicitly, not treated as validation success.
- Explicitly suppress MSI reboot and require exit 0 in this disposable CI smoke; reboot-required exits 3010/1641 are not a validated installation. Production reboot policy is unchanged.

Run `pwsh -NoProfile -File scripts/windows-validation-process-tests.ps1` (also wired under Windows PowerShell 5.1 in release CI). Coverage includes unsafe PID rejection, already-exited target, taskkill nonzero after target exit, surviving target, hung taskkill, bounded waits, exit/timeout propagation, retained stderr, output-drain failure, and real subprocess stdout/stderr plus a real 5s drain deadline. Linux PowerShell testing does not validate Windows taskkill semantics, MSI, registry/CIM/event APIs, native GUI, or actual runner-loss recovery. Filesystem/native OS calls and Windows Installer service transactions are not all made cancellable by these process deadlines.

An existing tag retry checks out the existing tag commit and is unaffected by these working-tree changes. No validation has been disabled, no tag rewritten, and no new release is required merely to retry the old tag.

## Recurrence: v0.7.131, 2026-10-19

**Runner-loss root cause is still unknown.** Run `39205247277`, attempt 1, job `117906582301` completed native terminal validation, MSI creation, and checkpoint upload (artifact `12169877891`). Smoke began at 07:32:29 UTC; the job failed at 08:19:29 with the check annotation “The hosted runner lost communication with the server.” Smoke has no completion timestamp and the subsequent evidence upload remained pending. The 47-minute interval is not a measured duration of any individual MSI operation. The recovered 31-file checkpoint contains diagnostics and two MSI hashes, **not either MSI**, and cannot reconstruct their bytes.

The actual CIM snapshot at 07:32:26 reports 13,166,532 KiB free physical memory out of 16,771,836 KiB, 18,211,172 KiB free virtual memory, 29,989,318,656 bytes free on C:, and 147,087,183,872 bytes on D:. The checkpoint shell (PID 6828, working set 100,028,416 bytes, 563 handles) is a child of Runner.Worker (1844), itself a child of Runner.Listener (5472). No Neoism process appears in that pre-install census. No events matched the selected System IDs (41/1074/6005/6006/6008/2004) during the preceding hour; this is not proof that no other events occurred. There is no retained post-start MSI phase, screenshot, application event log, or HKEY/registry-state capture for the failed smoke. The single pre-smoke sample has no CPU-rate or file-growth time series and cannot exclude later resource exhaustion, an ancestor termination, a network failure, or VM loss.

Successful `.130` run `39176772908`, Windows job `117820973468`, used identical validation scripts and the same image `20261007.264.1` / PowerShell `7.6.6`. Its checkpoint had 13,041,588 KiB free physical memory, 26,852,831,232 bytes free on C:, and 226,775,535,616 bytes on D:. Its retained phases show fixture install 01:03:43–49, candidate install 01:03:50–59, registry checks 01:04:02, managed updater 01:04:05–36, portable updater 01:04:38–57, GUI 01:05:00–30, then uninstall and PASS at 01:05:32 (109 seconds total). Comparable pre-smoke headroom and success with identical scripts do not identify the failed operation in `.131`. Existing bounded wait/output/taskkill/phase changes were already present in `.131`; they are not a new solution to this recurrence. The cleanup helper uses bounded kernel waits, not a busy polling loop; the updater's quiescence loop has a 30-second deadline and a 250ms sleep. There is no demonstrated runner/ancestor-kill path in these scripts. This does not prove all product startup paths safe, and no speculative production change is made here.

### Package-bearing checkpoint and no-build recovery

For subsequent builds, `windows-pre-msi-<run>-<attempt>` now uploads **both exact MSIs**, both SHA256 sidecars, diagnostics, and `windows-validation/msi-checkpoint.json` before destructive smoke. The manifest records the original full version, source commit/run/attempt, conditional signing requirement, validation-script hashes, and every packaged binary/web asset's SHA256 from the build inputs. Upload must succeed before smoke begins. This is an **unvalidated recovery artifact**, never evidence of release acceptance and never automatically promoted to publication. An `always()` upload after smoke remains best effort, not runner-loss recovery.

The standalone **Validate existing Windows MSI** workflow can be dispatched on a ref containing the corrected scripts, with `source_run_id`, `source_run_attempt`, and the exact original `version`. It does not compile, change versions/tags, publish, or bypass release gating. It verifies provenance and both MSI checksums, administratively extracts the candidate with a bounded process and exit 0 requirement, verifies the complete extracted payload against the original build-input manifest, then invokes the same `windows-msi-validation.ps1` used by release CI with that independent packaging baseline. All existing fixture upgrade, binary hashes/signatures, exact version, real registry/PATH/shortcut checks, managed/portable updater transactions, visible responsive GUI/Agent checks, uninstall, and user-data preservation assertions remain in place. Replay records both source and validation commits; signing checks remain required when the source build required them. Failure remains failure; there is no fallback to a different version or old executable. The administrative extraction baseline and replay orchestration still require native Windows validation; Linux tests cannot prove those OS operations.

Old diagnostics-only checkpoints, including `.131` attempt 1, deliberately fail closed: hashes are not packages. If an exact candidate/fixture pair was never uploaded, this workflow cannot recover it. A parent-coordinated rebuild of the unchanged source identity is necessary unless exact original package bytes become available elsewhere; a warm compilation cache can help but does not guarantee a short build. No cross-build, workflow dispatch, retry/cancel, commit/push, or tag operation is part of this patch.

Run `pwsh -NoProfile -File scripts/windows-validation-process-tests.ps1` for the existing process tests plus new checkpoint regressions (capture, full web/binary manifest, signing/source identity, tampered candidate/fixture/sidecars, wrong version/run/attempt, and diagnostics-only rejection). Workflow syntax can be checked with `actionlint .github/workflows/release-neoism.yml .github/workflows/validate-windows-msi.yml`. Native Windows MSI replay and actual future runner-loss package recovery are not verified by these local checks.
