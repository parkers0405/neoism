# Windows PowerShell 5.1 compatible. Dot-source with -LibraryOnly for isolated tests.
# No PATH rewriting, App Paths redirection, service autostart, or global image kills.
param(
    [int]$UpdaterPid, [int]$GuiPid,
    [string]$MsiPath, [string]$TempDir, [string]$InvokingExe,
    [string]$ExpectedVersion, [string]$ResultPath,
    [int]$Relaunch = 0, [switch]$LibraryOnly
)
Set-StrictMode -Version 3
$ErrorActionPreference = 'Stop'
$script:BinaryNames = @('neoism.exe', 'neoism-workspace-daemon.exe', 'neoism-agent.exe')

function Get-FullPath([string]$Path) {
    # Rust may supply a verbatim path while Process.Path/registry use DOS paths.
    if ($Path.StartsWith('\\?\UNC\', [StringComparison]::OrdinalIgnoreCase)) { $Path = '\\' + $Path.Substring(8) }
    elseif ($Path.StartsWith('\\?\', [StringComparison]::Ordinal)) { $Path = $Path.Substring(4) }
    if (-not [IO.Path]::IsPathRooted($Path)) { throw "Expected an absolute path: $Path" }
    $full = [IO.Path]::GetFullPath($Path)
    if ($full.Length -eq [IO.Path]::GetPathRoot($full).Length) { return $full }
    return $full.TrimEnd('\', '/')
}
function Test-SamePath([string]$Left, [string]$Right) {
    return [string]::Equals((Get-FullPath $Left), (Get-FullPath $Right), [StringComparison]::OrdinalIgnoreCase)
}
function Assert-NoReparsePoint([string]$Path) {
    $cursor = Get-FullPath $Path
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw "Update through a link/junction is unsupported; invoke the real installation: $cursor"
            }
        }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
}
function Get-RegisteredInstallDir {
    # WiX InstallRegistration is explicitly always64, including on ARM64.
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::CurrentUser, [Microsoft.Win32.RegistryView]::Registry64)
    try {
        $key = $base.OpenSubKey('Software\Neoism')
        if ($null -eq $key) { return $null }
        try { return $key.GetValue('InstallDir', $null) } finally { $key.Dispose() }
    } finally { $base.Dispose() }
}
function Resolve-UpdateTarget([string]$Exe, [string]$RegisteredDir) {
    $exePath = Get-FullPath $Exe
    if ([IO.Path]::GetFileName($exePath) -ine 'neoism.exe') {
        throw 'Rename the invoking portable executable to neoism.exe before updating.'
    }
    $directory = [IO.Path]::GetDirectoryName($exePath)
    $mode = 'portable'
    if ($RegisteredDir -and (Test-SamePath $directory $RegisteredDir)) { $mode = 'managed' }
    # An MSI elsewhere does not make this portable/PATH-shadowing copy managed.
    # Update THIS complete stack in place; never silently install into that MSI.
    return @{ mode = $mode; directory = $directory; executable = $exePath }
}
function Assert-VersionOutput([string]$Name, [string]$Output, [string]$Expected) {
    $label = [IO.Path]::GetFileNameWithoutExtension($Name)
    $pattern = '^' + [regex]::Escape($label) + '\s+v?' + [regex]::Escape($Expected) + '$'
    if ($Output.Trim() -cnotmatch $pattern) {
        throw "Release identity mismatch: $Name expected $Expected, reported '$($Output.Trim())'"
    }
}
function Get-BinaryVersion([string]$Exe) {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $Exe
    $info.Arguments = '--version'
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = New-Object Diagnostics.Process
    $process.StartInfo = $info
    try {
        if (-not $process.Start()) { throw "Cannot query version: $Exe" }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(15000)) {
            $process.Kill()
            if (-not $process.WaitForExit(5000)) { throw "Version process would not stop: $Exe" }
            throw "Version query timed out: $Exe"
        }
        if (-not $stdout.Wait(5000) -or -not $stderr.Wait(5000)) { throw "Version output timed out: $Exe" }
        if ($process.ExitCode -ne 0) { throw "Version query failed: $Exe ($($stderr.Result.Trim()))" }
        return $stdout.Result
    } finally { $process.Dispose() }
}
function Assert-StackVersion([string]$Directory, [string]$Expected) {
    foreach ($name in $script:BinaryNames) {
        $exe = Join-Path $Directory $name
        if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "Installed/payload executable missing: $exe" }
        Assert-VersionOutput $name (Get-BinaryVersion $exe) $Expected
    }
}
function Get-PayloadManifest([string]$Directory) {
    $manifest = @{}
    foreach ($name in $script:BinaryNames) {
        $manifest[$name] = (Get-FileHash -LiteralPath (Join-Path $Directory $name) -Algorithm SHA256).Hash
    }
    $web = Join-Path $Directory 'web'
    if (-not (Test-Path -LiteralPath $web -PathType Container)) { throw 'MSI payload is missing web assets' }
    foreach ($file in Get-ChildItem -LiteralPath $web -File -Recurse -Force) {
        Assert-NoReparsePoint $file.FullName
        $relative = $file.FullName.Substring($Directory.TrimEnd('\').Length + 1)
        $manifest[$relative] = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
    }
    return $manifest
}
function Assert-InstalledPayload([string]$Directory, [hashtable]$Manifest, [string]$Expected) {
    foreach ($relative in $Manifest.Keys) {
        $path = Join-Path $Directory $relative
        Assert-NoReparsePoint $path
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Installed payload missing: $path" }
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $Manifest[$relative]) {
            throw "Installed payload checksum mismatch: $path"
        }
    }
    Assert-StackVersion $Directory $Expected
}
function Write-UpdateResult([string]$State, [string]$Message) {
    $record = [ordered]@{
        protocol = 1; state = $State; message = $Message
        expected_version = $ExpectedVersion; updated_at = [DateTime]::UtcNow.ToString('o')
        target = $script:Target; msi_exit_code = $script:MsiExitCode
        installation_verified = $script:InstallationVerified
        log = $script:LogPath; msi_log = $script:MsiLogPath
        extract_msi_log = Join-Path (Split-Path $ResultPath) 'extract-msi.log'
        rollback_directory = $script:BackupDir
    }
    $utf8 = New-Object Text.UTF8Encoding($false)
    [IO.File]::AppendAllText($script:LogPath, "$($record.updated_at) $State $Message`r`n", $utf8)
    $bytes = $utf8.GetBytes(($record | ConvertTo-Json -Depth 5))
    $pending = "$ResultPath.new"
    $stream = [IO.File]::Open($pending, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try { $stream.Write($bytes, 0, $bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
    if (Test-Path -LiteralPath $ResultPath) { [IO.File]::Replace($pending, $ResultPath, [NullString]::Value) }
    else { [IO.File]::Move($pending, $ResultPath) }
}
function Assert-NotCancelled {
    if (Test-Path -LiteralPath (Join-Path (Split-Path $ResultPath) 'cancel')) { throw 'Update cancelled before replacement' }
}
function Get-MsiOutcome([int]$Code) {
    if ($Code -eq 0) { return 'succeeded' }
    if ($Code -in @(3010, 1641)) { return 'reboot_required' }
    return 'failed'
}
function Get-CurrentUserMsiProductState([string]$ProductCode) {
    # Query the exact per-user instance directly. ProductsEx exposes a COM
    # collection whose PowerShell enumeration is not reliably Product objects.
    if (-not ('NeoismMsiProductQuery' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Text;
public static class NeoismMsiProductQuery {
    [DllImport("msi.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    private static extern uint MsiGetProductInfoExW(string product, string sid,
        uint context, string property, StringBuilder value, ref uint length);
    public static int CurrentUserState(string product) {
        var value = new StringBuilder(16);
        uint length = (uint)value.Capacity;
        uint result = MsiGetProductInfoExW(product, null, 2, "State", value, ref length);
        if (result == 1605) return -1; // ERROR_UNKNOWN_PRODUCT
        if (result != 0) throw new Win32Exception((int)result, "Cannot query current-user MSI product state");
        return int.Parse(value.ToString(), CultureInfo.InvariantCulture);
    }
}
'@
    }
    return [NeoismMsiProductQuery]::CurrentUserState($ProductCode)
}
function Test-MsiProductInstalled([string]$Path) {
    # Read-only MSI metadata and current-user installation enumeration. Never
    # Win32_Product: querying that provider can trigger unrelated MSI repairs.
    $installer = $null; $database = $null; $view = $null; $record = $null
    try {
        $installer = New-Object -ComObject WindowsInstaller.Installer
        $database = $installer.OpenDatabase($Path, 0)
        $view = $database.OpenView('SELECT `Value` FROM `Property` WHERE `Property` = ''ProductCode''')
        $view.Execute()
        $record = $view.Fetch()
        if ($null -eq $record) { throw 'Candidate MSI has no ProductCode' }
        $code = $record.StringData(1)
        $guid = [Guid]::Empty
        if (-not [Guid]::TryParse($code, [ref]$guid)) { throw 'Candidate MSI ProductCode is invalid' }
        # MSIINSTALLCONTEXT_USERUNMANAGED, current user only; advertised is not installed.
        return (Get-CurrentUserMsiProductState $code) -eq 5
    } finally {
        if ($null -ne $view) { $view.Close() }
        foreach ($object in @($record, $view, $database, $installer)) {
            if ($null -ne $object -and [Runtime.InteropServices.Marshal]::IsComObject($object)) {
                [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($object)
            }
        }
    }
}
function Invoke-Msi([string[]]$Arguments) {
    $exe = Join-Path $env:SystemRoot 'System32\msiexec.exe'
    # MSI timeout is bounded, and a timeout is NEVER permission to relaunch.
    $process = Start-Process -FilePath $exe -ArgumentList $Arguments -WindowStyle Hidden -PassThru
    try {
        if (-not $process.WaitForExit(900000)) {
            # The MSI service may outlive its client. Poison subsequent updater
            # attempts rather than release our lock and start overlapping installs.
            $blocked = Join-Path $env:LOCALAPPDATA 'Neoism\updates\recovery-required.txt'
            [IO.File]::WriteAllText($blocked, "MSI client $($process.Id) timed out. Confirm Windows Installer has stopped and inspect $script:MsiLogPath before removing this recovery marker.")
            throw "Windows Installer timed out; it may still be running. Updates blocked until recovery: $blocked"
        }
        return $process.ExitCode
    } finally { $process.Dispose() }
}
function Get-TrackedProcess([int]$ProcessId, [string]$Exe) {
    if ($ProcessId -le 0) { return $null }
    $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if ($null -eq $process) { throw "Update owner $ProcessId exited before preflight" }
    if (-not $process.Path -or -not (Test-SamePath $process.Path $Exe)) { throw "Update owner $ProcessId is not $Exe" }
    $null = $process.Handle # retain the actual process handle, not a reusable PID
    return $process
}
function Wait-UpdateOwner($Process) {
    if ($null -ne $Process -and -not $Process.WaitForExit(120000)) {
        throw "Timed out waiting for Neoism process $($Process.Id) to close; nothing installed"
    }
}
function Stop-TargetStack([string]$Directory) {
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        $relevant = @()
        foreach ($process in Get-Process -Name 'neoism', 'neoism-workspace-daemon', 'neoism-agent' -ErrorAction SilentlyContinue) {
            # Inaccessible unrelated processes are not killed. Any target file
            # still locked is rejected by exclusive-open below (and by MSI).
            try { $path = $process.Path } catch { continue }
            if (-not $path) { continue }
            if (([IO.Path]::GetFileName($path) -iin $script:BinaryNames) -and
                (Test-SamePath ([IO.Path]::GetDirectoryName($path)) $Directory)) {
                $relevant += $process
                if (-not $process.HasExited) {
                    # Only exact target paths, no /IM, no /T, no LSP/browser kills.
                    $remaining = [int][Math]::Max(0, ($deadline - [DateTime]::UtcNow).TotalMilliseconds)
                    if ($remaining -eq 0) { throw 'Timed out quiescing the selected Neoism stack' }
                    $process.Kill()
                    if (-not $process.WaitForExit([Math]::Min(5000, $remaining))) { throw "Cannot stop target process $($process.Id)" }
                }
            }
        }
        if ($relevant.Count -eq 0) { break }
        if ([DateTime]::UtcNow -ge $deadline) { throw 'Target stack keeps respawning; stop its supervisor and retry' }
        Start-Sleep -Milliseconds 250
    } while ($true)
    foreach ($name in $script:BinaryNames) {
        $path = Join-Path $Directory $name
        if (Test-Path -LiteralPath $path) {
            $handle = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
            $handle.Dispose() # locked/inaccessible files throw, never ignored
        }
    }
}
function Install-PortablePayload([string]$Payload, [string]$Directory, [hashtable]$Manifest) {
    $id = [Guid]::NewGuid().ToString('N')
    $stage = Join-Path $Directory ".neoism-update-$id"
    $script:BackupDir = Join-Path $Directory ".neoism-rollback-$id"
    $names = @($script:BinaryNames) + @('web')
    $moved = @(); $installed = @()
    New-Item -ItemType Directory -Path $stage, $script:BackupDir | Out-Null
    try {
        foreach ($name in $names) {
            Copy-Item -LiteralPath (Join-Path $Payload $name) -Destination (Join-Path $stage $name) -Recurse -Force
        }
        Assert-InstalledPayload $stage $Manifest $ExpectedVersion
        Write-UpdateResult 'applying' 'Replacing the invoking portable stack in place; rollback retained'
        foreach ($name in $names) {
            $destination = Join-Path $Directory $name
            Assert-NoReparsePoint $destination
            if (Test-Path -LiteralPath $destination) {
                Move-Item -LiteralPath $destination -Destination (Join-Path $script:BackupDir $name)
                $moved += $name
            }
            Move-Item -LiteralPath (Join-Path $stage $name) -Destination $destination
            $installed += $name
        }
        Assert-InstalledPayload $Directory $Manifest $ExpectedVersion
    } catch {
        $originalError = $_.Exception.Message
        $rollbackErrors = @()
        foreach ($name in $installed) {
            try { Remove-Item -LiteralPath (Join-Path $Directory $name) -Recurse -Force }
            catch { $rollbackErrors += $_.Exception.Message }
        }
        foreach ($name in $moved) {
            try { Move-Item -LiteralPath (Join-Path $script:BackupDir $name) -Destination (Join-Path $Directory $name) }
            catch { $rollbackErrors += $_.Exception.Message }
        }
        throw "$originalError; rollback errors: $($rollbackErrors -join '; '); backups: $script:BackupDir"
    } finally {
        Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
    }
}
function Complete-VerifiedUpdate([string]$Directory, [hashtable]$Manifest) {
    Assert-InstalledPayload $Directory $Manifest $ExpectedVersion
    $script:InstallationVerified = $true
    if ($Relaunch -eq 1) {
        # Explicit verified path; neither PATH nor the old portable/MSI alternative.
        Start-Process -FilePath (Join-Path $Directory 'neoism.exe') -WorkingDirectory $Directory | Out-Null
    }
    Write-UpdateResult 'succeeded' 'All three installed binaries match the expected release version and payload hashes'
}
function Invoke-WindowsUpdate {
    $script:Target = $null; $script:MsiExitCode = $null; $script:InstallationVerified = $false
    $script:BackupDir = $null
    $script:LogPath = Join-Path (Split-Path $ResultPath) 'helper.log'
    $script:MsiLogPath = Join-Path (Split-Path $ResultPath) 'install-msi.log'
    $lock = $null; $updater = $null; $gui = $null
    try {
        $updater = Get-TrackedProcess $UpdaterPid $InvokingExe
        $gui = Get-TrackedProcess $GuiPid $InvokingExe
        $updatesDirectory = [IO.Directory]::CreateDirectory((Join-Path $env:LOCALAPPDATA 'Neoism\updates'))
        $lockPath = Join-Path $updatesDirectory.FullName 'update.lock'
        # FileShare.None serializes all this user's Neoism updates across sessions.
        try { $lock = [IO.File]::Open($lockPath, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None) }
        catch { throw "Another Neoism update is active or its lock cannot be opened at ${lockPath}: $($_.Exception.Message)" }
        $blocked = Join-Path $env:LOCALAPPDATA 'Neoism\updates\recovery-required.txt'
        if (Test-Path -LiteralPath $blocked) { throw "Previous MSI timed out; recovery required before retry: $blocked" }
        if ($ExpectedVersion -cnotmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$') { throw 'Invalid expected release version' }
        $script:Target = Resolve-UpdateTarget $InvokingExe (Get-RegisteredInstallDir)
        Assert-NoReparsePoint $script:Target.directory
        Assert-NoReparsePoint $InvokingExe
        if ($script:Target.mode -eq 'portable' -and (Test-Path -LiteralPath (Join-Path $script:Target.directory 'web'))) {
            foreach ($name in $script:BinaryNames) {
                if (-not (Test-Path -LiteralPath (Join-Path $script:Target.directory $name) -PathType Leaf)) {
                    throw 'Loose executable shares a directory with an unowned web folder. Move neoism.exe to its own portable folder and retry; no files were replaced.'
                }
            }
        }
        # Probe write access before asking a working GUI to close.
        $probe = Join-Path $script:Target.directory ('.neoism-write-probe-' + [Guid]::NewGuid().ToString('N'))
        [IO.File]::WriteAllText($probe, '')
        [IO.File]::Delete($probe)
        Write-UpdateResult 'preparing' "Verifying $($script:Target.mode) update for $($script:Target.directory)"
        $msiHash = (Get-FileHash -LiteralPath $MsiPath -Algorithm SHA256).Hash
        $extract = Join-Path $TempDir 'payload'
        New-Item -ItemType Directory -Path $extract | Out-Null
        $extractLog = Join-Path (Split-Path $ResultPath) 'extract-msi.log'
        $code = Invoke-Msi @('/a', ('"' + $MsiPath + '"'), '/qn', '/norestart', 'REBOOT=ReallySuppress', ('TARGETDIR="' + $extract + '"'), '/L*v', ('"' + $extractLog + '"'))
        if ($code -ne 0) { throw "MSI payload extraction failed (code $code); see $extractLog" }
        $roots = @(Get-ChildItem -LiteralPath $extract -Filter 'neoism.exe' -Recurse -File)
        if ($roots.Count -ne 1) { throw 'MSI must contain exactly one Neoism executable payload' }
        $payload = $roots[0].DirectoryName
        Assert-NoReparsePoint $payload
        Assert-StackVersion $payload $ExpectedVersion
        $manifest = Get-PayloadManifest $payload
        Assert-NotCancelled
        Write-UpdateResult 'handed_off' 'Payload verified; waiting for updater and GUI exit. Installation is NOT complete.'
        Wait-UpdateOwner $updater
        Wait-UpdateOwner $gui
        Assert-NotCancelled
        # Do not let registry changes during preflight redirect the update.
        $targetNow = Resolve-UpdateTarget $InvokingExe (Get-RegisteredInstallDir)
        if ($targetNow.mode -ne $script:Target.mode) { throw 'Installation registration changed during update; retry' }
        Stop-TargetStack $script:Target.directory
        if ((Get-FileHash -LiteralPath $MsiPath -Algorithm SHA256).Hash -ne $msiHash) { throw 'Staged MSI changed after verification' }
        if ($script:Target.mode -eq 'managed') {
            Write-UpdateResult 'applying' 'Windows Installer is upgrading the registered stack'
            # REINSTALLMODE alone does not select installed features for repair:
            # https://learn.microsoft.com/windows/win32/msi/reinstallmode
            $arguments = @('/i', ('"' + $MsiPath + '"'), '/qn', '/norestart', 'REBOOT=ReallySuppress', 'MSIRESTARTMANAGERCONTROL=Disable', ('INSTALLFOLDER="' + $script:Target.directory + '"'), '/L*v', ('"' + $script:MsiLogPath + '"'))
            if (Test-MsiProductInstalled $MsiPath) {
                # Re-cache this verified package and force every installed feature.
                $arguments += @('REINSTALL=ALL', 'REINSTALLMODE=vamus')
            } else {
                # A first install/new ProductCode (major upgrade) must select its
                # features normally, never REINSTALL=ALL or the re-cache 'v' flag.
                $arguments += 'REINSTALLMODE=amus'
            }
            $script:MsiExitCode = Invoke-Msi $arguments
            $outcome = Get-MsiOutcome $script:MsiExitCode
            if ($outcome -eq 'reboot_required') {
                Write-UpdateResult 'reboot_required' 'Windows Installer requires a reboot. No success or relaunch until replacement is verified after reboot.'
                return 2
            }
            if ($outcome -ne 'succeeded') { throw "Windows Installer failed (code $script:MsiExitCode); see $script:MsiLogPath" }
            $registered = Get-RegisteredInstallDir
            if (-not $registered -or -not (Test-SamePath $registered $script:Target.directory)) { throw 'MSI installed into an unexpected/unregistered directory; not relaunching' }
        } else {
            Install-PortablePayload $payload $script:Target.directory $manifest
        }
        Complete-VerifiedUpdate $script:Target.directory $manifest
        # Keep receipts, MSI logs and portable rollback. Only verified-success
        # downloads/extraction can be discarded; error evidence is retained.
        Remove-Item -LiteralPath $TempDir -Recurse -Force -ErrorAction SilentlyContinue
        return 0
    } catch {
        Write-UpdateResult 'failed' $_.Exception.Message
        return 1
    } finally {
        if ($null -ne $gui) { $gui.Dispose() }
        if ($null -ne $updater) { $updater.Dispose() }
        if ($null -ne $lock) { $lock.Dispose() }
    }
}
if (-not $LibraryOnly) { exit (Invoke-WindowsUpdate) }
