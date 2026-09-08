# Native, destructive CI fixture: call only from the MSI smoke's owned installation.
# Exercises the production worker, NOT an old installed `neoism update` command.
param(
    [Parameter(Mandatory)][string]$CandidateMsi,
    [Parameter(Mandatory)][string]$ManagedDir,
    [Parameter(Mandatory)][string]$PackagingDir,
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$Evidence
)
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/windows-validation-process.ps1"
$helper = (Resolve-Path "$PSScriptRoot/../neoism-frontend/desktop/src/windows_update.ps1").Path
. $helper -LibraryOnly
$CandidateMsi = (Resolve-Path $CandidateMsi).Path
$ManagedDir = (Resolve-Path $ManagedDir).Path
$PackagingDir = (Resolve-Path $PackagingDir).Path
$evidenceRoot = (New-Item -ItemType Directory -Force (Join-Path $Evidence 'updater-worker')).FullName
$fixtureRoot = Join-Path ([IO.Path]::GetTempPath()) ('neoism-updater-native-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixtureRoot | Out-Null
$originalPath = $env:PATH
$markerName = '.updater-preserve-' + [Guid]::NewGuid().ToString('N')
$markerText = 'unrelated fixture data must survive the updater'
$markers = @()
function Assert-Native($Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Assert-NoFixtureProcesses([string]$Directory) {
    foreach ($process in Get-Process -Name 'neoism', 'neoism-workspace-daemon', 'neoism-agent' -ErrorAction SilentlyContinue) {
        $path = $process.Path
        if ($path -and (Test-SamePath ([IO.Path]::GetDirectoryName($path)) $Directory)) {
            throw "Fixture target already has a live process ($($process.Id)); refusing to stop it"
        }
    }
}
function Invoke-NativeWorker([string]$Mode, [string]$Directory) {
    Assert-NoFixtureProcesses $Directory
    $caseEvidence = (New-Item -ItemType Directory -Path (Join-Path $evidenceRoot $Mode) -Force).FullName
    $download = (New-Item -ItemType Directory -Path (Join-Path $fixtureRoot "$Mode-download")).FullName
    $msi = Join-Path $download 'candidate.msi'
    Copy-Item -LiteralPath $CandidateMsi -Destination $msi
    $result = Join-Path $caseEvidence 'result.json'
    $exe = Join-Path $Directory 'neoism.exe'
    # These owners are absent: the fixture never launches its tampered exe/GUI.
    # PID 0 is the production helper's existing optional-owner contract.
    $arguments = "-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$helper`" -UpdaterPid 0 -GuiPid 0 -MsiPath `"$msi`" -TempDir `"$download`" -InvokingExe `"$exe`" -ExpectedVersion `"$Version`" -ResultPath `"$result`" -Relaunch 0"
    Invoke-CheckedProcess (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe') `
        $arguments (Join-Path $caseEvidence 'worker') 180 @(0) | Out-Host
    $receipt = Get-Content -LiteralPath $result -Raw | ConvertFrom-Json
    Assert-Native ($receipt.protocol -eq 1 -and $receipt.state -eq 'succeeded' -and $receipt.installation_verified) 'Worker did not durably verify installation'
    Assert-Native ($receipt.expected_version -eq $Version -and $receipt.target.mode -eq $Mode) 'Wrong worker release/installation mode'
    Assert-Native (Test-SamePath $receipt.target.directory $Directory) 'Worker selected a different installation directory'
    Assert-Native (Test-SamePath $receipt.target.executable $exe) 'Worker selected a different invoking executable'
    Assert-Native (Test-Path -LiteralPath $receipt.log) 'Missing durable helper log'
    Assert-Native (Test-Path -LiteralPath $receipt.extract_msi_log) 'Real MSI extraction log missing'
    if ($Mode -eq 'managed') {
        Assert-Native ($receipt.msi_exit_code -eq 0 -and (Test-Path -LiteralPath $receipt.msi_log)) 'Real managed MSI install did not finish without reboot'
    } else {
        Assert-Native ($null -eq $receipt.msi_exit_code -and -not (Test-Path -LiteralPath $receipt.msi_log)) 'Portable worker must extract, not install another MSI'
    }
    Assert-InstalledPayload $Directory $script:CandidateManifest $Version
    Get-PayloadManifest $Directory | ConvertTo-Json | Set-Content (Join-Path $caseEvidence 'installed-payload-hashes.json')
    Assert-Native (([IO.File]::ReadAllText((Join-Path $Directory $markerName))) -eq $markerText) 'Updater removed unrelated fixture data'
    Assert-NoFixtureProcesses $Directory
    return $receipt
}
try {
    Assert-Native (Test-SamePath (Get-RegisteredInstallDir) $ManagedDir) 'MSI smoke must own the registered target before this fixture runs'
    Assert-Native (Test-MsiProductInstalled $CandidateMsi) 'Candidate ProductCode must already be installed for the same-package managed repair test'
    Assert-NoFixtureProcesses $ManagedDir
    Assert-StackVersion $PackagingDir $Version
    $script:CandidateManifest = Get-PayloadManifest $PackagingDir
    $script:CandidateManifest | ConvertTo-Json | Set-Content (Join-Path $evidenceRoot 'expected-payload-hashes.json')
    foreach ($mode in @('managed', 'portable')) {
        $directory = $ManagedDir
        if ($mode -eq 'portable') {
            $directory = (New-Item -ItemType Directory -Path (Join-Path $fixtureRoot 'PATH-shadow portable')).FullName
            foreach ($name in @($script:BinaryNames) + @('web')) {
                Copy-Item -LiteralPath (Join-Path $PackagingDir $name) -Destination (Join-Path $directory $name) -Recurse
            }
            $env:PATH = "$directory;$originalPath"
            Assert-Native (Test-SamePath (Get-Command neoism.exe -CommandType Application).Source (Join-Path $directory 'neoism.exe')) 'Portable fixture must actually shadow the managed CLI on PATH'
        }
        Assert-NoFixtureProcesses $directory
        # Managed: same installed ProductCode/version must really repair files.
        # Portable: installing only into the managed path must never pass.
        foreach ($name in @($script:BinaryNames) + @('web\index.html')) {
            [IO.File]::AppendAllText((Join-Path $directory $name), 'native-updater-old-copy-fixture')
        }
        $before = Get-PayloadManifest $directory
        $before | ConvertTo-Json | Set-Content (Join-Path $evidenceRoot "$mode-original-hashes.json")
        $marker = Join-Path $directory $markerName
        [IO.File]::WriteAllText($marker, $markerText)
        $markers += $marker
        $receipt = Invoke-NativeWorker $mode $directory
        foreach ($name in @($script:BinaryNames) + @('web\index.html')) {
            $hash = (Get-FileHash -LiteralPath (Join-Path $directory $name) -Algorithm SHA256).Hash
            Assert-Native ($hash -ne $before[$name] -and $hash -eq $script:CandidateManifest[$name]) "$mode $name was not restored in place"
        }
        if ($mode -eq 'portable') {
            foreach ($relative in $before.Keys) {
                $backupHash = (Get-FileHash -LiteralPath (Join-Path $receipt.rollback_directory $relative) -Algorithm SHA256).Hash
                Assert-Native ($backupHash -eq $before[$relative]) "Original portable rollback bytes lost: $relative"
            }
            Assert-Native (Test-SamePath (Get-RegisteredInstallDir) $ManagedDir) 'Portable worker changed managed registration'
            Assert-InstalledPayload $ManagedDir $script:CandidateManifest $Version
        }
    }
    'native-managed-and-portable-worker-passed; relaunch=0; CLI bootstrap not exercised' |
        Set-Content (Join-Path $evidenceRoot 'passed.txt')
} finally {
    $env:PATH = $originalPath
    foreach ($marker in $markers) { Remove-Item -LiteralPath $marker -Force -ErrorAction SilentlyContinue }
    # Logs/receipts/hashes are retained in evidence; only this GUID fixture is removed.
    Remove-Item -LiteralPath $fixtureRoot -Recurse -Force -ErrorAction SilentlyContinue
}
