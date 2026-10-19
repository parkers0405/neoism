# Files/checksums/provenance only; never invokes MSI or launches fixture executables.
$ErrorActionPreference = 'Stop'
$checkpointScript = Join-Path $PSScriptRoot 'windows-msi-checkpoint.ps1'
$root = Join-Path ([IO.Path]::GetTempPath()) ('neoism-checkpoint-test-' + [Guid]::NewGuid().ToString('N'))
$oldRun = $env:GITHUB_RUN_ID; $oldAttempt = $env:GITHUB_RUN_ATTEMPT; $oldSha = $env:GITHUB_SHA
$oldSigning = $env:HAVE_WINDOWS_SIGNING
function Assert-Checkpoint($Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Assert-Rejected([string]$Message, [string]$Version = '0.7.131', [string]$Run = '123', [string]$Attempt = '1') {
    $failed = $false
    try { & $checkpointScript -Mode Validate -Directory $root -ExpectedVersion $Version -SourceRun $Run -SourceAttempt $Attempt }
    catch {
        if ($_ -notmatch $Message) { throw }
        $failed = $true
    }
    Assert-Checkpoint $failed "Expected rejection: $Message"
}
New-Item -ItemType Directory -Path $root | Out-Null
Push-Location $root
try {
    $env:GITHUB_RUN_ID = '123'; $env:GITHUB_RUN_ATTEMPT = '1'; $env:GITHUB_SHA = 'a' * 40
    $env:HAVE_WINDOWS_SIGNING = 'true'
    $payload = New-Item -ItemType Directory -Force 'target/x86_64-pc-windows-msvc/release'
    New-Item -ItemType Directory -Force (Join-Path $payload.FullName 'web/agent-gui'), 'windows-validation' | Out-Null
    foreach ($name in @('neoism.exe', 'neoism-workspace-daemon.exe', 'neoism-agent.exe', 'neoism-agent-lua-runner.exe',
            'web/index.html', 'web/agent-gui/index.html', 'web/extra.js')) {
        Set-Content -LiteralPath (Join-Path $payload.FullName $name) -Value "fixture $name"
    }
    Set-Content Neoism-x86_64.msi 'candidate fixture bytes, not executable'
    Set-Content Neoism-upgrade-fixture.msi 'upgrade fixture bytes, not executable'
    & $checkpointScript -Mode Capture -ExpectedVersion '0.7.131'
    $manifestFile = Join-Path $root 'windows-validation/msi-checkpoint.json'
    $original = Get-Content -LiteralPath $manifestFile -Raw
    $manifest = $original | ConvertFrom-Json
    Assert-Checkpoint ($manifest.version -ceq '0.7.131') 'Dot-sourced helper must preserve exact version'
    Assert-Checkpoint ($manifest.status -ceq 'unvalidated') 'Checkpoint must never claim validation'
    Assert-Checkpoint ($manifest.signing_required -eq $true) 'Signing requirement must survive capture'
    Assert-Checkpoint ($manifest.source_run -eq '123' -and $manifest.source_attempt -eq '1') 'Source identity missing'
    Assert-Checkpoint (@($manifest.payload.PSObject.Properties).Count -eq 7) 'Manifest must include every web asset and binary'
    Assert-Checkpoint (@($manifest.scripts.PSObject.Properties).Count -eq 6) 'Script provenance missing'
    foreach ($name in @('Neoism-x86_64.msi', 'Neoism-upgrade-fixture.msi')) {
        $hash = (Get-FileHash -LiteralPath $name).Hash
        Assert-Checkpoint ($hash -eq $manifest.packages.$name) "Wrong package hash: $name"
        Assert-Checkpoint ((Get-Content -LiteralPath "$name.sha256" -Raw) -ceq "$($hash.ToLowerInvariant())  $name") "Wrong checksum sidecar: $name"
    }
    Assert-Rejected 'provenance' '0.7.130'
    Assert-Rejected 'provenance' '0.7.131' '124'
    Assert-Rejected 'provenance' '0.7.131' '123' '2'
    foreach ($name in @('Neoism-x86_64.msi', 'Neoism-upgrade-fixture.msi')) {
        $bytes = [IO.File]::ReadAllBytes((Join-Path $root $name))
        Add-Content -LiteralPath $name -Value 'tampered'
        Assert-Rejected 'checksum mismatch'
        [IO.File]::WriteAllBytes((Join-Path $root $name), $bytes)
        $sum = Get-Content -LiteralPath "$name.sha256" -Raw
        Set-Content -LiteralPath "$name.sha256" -Value ('0' * 64 + "  $name")
        Assert-Rejected 'checksum mismatch'
        Set-Content -LiteralPath "$name.sha256" -Value $sum -NoNewline
    }
    $manifest.version = '0.7.999'
    $manifest | ConvertTo-Json -Depth 5 | Set-Content $manifestFile
    Assert-Rejected 'provenance'
    Set-Content -LiteralPath $manifestFile -Value $original
    Remove-Item -LiteralPath $manifestFile
    Assert-Rejected 'does not exist|Cannot find path'
    'PASS: checkpoint capture, complete payload hashes, signing/provenance, tampered candidate/fixture/sidecars, wrong version/run/attempt, diagnostics-only rejection'
} finally {
    Pop-Location
    $env:GITHUB_RUN_ID = $oldRun; $env:GITHUB_RUN_ATTEMPT = $oldAttempt; $env:GITHUB_SHA = $oldSha
    $env:HAVE_WINDOWS_SIGNING = $oldSigning
    Remove-Item -LiteralPath $root -Recurse -Force
}
