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
    # Require a real visible top-level window and bounded WM_NULL replies, not just a live PID.
    Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class NeoismWindowProbe {
    public sealed class Window {
        public long Handle;
        public uint ProcessId;
        public string Title;
        public string ClassName;
    }
    private delegate bool EnumWindowProc(IntPtr h, IntPtr parameter);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumWindowProc callback, IntPtr parameter);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr h, out uint processId);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] private static extern int GetWindowText(IntPtr h, StringBuilder text, int length);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] private static extern int GetClassName(IntPtr h, StringBuilder text, int length);
    [StructLayout(LayoutKind.Sequential)]
    private struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] private static extern bool GetWindowRect(IntPtr h, out Rect rect);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SendMessageTimeout(
        IntPtr h, uint msg, UIntPtr w, IntPtr l, uint flags, uint timeout, out UIntPtr result);
    public static Window[] Snapshot() {
        var windows = new List<Window>();
        EnumWindows((h, unused) => {
            if (!IsWindowVisible(h)) return true;
            // Winit's zero-size event target carries WS_VISIBLE for WM_PAINT
            // dispatch but is not displayed. Count actual window rectangles.
            Rect rect;
            if (!GetWindowRect(h, out rect) || rect.Right <= rect.Left || rect.Bottom <= rect.Top) return true;
            uint pid;
            GetWindowThreadProcessId(h, out pid);
            var title = new StringBuilder(1024);
            var className = new StringBuilder(256);
            GetWindowText(h, title, title.Capacity);
            GetClassName(h, className, className.Capacity);
            windows.Add(new Window { Handle = h.ToInt64(), ProcessId = pid,
                Title = title.ToString(), ClassName = className.ToString() });
            return true;
        }, IntPtr.Zero);
        return windows.ToArray();
    }
}
'@
    $baseline = [Collections.Generic.HashSet[long]]::new()
    foreach ($window in [NeoismWindowProbe]::Snapshot()) { [void]$baseline.Add($window.Handle) }
    $p = Start-Process (Join-Path $InstallDir 'neoism.exe') -PassThru -WorkingDirectory $workspace `
        -ArgumentList "--enable-log-file --working-dir `"$workspace`"" `
        -RedirectStandardOutput "$Evidence/gui.stdout.log" -RedirectStandardError "$Evidence/gui.stderr.log"
    $deadline = (Get-Date).AddSeconds(90)
    $agentBase = if ($env:NEOISM_AGENT_SERVER) { $env:NEOISM_AGENT_SERVER } elseif ($env:NEOISM_SERVER) { $env:NEOISM_SERVER } else { 'http://127.0.0.1:4096' }
    $healthUrl = $agentBase.TrimEnd('/') + '/v2/health'
    $nextAgentProbe = [DateTime]::MinValue
    $agentReady = $false
    $samples = 0
    while ((Get-Date) -lt $deadline) {
        $p.Refresh()
        if ($p.HasExited) { throw "Installed GUI crashed/exited: $($p.ExitCode) (stack overflow is 0xC00000FD; do not patch stack reserve)" }
        $visibleWindows = [NeoismWindowProbe]::Snapshot()
        $mainWindows = @($visibleWindows | Where-Object { $_.ProcessId -eq $p.Id })
        # Process.MainWindowHandle can briefly select the same internal event
        # target. Use the positive-area windows for primary-window detection too.
        $h = if ($mainWindows.Count -gt 0) { [IntPtr]::new($mainWindows[0].Handle) } else { [IntPtr]::Zero }
        foreach ($window in $visibleWindows) {
            if ($baseline.Contains($window.Handle) -or $window.Handle -eq $h.ToInt64()) { continue }
            $owner = Get-Process -Id $window.ProcessId -ErrorAction SilentlyContinue
            $isUnexpected = $window.ProcessId -eq $p.Id -or
                $window.ClassName -eq 'ConsoleWindowClass' -or
                ($null -ne $owner -and $owner.ProcessName -match '^(neoism(-workspace-daemon|-agent)?|git|curl|icacls|tasklist|cmd|powershell|pwsh|conhost|OpenConsole|WindowsTerminal)$')
            if ($isUnexpected) {
                [pscustomobject]@{ time = (Get-Date -Format o); window = $window; process = $owner.ProcessName } |
                    ConvertTo-Json -Depth 4 -Compress | Add-Content "$Evidence/unexpected-windows.jsonl"
                Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, ExecutablePath |
                    ConvertTo-Json -Depth 3 | Set-Content "$Evidence/processes-on-window-failure.json"
                throw "Unexpected visible window: pid=$($window.ProcessId) process=$($owner.ProcessName) class=$($window.ClassName) title=$($window.Title)"
            }
        }
        $reply = [UIntPtr]::Zero
        $responsive = $h -ne [IntPtr]::Zero -and [NeoismWindowProbe]::IsWindowVisible($h) -and `
            [NeoismWindowProbe]::SendMessageTimeout($h, 0, [UIntPtr]::Zero, [IntPtr]::Zero, 2, 2000, [ref]$reply) -ne [IntPtr]::Zero
        "$(Get-Date -Format o) pid=$($p.Id) hwnd=$h responsive=$responsive" | Add-Content "$Evidence/window.log"
        if ($responsive) { $samples++ } else { $samples = 0 }
        if ((Get-Date) -ge $nextAgentProbe) {
            try {
                $health = (Invoke-WebRequest -UseBasicParsing -Uri $healthUrl -TimeoutSec 1).Content | ConvertFrom-Json
                $agentReady = $health.healthy -eq $true -and -not [string]::IsNullOrWhiteSpace($health.version) -and
                    (-not [string]::IsNullOrWhiteSpace($health.provider_credential_store) -or -not [string]::IsNullOrWhiteSpace($health.providerCredentialStore))
            } catch { $agentReady = $false }
            "$(Get-Date -Format o) ready=$agentReady" | Add-Content "$Evidence/agent-ready.log"
            $nextAgentProbe = (Get-Date).AddSeconds(1)
        }
        if ($samples -ge 150 -and $agentReady) { break }
        Start-Sleep -Milliseconds 100
    }
    if ($samples -lt 150) { throw 'No continuously responsive visible installed GUI window within 90 seconds' }
    if (-not $agentReady) { throw 'The installed GUI did not automatically start a healthy Agent server within 90 seconds; see config/log/workspace-service.log' }
    'PASS: visible window replied for 150 samples, Agent health became ready, and no extra Neoism/console-helper window was observed. Sampling cannot exclude shorter-lived windows. NOT proof of rendered terminal, GPU correctness, composer, or keyboard execution.' |
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
