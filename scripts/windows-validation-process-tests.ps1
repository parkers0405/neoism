# No Pester dependency; also runnable under PowerShell on Linux.
$ErrorActionPreference = 'Stop'
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
# Mock only process launching; exercise cleanup's real exit-code/identity logic.
$script:launches = 0
function Start-Process {
    param($FilePath, $ArgumentList, [switch]$PassThru, [switch]$Wait,
          [switch]$NoNewWindow, $RedirectStandardOutput, $RedirectStandardError)
    $script:launches++
    Assert-Equal $FilePath 'taskkill.exe' 'cleanup executable'
    return [pscustomobject]@{ ExitCode = 1 }
}
$process = [pscustomobject]@{ Id = 123; HasExited = $false; Exits = $true }
$process | Add-Member ScriptMethod Refresh {}
$process | Add-Member ScriptMethod WaitForExit { param($milliseconds) return $this.Exits }
$global:LASTEXITCODE = 0
Stop-ValidationProcessTree -Process $process
Assert-Equal $global:LASTEXITCODE 0 'taskkill failure does not leak into CI exit'
Assert-Equal $script:launches 1 'cleanup launched once'
$process.HasExited = $true
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
Remove-Item Function:Start-Process
'PASS: installed process cleanup regression tests'
