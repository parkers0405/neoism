$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/windows-validation-process.ps1"
$evidence = (New-Item -ItemType Directory -Force 'windows-validation').FullName
$bin = (Resolve-Path 'neoism-frontend/gpuix/dist').Path
# MSI consumes dist directly; do not rely on a prior ZIP packaging invocation.
python "$PSScriptRoot/windows-bundle.py" $bin (Join-Path $PSScriptRoot '../target/release')
if ($LASTEXITCODE -ne 0) { throw 'Windows GUI subsystem or release service provenance validation failed' }
node "$PSScriptRoot/build-docs.mjs" --verify (Join-Path $bin 'resources/docs')
if ($LASTEXITCODE -ne 0) { throw 'Bundled docs verification failed' }
# GUI-subsystem executables are not reliable PowerShell success-pipeline
# producers. Explicitly wait and redirect, as installed-binary validation does.
Invoke-CheckedProcess (Join-Path $bin 'neoism.exe') '--version' "$evidence/packaging-version" 30
$versionOutput = [IO.File]::ReadAllText("$evidence/packaging-version.stdout.log").Trim()
if ($versionOutput -notmatch '^neoism (\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.+-]+)?)$') {
    throw "Cannot read application version: $versionOutput"
}
$version = $Matches[1]
$installerVersion = ($version -split '[-+]', 2)[0]
$fixture = Join-Path $env:RUNNER_TEMP 'neoism-msi-fixture'
New-Item -ItemType Directory -Force $fixture | Out-Null
foreach ($name in @('neoism.exe','neoism-workspace-daemon.exe','neoism-agent.exe','neoism-agent-lua-runner.exe')) {
    Copy-Item (Join-Path $bin $name) $fixture
}
Copy-Item (Join-Path $bin 'web') (Join-Path $fixture 'web') -Recurse
Copy-Item (Join-Path $bin 'resources') (Join-Path $fixture 'resources') -Recurse
[IO.File]::AppendAllText((Join-Path $fixture 'neoism.exe'), 'neoism-upgrade-fixture')
wix build misc/windows/neoism.wxs -arch x64 -ext WixToolset.UI.wixext -d 'Version=0.0.1' -d "BinaryDir=$fixture" -o release/Neoism-upgrade-fixture.msi
if ($LASTEXITCODE -ne 0) { throw 'Fixture MSI build failed' }
wix build misc/windows/neoism.wxs -arch x64 -ext WixToolset.UI.wixext -d "Version=$installerVersion" -d "BinaryDir=$bin" -o release/Neoism-x86_64.msi
if ($LASTEXITCODE -ne 0) { throw 'Release MSI build failed' }
& "$PSScriptRoot/windows-updater-tests.ps1"
& "$PSScriptRoot/windows-msi-validation.ps1"
$hash = (Get-FileHash release/Neoism-x86_64.msi -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText((Join-Path $PWD 'release/Neoism-x86_64.msi.sha256'), "$hash  Neoism-x86_64.msi`n", (New-Object Text.UTF8Encoding $false))
