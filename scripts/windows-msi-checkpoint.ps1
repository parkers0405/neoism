# Capture exact, unvalidated packages before smoke; replay the SAME full smoke without a build.
param(
    [Parameter(Mandatory)][ValidateSet('Capture', 'Validate')][string]$Mode,
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$')][string]$ExpectedVersion,
    [string]$Directory = '.',
    [ValidatePattern('^\d+$')][string]$SourceRun,
    [ValidatePattern('^\d+$')][string]$SourceAttempt
)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/windows-validation-process.ps1"
. "$PSScriptRoot/../neoism-frontend/desktop/src/windows_update.ps1" -LibraryOnly -ExpectedVersion $ExpectedVersion
$Directory = (Resolve-Path -LiteralPath $Directory).Path
$manifestPath = Join-Path $Directory 'windows-validation/msi-checkpoint.json'
$packages = @('Neoism-x86_64.msi', 'Neoism-upgrade-fixture.msi')
if ($Mode -eq 'Capture') {
    $payload = (Resolve-Path 'target/x86_64-pc-windows-msvc/release').Path
    $hashes = @{}
    foreach ($name in $packages) {
        $hash = (Get-FileHash -LiteralPath (Join-Path $Directory $name) -Algorithm SHA256).Hash
        $hashes[$name] = $hash
        "$($hash.ToLowerInvariant())  $name" | Set-Content -NoNewline -LiteralPath (Join-Path $Directory "$name.sha256")
    }
    $scripts = @{}
    foreach ($name in @('windows-msi-checkpoint.ps1', 'windows-msi-validation.ps1', 'windows-validation-process.ps1',
            'windows-updater-native-validation.ps1', 'windows-installed-gui-smoke.ps1')) {
        $scripts[$name] = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $name)).Hash
    }
    $scripts['windows_update.ps1'] = (Get-FileHash "$PSScriptRoot/../neoism-frontend/desktop/src/windows_update.ps1").Hash
    [ordered]@{ schema = 1; status = 'unvalidated'; version = $ExpectedVersion;
        source_run = $env:GITHUB_RUN_ID; source_attempt = $env:GITHUB_RUN_ATTEMPT; source_commit = $env:GITHUB_SHA;
        signing_required = ($env:HAVE_WINDOWS_SIGNING -eq 'true'); packages = $hashes;
        payload = (Get-PayloadManifest $payload); scripts = $scripts } |
        ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifestPath
    return
}
# Do not accept the old diagnostics-only checkpoint or a different release/attempt.
$checkpoint = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($checkpoint.schema -ne 1 -or $checkpoint.status -cne 'unvalidated' -or
    $checkpoint.version -cne $ExpectedVersion -or -not $SourceRun -or -not $SourceAttempt -or
    $checkpoint.source_run -ne $SourceRun -or $checkpoint.source_attempt -ne $SourceAttempt -or
    $checkpoint.source_commit -notmatch '^[a-f0-9]{40}$' -or $checkpoint.signing_required -isnot [bool]) {
    throw 'Checkpoint provenance does not match the requested release/run/attempt'
}
foreach ($name in $packages) {
    $file = Join-Path $Directory $name
    $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash
    $sum = (Get-Content -LiteralPath "$file.sha256" -Raw).Trim()
    if ($sum -notmatch ('^([a-fA-F0-9]{64})\s+\*?' + [regex]::Escape($name) + '$') -or
        $hash -ne $Matches[1] -or $hash -ne $checkpoint.packages.$name) {
        throw "Checkpoint package checksum mismatch: $name"
    }
}
$evidence = (New-Item -ItemType Directory -Force 'windows-validation').FullName
$checkpoint | ConvertTo-Json -Depth 5 | Set-Content "$evidence/source-checkpoint.json"
[ordered]@{ validation_commit = $env:GITHUB_SHA; source_run = $SourceRun; source_attempt = $SourceAttempt;
    source_commit = $checkpoint.source_commit; version = $ExpectedVersion } |
    ConvertTo-Json | Set-Content "$evidence/replay-provenance.json"
Get-FileHash "$PSScriptRoot/*.ps1", "$PSScriptRoot/../neoism-frontend/desktop/src/windows_update.ps1" |
    Select-Object Path, Hash | ConvertTo-Json | Set-Content "$evidence/replay-script-hashes.json"
# Administrative extraction supplies an independent baseline, not an installed-copy oracle.
# Compare every packaged binary/web file to the manifest captured from original build inputs.
$extract = Join-Path $env:RUNNER_TEMP ('neoism-checkpoint-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $extract | Out-Null
$oldSigning = $env:HAVE_WINDOWS_SIGNING
try {
    Write-ValidationSnapshot -Evidence $evidence -Name pre-checkpoint-replay
    Invoke-CheckedProcess 'msiexec.exe' "/a `"$(Join-Path $Directory $packages[0])`" /qn /norestart REBOOT=ReallySuppress TARGETDIR=`"$extract`" /L*v `"$evidence/extract.log`"" "$evidence/extract" 180 @(0)
    $roots = @(Get-ChildItem -LiteralPath $extract -Filter neoism.exe -File -Recurse)
    if ($roots.Count -ne 1) { throw 'Checkpoint MSI must contain exactly one executable payload root' }
    $payload = $roots[0].DirectoryName
    $actual = Get-PayloadManifest $payload
    $expected = @($checkpoint.payload.PSObject.Properties)
    if ($expected.Count -ne $actual.Count) { throw 'Checkpoint payload file count mismatch' }
    foreach ($entry in $expected) {
        if ($entry.Value -notmatch '^[a-fA-F0-9]{64}$' -or $actual[$entry.Name] -ne $entry.Value) {
            throw "Checkpoint extracted payload mismatch: $($entry.Name)"
        }
    }
    $env:HAVE_WINDOWS_SIGNING = "$($checkpoint.signing_required)".ToLowerInvariant()
    & "$PSScriptRoot/windows-msi-validation.ps1" -ExpectedVersion $ExpectedVersion -PackagingDir $payload -PackageDir $Directory
} finally {
    $env:HAVE_WINDOWS_SIGNING = $oldSigning
    Remove-Item -LiteralPath $extract -Recurse -Force
}
