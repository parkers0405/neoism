# Read the live Process.Path getter only once: a process can exit between reads.
# Compare with a directory separator so a sibling such as Neoism-old is excluded.
function Test-InstalledProcess {
    param($Process, [Parameter(Mandatory)][string]$InstallDir)
    try { $processPath = $Process.Path } catch { return $false }
    $prefix = $InstallDir.TrimEnd([char[]]'\/') + [IO.Path]::DirectorySeparatorChar
    return -not [string]::IsNullOrEmpty($processPath) -and
        $processPath.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)
}

# Start-Process keeps taskkill's exit code out of the caller's LASTEXITCODE.
# A disappearing child can make taskkill fail even after the parent is gone.
function Stop-ValidationProcessTree {
    param([Parameter(Mandatory)]$Process, [string]$Log)
    $Process.Refresh()
    if ($Process.HasExited) { return }
    $options = @{
        FilePath = 'taskkill.exe'
        ArgumentList = @('/PID', $Process.Id, '/T', '/F')
        PassThru = $true
        Wait = $true
        NoNewWindow = $true
    }
    if ($Log) {
        $options.RedirectStandardOutput = $Log
        $options.RedirectStandardError = "$Log.stderr.log"
    }
    $killer = Start-Process @options
    if (-not $Process.WaitForExit(5000)) {
        throw "Cleanup failed: process $($Process.Id) is still running (taskkill exit=$($killer.ExitCode))"
    }
}

# Shared bounded process runner. stdout/stderr go directly to disk even on timeout.
function Invoke-CheckedProcess {
    param([string]$File, [string]$Arguments, [string]$Log, [int]$Seconds = 300,
          [int[]]$AllowedExitCodes = @(0))
    "$File $Arguments" | Set-Content "$Log.command.txt"
    $p = Start-Process -FilePath $File -ArgumentList $Arguments -PassThru -NoNewWindow `
        -RedirectStandardOutput "$Log.stdout.log" -RedirectStandardError "$Log.stderr.log"
    try {
        if (-not $p.WaitForExit($Seconds * 1000)) {
            "TIMEOUT after ${Seconds}s" | Set-Content "$Log.result.txt"
            throw "$File timed out; see $Log"
        }
        $p.WaitForExit()
        "exit=$($p.ExitCode)" | Set-Content "$Log.result.txt"
        if ($p.ExitCode -notin $AllowedExitCodes) { throw "$File exited $($p.ExitCode); see $Log" }
    } finally {
        if (-not $p.HasExited) { Stop-ValidationProcessTree -Process $p -Log "$Log.cleanup.log" }
        foreach ($stream in @('stdout', 'stderr')) {
            Get-Content "$Log.$stream.log" -Tail 100 -ErrorAction SilentlyContinue
        }
    }
}
