$ErrorActionPreference = 'Stop'
$bin = (Resolve-Path 'neoism-frontend/gpuix/dist').Path
$version = (& "$bin/neoism.exe" --version).Trim().Split(' ')[1]
if ($LASTEXITCODE -ne 0) { throw 'Cannot read application version' }
$installerVersion = ($version -split '[-+]', 2)[0]
$fixture = Join-Path $env:RUNNER_TEMP 'neoism-msi-fixture'
New-Item -ItemType Directory -Force $fixture | Out-Null
foreach ($name in @('neoism.exe','neoism-workspace-daemon.exe','neoism-agent.exe','neoism-agent-lua-runner.exe')) {
    Copy-Item (Join-Path $bin $name) $fixture
}
Copy-Item (Join-Path $bin 'web') (Join-Path $fixture 'web') -Recurse
[IO.File]::AppendAllText((Join-Path $fixture 'neoism.exe'), 'neoism-upgrade-fixture')
wix build misc/windows/neoism.wxs -arch x64 -ext WixToolset.UI.wixext -d 'Version=0.0.1' -d "BinaryDir=$fixture" -o release/Neoism-upgrade-fixture.msi
if ($LASTEXITCODE -ne 0) { throw 'Fixture MSI build failed' }
wix build misc/windows/neoism.wxs -arch x64 -ext WixToolset.UI.wixext -d "Version=$installerVersion" -d "BinaryDir=$bin" -o release/Neoism-x86_64.msi
if ($LASTEXITCODE -ne 0) { throw 'Release MSI build failed' }
& "$PSScriptRoot/windows-updater-tests.ps1"
& "$PSScriptRoot/windows-msi-validation.ps1"
$hash = (Get-FileHash release/Neoism-x86_64.msi -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText((Join-Path $PWD 'release/Neoism-x86_64.msi.sha256'), "$hash  Neoism-x86_64.msi`n", (New-Object Text.UTF8Encoding $false))
