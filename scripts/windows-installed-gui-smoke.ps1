# Starts only the unmodified installed executable; no synthetic keyboard input.
param([Parameter(Mandatory)][string]$InstallDir, [Parameter(Mandatory)][string]$Evidence)
$ErrorActionPreference = 'Stop'
$started = Get-Date
$oldConfig = $env:NEOISM_CONFIG_HOME
$p = $null
try {
    $env:NEOISM_CONFIG_HOME = Join-Path $Evidence 'config'
    $workspace = Join-Path $Evidence 'workspace'
    New-Item -ItemType Directory -Force $env:NEOISM_CONFIG_HOME, $workspace | Out-Null
    $p = Start-Process (Join-Path $InstallDir 'neoism.exe') -PassThru -WorkingDirectory $workspace `
        -ArgumentList "--enable-log-file --working-dir `"$workspace`"" `
        -RedirectStandardOutput "$Evidence/gui.stdout.log" -RedirectStandardError "$Evidence/gui.stderr.log"
    # Require a real visible top-level window and bounded WM_NULL replies, not just a live PID.
    Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class NeoismWindowProbe {
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SendMessageTimeout(
        IntPtr h, uint msg, UIntPtr w, IntPtr l, uint flags, uint timeout, out UIntPtr result);
}
'@
    $deadline = (Get-Date).AddSeconds(90)
    $samples = 0
    while ((Get-Date) -lt $deadline) {
        $p.Refresh()
        if ($p.HasExited) { throw "Installed GUI crashed/exited: $($p.ExitCode) (stack overflow is 0xC00000FD; do not patch stack reserve)" }
        $h = $p.MainWindowHandle
        $reply = [UIntPtr]::Zero
        $responsive = $h -ne [IntPtr]::Zero -and [NeoismWindowProbe]::IsWindowVisible($h) -and `
            [NeoismWindowProbe]::SendMessageTimeout($h, 0, [UIntPtr]::Zero, [IntPtr]::Zero, 2, 2000, [ref]$reply) -ne [IntPtr]::Zero
        "$(Get-Date -Format o) pid=$($p.Id) hwnd=$h responsive=$responsive" | Add-Content "$Evidence/window.log"
        if ($responsive) { $samples++ } else { $samples = 0 }
        if ($samples -ge 15) { break }
        Start-Sleep -Seconds 1
    }
    if ($samples -lt 15) { throw 'No continuously responsive visible installed GUI window within 90 seconds' }
    'PASS: visible window replied for 15 samples. NOT proof of rendered terminal, GPU correctness, composer, or keyboard execution.' |
        Set-Content "$Evidence/gui-passed.txt"
} finally {
    # Capture the interactive desktop even on startup failure; absence is recorded, not concealed.
    try {
        Add-Type -AssemblyName System.Windows.Forms, System.Drawing
        $bounds = [Windows.Forms.SystemInformation]::VirtualScreen
        $bitmap = [Drawing.Bitmap]::new($bounds.Width, $bounds.Height)
        $graphics = [Drawing.Graphics]::FromImage($bitmap)
        try {
            $graphics.CopyFromScreen($bounds.Location, [Drawing.Point]::Empty, $bounds.Size)
            $bitmap.Save("$Evidence/desktop.png")
        } finally { $graphics.Dispose(); $bitmap.Dispose() }
    } catch { "Screenshot unavailable: $_" | Set-Content "$Evidence/screenshot-error.txt" }
    if ($null -ne $p) {
        $p.Refresh()
        if ($p.HasExited) { "exit=$($p.ExitCode)" | Set-Content "$Evidence/gui-exit.txt" }
        else { & taskkill.exe /PID $p.Id /T /F | Out-File "$Evidence/gui-cleanup.log" }
    }
    # Catch detached installed daemon/agent children on this dedicated CI runner before uninstall.
    Get-Process neoism, neoism-workspace-daemon, neoism-agent -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path.StartsWith($InstallDir, [StringComparison]::OrdinalIgnoreCase) } |
        Stop-Process -Force -ErrorAction Continue
    Start-Sleep -Seconds 2
    Get-WinEvent -FilterHashtable @{ LogName = 'Application'; StartTime = $started } -ErrorAction SilentlyContinue |
        Where-Object { $_.ProviderName -in @('Application Error', 'Windows Error Reporting', 'Application Hang') } |
        Format-List TimeCreated, Id, ProviderName, Message | Out-File "$Evidence/application-events.log"
    $env:NEOISM_CONFIG_HOME = $oldConfig
}
