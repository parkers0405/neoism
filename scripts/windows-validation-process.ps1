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
        if (-not $p.HasExited) { & taskkill.exe /PID $p.Id /T /F | Out-Null }
        foreach ($stream in @('stdout', 'stderr')) {
            Get-Content "$Log.$stream.log" -Tail 100 -ErrorAction SilentlyContinue
        }
    }
}
