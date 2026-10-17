# Read the live Process.Path getter only once: a process can exit between reads.
# Compare with a directory separator so a sibling such as Neoism-old is excluded.
function Test-InstalledProcess {
    param($Process, [Parameter(Mandatory)][string]$InstallDir)
    try { $processPath = $Process.Path } catch { return $false }
    $prefix = $InstallDir.TrimEnd([char[]]'\/') + [IO.Path]::DirectorySeparatorChar
    return -not [string]::IsNullOrEmpty($processPath) -and
        $processPath.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)
}

# Emit to the host, not the success pipeline (callers return structured receipts).
function Write-ValidationPhase {
    param([string]$Evidence, [string]$Phase)
    $line = "$([DateTime]::UtcNow.ToString('o')) validation-pid=$PID $Phase"
    Write-Host $line
    if ($Evidence) { Add-Content -LiteralPath (Join-Path $Evidence 'phases.log') -Value $line }
}

# Capture ancestry and resource pressure without command lines/environment secrets.
# This is diagnostic evidence, not another acceptance check.
function Write-ValidationSnapshot {
    param([Parameter(Mandatory)][string]$Evidence, [string]$Name = 'pre-smoke')
    New-Item -ItemType Directory -Force $Evidence | Out-Null
    $snapshot = [ordered]@{ time = [DateTime]::UtcNow.ToString('o'); validation_pid = $PID;
        version = $env:NEOISM_RELEASE_VERSION; run = $env:GITHUB_RUN_ID; attempt = $env:GITHUB_RUN_ATTEMPT;
        image = $env:ImageVersion; powershell = "$($PSVersionTable.PSVersion)" }
    try {
        $snapshot.processes = @(Get-CimInstance Win32_Process -OperationTimeoutSec 5 -ErrorAction Stop |
            Select-Object ProcessId, ParentProcessId, CreationDate, Name, ExecutablePath, WorkingSetSize, HandleCount)
        $snapshot.os = Get-CimInstance Win32_OperatingSystem -OperationTimeoutSec 5 -ErrorAction Stop |
            Select-Object LastBootUpTime, FreePhysicalMemory, TotalVisibleMemorySize, FreeVirtualMemory
        $snapshot.disks = @(Get-PSDrive -PSProvider FileSystem | Select-Object Name, Used, Free)
    } catch {
        $snapshot.error = "$_"
        Write-Warning "Snapshot $Name incomplete: $_"
    }
    try {
        $snapshot.system_events = @(Get-WinEvent -FilterHashtable @{
                LogName = 'System'; StartTime = (Get-Date).AddHours(-1); Id = @(41, 1074, 6005, 6006, 6008, 2004)
            } -MaxEvents 40 -ErrorAction Stop |
            Select-Object TimeCreated, Id, ProviderName, Message)
    } catch {
        # An empty result also raises NoMatchingEventsFound; record it explicitly.
        $snapshot.system_events_note = "$_"
        Write-Warning "System-event snapshot ${Name}: $_"
    }
    $snapshot | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $Evidence "$Name.json")
}

# Process.WaitForExit(int) does not guarantee redirected output callbacks have
# drained. Preserve that guarantee, but never block the validation thread on EOF:
# a descendant can retain an inherited pipe after its parent has exited.
function Wait-ValidationOutput {
    param([Diagnostics.Process]$Process)
    if (-not ('NeoismValidationOutput' -as [type])) {
        Add-Type @'
using System.Diagnostics;
using System.Threading.Tasks;
public static class NeoismValidationOutput {
    public static bool Drain(Process process, int milliseconds) {
        return Task.Run(() => process.WaitForExit()).Wait(milliseconds);
    }
}
'@
    }
    if (-not [NeoismValidationOutput]::Drain($Process, 5000)) {
        throw "Redirected output did not drain within 5s for process $($Process.Id)"
    }
}

# Retain a real process handle before using taskkill's numeric PID. On Windows
# this pins process identity against PID reuse. Never kill the validation shell.
# Start-Process -Wait and parameterless WaitForExit can wait indefinitely for
# descendants/inherited redirect handles; every wait below has its own bound.
function Stop-ValidationProcessTree {
    param([Parameter(Mandatory)]$Process, [string]$Log)
    if ($Process.Id -le 0 -or $Process.Id -eq $PID) { throw 'Refusing to kill the validation shell or a nonpositive PID' }
    $null = $Process.Handle
    $Process.Refresh()
    if ($Process.HasExited) { return }
    $options = @{
        FilePath = 'taskkill.exe'
        ArgumentList = @('/PID', $Process.Id, '/T', '/F')
        PassThru = $true
        NoNewWindow = $true
    }
    if ($Log) {
        $options.RedirectStandardOutput = $Log
        $options.RedirectStandardError = "$Log.stderr.log"
    }
    Write-ValidationPhase -Phase "cleanup begin target-pid=$($Process.Id)"
    $killer = Start-Process @options
    try {
        if (-not $killer.WaitForExit(10000)) {
            $killer.Kill()
            if (-not $killer.WaitForExit(5000)) { throw 'Cleanup taskkill itself would not stop' }
            throw "Cleanup taskkill timed out for process $($Process.Id)"
        }
        # A disappearing descendant can make taskkill return nonzero even when
        # the target has gone; target liveness, not taskkill's status, is decisive.
        if (-not $Process.WaitForExit(5000)) {
            throw "Cleanup failed: process $($Process.Id) is still running (taskkill exit=$($killer.ExitCode))"
        }
        Write-ValidationPhase -Phase "cleanup complete target-pid=$($Process.Id) taskkill-exit=$($killer.ExitCode)"
    } finally {
        $killer.Dispose()
        if ($Log) {
            Get-Content -LiteralPath "$Log.stderr.log" -Tail 100 -ErrorAction SilentlyContinue | Out-Host
        }
    }
}

# stdout/stderr are retained on disk, including timeout/failure diagnostics.
function Invoke-CheckedProcess {
    param([string]$File, [string]$Arguments, [string]$Log,
          [ValidateRange(1, 1800)][int]$Seconds = 300, [int[]]$AllowedExitCodes = @(0))
    "$File $Arguments" | Set-Content "$Log.command.txt"
    Write-ValidationPhase -Phase "process begin file=$File timeout=${Seconds}s log=$Log"
    $p = Start-Process -FilePath $File -ArgumentList $Arguments -PassThru -NoNewWindow `
        -RedirectStandardOutput "$Log.stdout.log" -RedirectStandardError "$Log.stderr.log"
    try {
        $null = $p.Handle
        Write-ValidationPhase -Phase "process started target-pid=$($p.Id) log=$Log"
        if (-not $p.WaitForExit($Seconds * 1000)) {
            "TIMEOUT after ${Seconds}s" | Set-Content "$Log.result.txt"
            throw "$File timed out; see $Log"
        }
        try { Wait-ValidationOutput $p }
        catch {
            "OUTPUT FAILURE: $_" | Set-Content "$Log.result.txt"
            throw
        }
        "exit=$($p.ExitCode)" | Set-Content "$Log.result.txt"
        Write-ValidationPhase -Phase "process exited target-pid=$($p.Id) exit=$($p.ExitCode) log=$Log"
        if ($p.ExitCode -notin $AllowedExitCodes) { throw "$File exited $($p.ExitCode); see $Log" }
    } finally {
        try {
            if (-not $p.HasExited) { Stop-ValidationProcessTree -Process $p -Log "$Log.cleanup.log" }
        } finally {
            $p.Dispose()
            foreach ($stream in @('stdout', 'stderr')) {
                Get-Content "$Log.$stream.log" -Tail 100 -ErrorAction SilentlyContinue | Out-Host
            }
        }
    }
}
