# No Pester dependency; also runnable under PowerShell on Linux.
$ErrorActionPreference = 'Stop'
foreach ($name in @('windows-validation-process.ps1', 'windows-msi-validation.ps1',
        'windows-updater-native-validation.ps1', 'windows-installed-gui-smoke.ps1')) {
    $tokens = $null; $parseErrors = $null
    $null = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $name), [ref]$tokens, [ref]$parseErrors)
    if ($parseErrors.Count) { throw ($parseErrors | Format-List | Out-String) }
}
. "$PSScriptRoot/windows-validation-process.ps1"
$root = Join-Path ([IO.Path]::GetTempPath()) 'Neoism'
$inside = Join-Path $root 'neoism.exe'
function Assert-Equal($actual, $expected, $label) {
    if ($actual -ne $expected) { throw "${label}: expected $expected, got $actual" }
}
Assert-Equal (Test-InstalledProcess ([pscustomobject]@{ Path = $inside }) $root) $true 'installed'
Assert-Equal (Test-InstalledProcess ([pscustomobject]@{ Path = $inside.ToUpperInvariant() }) $root) $true 'case insensitive'
Assert-Equal (Test-InstalledProcess ([pscustomobject]@{ Path = $null }) $root) $false 'exited'
Assert-Equal (Test-InstalledProcess ([pscustomobject]@{ Path = '' }) $root) $false 'empty'
Assert-Equal (Test-InstalledProcess ([pscustomobject]@{ Path = "$root-old/neoism.exe" }) $root) $false 'sibling'
Assert-Equal (Test-InstalledProcess ([pscustomobject]@{ Path = $inside }) ($root + [IO.Path]::DirectorySeparatorChar)) $true 'trailing separator'
$script:reads = 0
$racing = [pscustomobject]@{}
$racing | Add-Member ScriptProperty Path {
    $script:reads++
    if ($script:reads -eq 1) { return $inside }
    return $null
}
Assert-Equal (Test-InstalledProcess $racing $root) $true 'exit between getter reads'
Assert-Equal $script:reads 1 'getter read once'
$inaccessible = [pscustomobject]@{}
$inaccessible | Add-Member ScriptProperty Path { throw 'Process exited or access denied' }
Assert-Equal (Test-InstalledProcess $inaccessible $root) $false 'unavailable getter'
# Mock process launching, not helper logic. Reject any unbounded wait so the
# original Start-Process -Wait / parameterless WaitForExit regressions fail here.
$script:launches = 0
$script:killerExits = $true
$script:killerStops = $true
function New-FakeProcess([int]$Identity, [bool]$Exited, [bool]$Exits, [int]$Code = 0) {
    $fake = [pscustomobject]@{ Id = $Identity; Handle = 1; HasExited = $Exited; Exits = $Exits;
        ExitCode = $Code; Killed = $false; Disposed = $false; Waits = @() }
    $fake | Add-Member ScriptMethod Refresh {}
    $fake | Add-Member ScriptMethod WaitForExit {
        param($milliseconds)
        if ($null -eq $milliseconds -or $milliseconds -le 0) { throw 'Unbounded wait forbidden' }
        $this.Waits += $milliseconds
        if ($this.Exits) { $this.HasExited = $true }
        return $this.Exits
    }
    $fake | Add-Member ScriptMethod Kill { $this.Killed = $true; $this.Exits = $script:killerStops }
    $fake | Add-Member ScriptMethod Dispose { $this.Disposed = $true }
    return $fake
}
function Start-Process {
    param($FilePath, $ArgumentList, [switch]$PassThru, [switch]$Wait,
          [switch]$NoNewWindow, $RedirectStandardOutput, $RedirectStandardError)
    if ($Wait) { throw 'Start-Process -Wait forbidden' }
    $script:launches++
    if ($FilePath -eq 'taskkill.exe') {
        $script:killer = New-FakeProcess 456 $false $script:killerExits 1
        return $script:killer
    }
    'fixture stdout' | Set-Content $RedirectStandardOutput
    'fixture stderr' | Set-Content $RedirectStandardError
    return $script:checked
}
$process = New-FakeProcess 123 $false $true
$global:LASTEXITCODE = 0
Stop-ValidationProcessTree -Process $process
Assert-Equal $global:LASTEXITCODE 0 'taskkill failure does not leak into CI exit'
Assert-Equal $script:launches 1 'cleanup launched once'
Assert-Equal $script:killer.Disposed $true 'killer disposed'
Stop-ValidationProcessTree -Process $process
Assert-Equal $script:launches 1 'already exited process needs no taskkill'
$process.HasExited = $false
$process.Exits = $false
$failed = $false
try { Stop-ValidationProcessTree -Process $process } catch {
    if ($_ -notmatch 'still running') { throw }
    $failed = $true
}
Assert-Equal $failed $true 'surviving process fails cleanup'
$script:killerExits = $false
$failed = $false
try { Stop-ValidationProcessTree -Process $process } catch {
    if ($_ -notmatch 'taskkill timed out') { throw }
    $failed = $true
}
Assert-Equal $failed $true 'hung taskkill fails cleanup'
Assert-Equal $script:killer.Killed $true 'hung taskkill killed by handle'
Assert-Equal ($script:killer.Waits -join ',') '10000,5000' 'killer waits bounded'
Assert-Equal $script:killer.Disposed $true 'hung killer disposed'
$before = $script:launches
foreach ($identity in @(0, -1, $PID)) {
    $failed = $false
    try { Stop-ValidationProcessTree -Process (New-FakeProcess $identity $false $false) } catch {
        if ($_ -notmatch 'Refusing to kill') { throw }
        $failed = $true
    }
    Assert-Equal $failed $true 'unsafe PID rejected'
}
Assert-Equal $script:launches $before 'unsafe PID never reaches taskkill'
$script:killerExits = $true
$script:outputDrains = $true
function Wait-ValidationOutput {
    param($Process)
    if (-not $script:outputDrains) { throw 'Redirected output did not drain within 5s' }
}
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('neoism-process-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $testRoot | Out-Null
try {
    $log = Join-Path $testRoot 'checked'
    $script:checked = New-FakeProcess 789 $false $true 0
    $output = @(Invoke-CheckedProcess 'fixture.exe' '--version' $log 1)
    Assert-Equal $output.Count 0 'host logging does not pollute success pipeline'
    Assert-Equal (Get-Content "$log.result.txt") 'exit=0' 'exit receipt'
    Assert-Equal $script:checked.Disposed $true 'checked process disposed'
    $script:checked = New-FakeProcess 789 $false $true 7
    $failed = $false
    try { Invoke-CheckedProcess 'fixture.exe' '' $log 1 } catch {
        if ($_ -notmatch 'exited 7') { throw }
        $failed = $true
    }
    Assert-Equal $failed $true 'nonzero exit fails validation'
    Invoke-CheckedProcess 'fixture.exe' '' $log 1 @(7)
    # Simulate taskkill finishing the target after its first timed wait.
    $script:checked = New-FakeProcess 789 $false $false
    $script:checked | Add-Member ScriptMethod WaitForExit {
        param($milliseconds)
        if ($null -eq $milliseconds) { throw 'Unbounded wait forbidden' }
        $this.Waits += $milliseconds
        if ($this.Waits.Count -gt 1) { $this.HasExited = $true; return $true }
        return $false
    } -Force
    $failed = $false
    try { Invoke-CheckedProcess 'fixture.exe' '' $log 1 } catch {
        if ($_ -notmatch 'timed out') { throw }
        $failed = $true
    }
    Assert-Equal $failed $true 'timeout remains failure after successful cleanup'
    Assert-Equal (Get-Content "$log.result.txt") 'TIMEOUT after 1s' 'timeout receipt retained'
    Assert-Equal (Get-Content "$log.stderr.log") 'fixture stderr' 'stderr retained'
    Assert-Equal ($script:checked.Waits -join ',') '1000,5000' 'checked waits bounded'
    Assert-Equal $script:checked.Disposed $true 'timed-out process disposed'
    $script:checked = New-FakeProcess 789 $false $true
    $script:outputDrains = $false
    $failed = $false
    try { Invoke-CheckedProcess 'fixture.exe' '' $log 1 } catch {
        if ($_ -notmatch 'output did not drain') { throw }
        $failed = $true
    }
    Assert-Equal $failed $true 'inherited-pipe output timeout fails validation'
    Assert-Equal (Get-Content "$log.result.txt") 'OUTPUT FAILURE: Redirected output did not drain within 5s' 'output failure receipt'
    Assert-Equal $script:checked.Disposed $true 'output-timeout process disposed'
} finally {
    Remove-Item Function:Start-Process
    Remove-Item Function:Wait-ValidationOutput
    Remove-Item -LiteralPath $testRoot -Recurse -Force
}
# Real subprocess/redirect test on Linux and Windows. No MSI or product launch.
. "$PSScriptRoot/windows-validation-process.ps1"
New-Item -ItemType Directory $testRoot | Out-Null
$real = $null
try {
    $engine = (Get-Process -Id $PID).Path
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes(
        '[Console]::Out.WriteLine("real stdout"); [Console]::Error.WriteLine("real stderr"); exit 9'))
    $log = Join-Path $testRoot 'real'
    Invoke-CheckedProcess $engine "-NoLogo -NoProfile -EncodedCommand $encoded" $log 15 @(9)
    Assert-Equal (Get-Content "$log.stdout.log") 'real stdout' 'real stdout fully drained'
    Assert-Equal (Get-Content "$log.stderr.log") 'real stderr' 'real stderr fully drained'
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes('Start-Sleep -Seconds 30'))
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $engine
    $info.Arguments = "-NoLogo -NoProfile -EncodedCommand $encoded"
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $real = New-Object Diagnostics.Process
    $real.StartInfo = $info
    [void]$real.Start()
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $failed = $false
    try { Wait-ValidationOutput $real } catch {
        if ($_ -notmatch 'output did not drain') { throw }
        $failed = $true
    }
    Assert-Equal $failed $true 'real output/exit drain has a deadline'
    if ($timer.Elapsed.TotalSeconds -gt 12) { throw 'Output deadline exceeded 12s test allowance' }
} finally {
    if ($null -ne $real) {
        if (-not $real.HasExited) { $real.Kill(); [void]$real.WaitForExit(5000) }
        $real.Dispose()
    }
    Remove-Item -LiteralPath $testRoot -Recurse -Force
}
'PASS: process identity guards, bounded waits, timeout/exit propagation, and real redirected subprocess regression tests'
