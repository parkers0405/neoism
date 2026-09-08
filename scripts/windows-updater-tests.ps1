# Isolated updater policy/transaction tests. Never installs MSI, launches product
# binaries, terminates processes, or writes registry/user installation data.
param([string]$Helper = (Join-Path $PSScriptRoot '../neoism-frontend/desktop/src/windows_update.ps1'))
$ErrorActionPreference = 'Stop'
$tokens = $null; $parseErrors = $null
$parsePaths = @($Helper, (Join-Path $PSScriptRoot 'windows-updater-native-validation.ps1'), (Join-Path $PSScriptRoot 'windows-msi-validation.ps1'))
foreach ($parsePath in $parsePaths) {
    $null = [Management.Automation.Language.Parser]::ParseFile($parsePath, [ref]$tokens, [ref]$parseErrors)
    if ($parseErrors.Count) { throw ($parseErrors | Format-List | Out-String) }
}
. $Helper -LibraryOnly
function Assert($Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Assert-Throws([scriptblock]$Action, [string]$Match) {
    try { & $Action } catch {
        if ($_.Exception.Message -notlike "*$Match*") { throw }
        return
    }
    throw "Expected failure containing: $Match"
}

$managed = 'C:\Users\Tester\AppData\Local\Programs\Neoism'
$t = Resolve-UpdateTarget "$managed\neoism.exe" ($managed.ToUpperInvariant() + '\')
Assert ($t.mode -eq 'managed') 'Registered invoking path must be managed, case insensitive'
$t = Resolve-UpdateTarget ('\\?\' + $managed + '\neoism.exe') $managed
Assert ($t.mode -eq 'managed') 'Rust verbatim executable path must match normal HKCU path'
$t = Resolve-UpdateTarget 'C:\portable\neoism.exe' $null
Assert ($t.mode -eq 'portable' -and $t.directory -eq 'C:\portable') 'No registration must update portable in place'
$t = Resolve-UpdateTarget 'C:\old-path-first\neoism.exe' $managed
Assert ($t.mode -eq 'portable' -and $t.executable -eq 'C:\old-path-first\neoism.exe') 'PATH-shadow copy must not be redirected to MSI'
$t = Resolve-UpdateTarget "$managed-other\neoism.exe" $managed
Assert ($t.mode -eq 'portable') 'Path prefix must not count as managed'
Assert-Throws { Resolve-UpdateTarget 'C:\loose\renamed.exe' $null } 'Rename'
foreach ($name in $script:BinaryNames) {
    $label = [IO.Path]::GetFileNameWithoutExtension($name)
    Assert-VersionOutput $name "$label 1.2.3" '1.2.3'
    Assert-Throws { Assert-VersionOutput $name "$label 1.2.30" '1.2.3' } 'mismatch'
    Assert-Throws { Assert-VersionOutput $name "$label 1.2.3-beta" '1.2.3' } 'mismatch'
    Assert-Throws { Assert-VersionOutput $name '1.2.3' '1.2.3' } 'mismatch'
}
Assert ((Get-MsiOutcome 0) -eq 'succeeded') 'MSI 0 classification'
foreach ($code in @(3010, 1641)) { Assert ((Get-MsiOutcome $code) -eq 'reboot_required') 'Reboot must not be success' }
foreach ($code in @(1603, 1618, 1)) { Assert ((Get-MsiOutcome $code) -eq 'failed') 'MSI failure classification' }

# Exercise the actual native binding read-only, rather than mocking a COM object
# with properties that its real PowerShell projection may not expose.
Assert ((Get-CurrentUserMsiProductState '{00000000-0000-0000-0000-000000000000}') -eq -1) 'Unknown product is not installed'

# Candidate metadata and known installed/advertised outcomes are isolated fakes.
& {
    $script:QueryCode = '{12345678-1234-1234-1234-123456789012}'
    $script:QueryRecord = [pscustomobject]@{}
    $script:QueryRecord | Add-Member ScriptMethod StringData { param($index) Assert ($index -eq 1) 'ProductCode record field'; return $script:QueryCode }
    $script:QueryView = [pscustomobject]@{}
    $script:QueryView | Add-Member ScriptMethod Execute {}
    $script:QueryView | Add-Member ScriptMethod Fetch { return $script:QueryRecord }
    $script:QueryView | Add-Member ScriptMethod Close {}
    $script:QueryDatabase = [pscustomobject]@{}
    $script:QueryDatabase | Add-Member ScriptMethod OpenView { param($sql) Assert ($sql -like '*ProductCode*') 'Must query candidate ProductCode'; return $script:QueryView }
    $script:QueryInstaller = [pscustomobject]@{}
    $script:QueryInstaller | Add-Member ScriptMethod OpenDatabase { param($path, $mode) Assert ($mode -eq 0) 'MSI metadata must be read-only'; return $script:QueryDatabase }
    function Get-CurrentUserMsiProductState([string]$code) {
        Assert ($code -eq $script:QueryCode) 'Repair query must match candidate ProductCode'
        return $script:QueryState
    }
    function New-Object { param($ComObject) Assert ($ComObject -eq 'WindowsInstaller.Installer') 'Only read-only Installer Automation expected'; return $script:QueryInstaller }
    $script:QueryState = -1
    Assert (-not (Test-MsiProductInstalled 'C:\candidate.msi')) 'New ProductCode must not enter repair mode'
    $script:QueryState = 1
    Assert (-not (Test-MsiProductInstalled 'C:\candidate.msi')) 'Advertised-only product must use normal installation'
    $script:QueryState = 5
    Assert (Test-MsiProductInstalled 'C:\candidate.msi') 'Already installed candidate ProductCode must enter repair mode'
}

$root = Join-Path ([IO.Path]::GetTempPath()) ('neoism-updater-unit-' + [Guid]::NewGuid().ToString('N'))
$originalLocal = $env:LOCALAPPDATA
New-Item -ItemType Directory -Path $root | Out-Null
# Production orchestration with explicit fakes for ALL external side effects.
function Get-RegisteredInstallDir { return $script:Registration }
function Test-MsiProductInstalled { return $script:SameProduct }
function Get-TrackedProcess { return $null }
function Wait-UpdateOwner {
    $receipt = Get-Content -LiteralPath $ResultPath -Raw | ConvertFrom-Json
    Assert ($receipt.state -eq 'handed_off' -and -not $receipt.installation_verified) 'Handoff must be durable and not success before owner exit'
    $script:SawHandoff = $true
}
function Stop-TargetStack([string]$Directory) {
    Assert ($Directory -eq $script:FixtureInstall) 'Must quiesce only the selected stack'
    if ($script:Locked) { throw 'fixture locked process' }
}
function Get-BinaryVersion([string]$Exe) { return [IO.File]::ReadAllText($Exe) }
function Start-Process([string]$FilePath, [string]$WorkingDirectory) {
    Assert ($FilePath -eq (Join-Path $script:FixtureInstall 'neoism.exe')) 'Must relaunch exact invoking path'
    Assert ($WorkingDirectory -eq $script:FixtureInstall) 'Relaunch cwd must match target'
    $script:Launches++
}
function Move-Item {
    param([string]$LiteralPath, [string]$Destination)
    if ($script:FailPortableMove -and $LiteralPath -like '*\.neoism-update-*\neoism-agent.exe') {
        throw 'fixture interrupted portable replacement'
    }
    Microsoft.PowerShell.Management\Move-Item -LiteralPath $LiteralPath -Destination $Destination
}
function Invoke-Msi([string[]]$Arguments) {
    $script:MsiCalls += ,$Arguments
    if ($Arguments[0] -eq '/a') {
        $destination = Join-Path $TempDir 'payload\LocalAppData\Programs\Neoism'
        New-Item -ItemType Directory -Path $destination -Force | Out-Null
        Copy-Item -Path (Join-Path $script:FixturePayload '*') -Destination $destination -Recurse
        return 0
    }
    Assert ($script:Registration -eq $script:FixtureInstall) 'Portable update must NEVER invoke MSI install'
    Assert ($Arguments -contains '/L*v') 'MSI verbose log required'
    Assert ($Arguments -contains 'MSIRESTARTMANAGERCONTROL=Disable') 'MSI must not kill unrelated applications'
    if ($script:SameProduct) {
        Assert ($Arguments -contains 'REINSTALL=ALL' -and $Arguments -contains 'REINSTALLMODE=vamus') 'Same ProductCode must explicitly repair all features from the verified source'
    } else {
        Assert ($Arguments -notcontains 'REINSTALL=ALL' -and $Arguments -notcontains 'REINSTALLMODE=vamus') 'First install/major upgrade must not use repair-only flags'
        Assert ($Arguments -contains 'REINSTALLMODE=amus') 'Normal installation overwrite mode missing'
    }
    if ($script:InstallCode -eq 0) {
        Copy-Item -Path (Join-Path $script:FixturePayload '*') -Destination $script:FixtureInstall -Recurse -Force
        if ($script:MissingInstalled) { Remove-Item -LiteralPath (Join-Path $script:FixtureInstall 'neoism-agent.exe') }
        if ($script:CorruptInstalled) { Set-Content -LiteralPath (Join-Path $script:FixtureInstall 'neoism-workspace-daemon.exe') -Value 'wrong' }
    }
    return $script:InstallCode
}
function New-Fixture([string]$Mode) {
    $case = Join-Path $root ([Guid]::NewGuid().ToString('N'))
    $script:FixtureInstall = Join-Path $case 'invoking stack'
    $script:FixturePayload = Join-Path $case 'source'
    $script:TempDir = Join-Path $case 'download'
    $script:ResultPath = Join-Path $case 'receipt\result.json'
    $env:LOCALAPPDATA = Join-Path $case 'local'
    New-Item -ItemType Directory -Path $script:FixtureInstall, $script:FixturePayload, $script:TempDir, (Split-Path $ResultPath), (Join-Path $env:LOCALAPPDATA 'Neoism\updates') -Force | Out-Null
    $script:ExpectedVersion = '1.2.3'
    foreach ($name in $script:BinaryNames) {
        $label = [IO.Path]::GetFileNameWithoutExtension($name)
        [IO.File]::WriteAllText((Join-Path $script:FixturePayload $name), "$label 1.2.3")
        [IO.File]::WriteAllText((Join-Path $script:FixtureInstall $name), "$label 0.1.0")
    }
    New-Item -ItemType Directory -Path (Join-Path $script:FixturePayload 'web'), (Join-Path $script:FixtureInstall 'web') | Out-Null
    [IO.File]::WriteAllText((Join-Path $script:FixturePayload 'web\index.html'), 'new web')
    [IO.File]::WriteAllText((Join-Path $script:FixtureInstall 'web\index.html'), 'old web')
    [IO.File]::WriteAllText((Join-Path $script:FixtureInstall 'unrelated.txt'), 'preserve me')
    $script:MsiPath = Join-Path $TempDir 'fixture.msi'
    [IO.File]::WriteAllText($MsiPath, 'not a real MSI; Invoke-Msi is mocked')
    $script:InvokingExe = Join-Path $script:FixtureInstall 'neoism.exe'
    $script:Registration = if ($Mode -eq 'managed') { $script:FixtureInstall } else { Join-Path $case 'different managed copy' }
    $script:Relaunch = 1; $script:InstallCode = 0; $script:Launches = 0; $script:SameProduct = $false
    $script:MissingInstalled = $false; $script:CorruptInstalled = $false; $script:Locked = $false
    $script:FailPortableMove = $false; $script:SawHandoff = $false; $script:MsiCalls = @()
}
function Read-Receipt { return (Get-Content -LiteralPath $ResultPath -Raw | ConvertFrom-Json) }
try {
    foreach ($mode in @('managed', 'portable')) {
        New-Fixture $mode
        Remove-Item -LiteralPath $env:LOCALAPPDATA -Recurse -Force
        Assert ((Invoke-WindowsUpdate) -eq 0) "$mode must complete without a precreated update directory"
        $receipt = Read-Receipt
        Assert ($receipt.state -eq 'succeeded' -and $receipt.installation_verified) "$mode verified completion"
        Assert ($script:SawHandoff -and $script:Launches -eq 1) "$mode handoff before one verified relaunch"
        Assert (([IO.File]::ReadAllText((Join-Path $script:FixtureInstall 'unrelated.txt'))) -eq 'preserve me') 'Must preserve unrelated files'
        Assert (Test-Path -LiteralPath $receipt.log) 'Durable log survives download cleanup'
        Assert (-not (Test-Path -LiteralPath $TempDir)) 'Verified success may clean extraction, not result'
        if ($mode -eq 'portable') {
            Assert ($script:MsiCalls.Count -eq 1 -and $script:MsiCalls[0][0] -eq '/a') 'Portable does extraction only'
            Assert (Test-Path -LiteralPath (Join-Path $receipt.rollback_directory 'neoism.exe')) 'Portable rollback originals retained'
        }
    }
    New-Fixture 'managed'; $script:SameProduct = $true
    Assert ((Invoke-WindowsUpdate) -eq 0) 'Already installed ProductCode must repair successfully'
    Assert ((Read-Receipt).installation_verified -and $script:Launches -eq 1) 'Repair must verify all installed hashes/versions before relaunch'
    foreach ($code in @(3010, 1641, 1603, 1618)) {
        New-Fixture 'managed'; $script:InstallCode = $code
        $exit = Invoke-WindowsUpdate
        $receipt = Read-Receipt
        $expectedState = if ($code -in @(3010, 1641)) { 'reboot_required' } else { 'failed' }
        Assert ($exit -ne 0 -and $receipt.state -eq $expectedState) "MSI $code must not claim success"
        Assert ($script:Launches -eq 0 -and -not $receipt.installation_verified) "MSI $code must not relaunch"
        Assert (Test-Path -LiteralPath $TempDir) 'Failure/reboot evidence retained'
    }
    foreach ($failure in @('missing', 'hash', 'version', 'locked', 'cancelled')) {
        New-Fixture 'managed'
        switch ($failure) {
            'missing' { $script:MissingInstalled = $true }
            'hash' { $script:CorruptInstalled = $true }
            'version' { [IO.File]::WriteAllText((Join-Path $script:FixturePayload 'neoism-agent.exe'), 'neoism-agent 0.0.0') }
            'locked' { $script:Locked = $true }
            'cancelled' { [IO.File]::WriteAllText((Join-Path (Split-Path $ResultPath) 'cancel'), 'cancelled') }
        }
        Assert ((Invoke-WindowsUpdate) -eq 1) "$failure must fail"
        Assert ((Read-Receipt).state -eq 'failed' -and $script:Launches -eq 0) "$failure must persist failure without relaunch"
        if ($failure -in @('version', 'cancelled')) { Assert (-not $script:SawHandoff) "$failure must fail before handoff" }
    }
    New-Fixture 'portable'
    Remove-Item -LiteralPath (Join-Path $script:FixtureInstall 'neoism-agent.exe'), (Join-Path $script:FixtureInstall 'neoism-workspace-daemon.exe'), (Join-Path $script:FixtureInstall 'web') -Recurse -Force
    Assert ((Invoke-WindowsUpdate) -eq 0) 'Loose portable copy must expand into a complete in-place stack'
    Assert ($script:MsiCalls.Count -eq 1 -and $script:Launches -eq 1) 'Loose portable must not migrate to managed MSI path'
    New-Fixture 'portable'
    Remove-Item -LiteralPath (Join-Path $script:FixtureInstall 'neoism-agent.exe')
    Assert ((Invoke-WindowsUpdate) -eq 1 -and -not $script:SawHandoff) 'Unowned web collision must fail before closing GUI'
    Assert (([IO.File]::ReadAllText((Join-Path $script:FixtureInstall 'web\index.html'))) -eq 'old web') 'Unowned web must not be overwritten'
    New-Fixture 'managed'
    [IO.File]::WriteAllText((Join-Path $env:LOCALAPPDATA 'Neoism\updates\recovery-required.txt'), 'fixture MSI timeout')
    Assert ((Invoke-WindowsUpdate) -eq 1 -and $script:MsiCalls.Count -eq 0) 'Unresolved MSI timeout must block overlapping updates'
    New-Fixture 'portable'; $script:FailPortableMove = $true
    Assert ((Invoke-WindowsUpdate) -eq 1) 'Portable interrupted replacement must fail'
    foreach ($name in $script:BinaryNames) {
        Assert (([IO.File]::ReadAllText((Join-Path $script:FixtureInstall $name))) -like '*0.1.0') 'Portable rollback must restore every original binary'
    }
    Assert (([IO.File]::ReadAllText((Join-Path $script:FixtureInstall 'web\index.html'))) -eq 'old web') 'Portable rollback must restore original web'
    Assert ($script:Launches -eq 0) 'No launch following portable rollback'
    New-Fixture 'managed'
    $lock = [IO.File]::Open((Join-Path $env:LOCALAPPDATA 'Neoism\updates\update.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try {
        Assert ((Invoke-WindowsUpdate) -eq 1) 'Concurrent updater must fail before changing anything'
        Assert (-not $script:SawHandoff -and $script:MsiCalls.Count -eq 0) 'Concurrent update cannot hand off or extract'
    } finally { $lock.Dispose() }
    Write-Output 'neoism-windows-updater-tests-passed'
} finally {
    $env:LOCALAPPDATA = $originalLocal
    Remove-Item -LiteralPath $root -Recurse -Force
}
