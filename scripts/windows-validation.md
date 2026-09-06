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

Automated startup only proves a visible window answering messages; the headless
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
