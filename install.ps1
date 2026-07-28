# Neoism Windows installer - downloads the prebuilt stack from GitHub Releases.
#
#   irm https://raw.githubusercontent.com/parkers0405/neoism/main/install.ps1 | iex
#   powershell -ExecutionPolicy Bypass -File install.ps1                # latest release
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Version v0.7.6
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Uninstall
#
# Windows counterpart of scripts/install.sh: fetches neoism-windows-x86_64.zip
# from the repo's GitHub Releases, verifies its .sha256 when present, installs
# the exes to %LOCALAPPDATA%\Programs\Neoism, adds that dir to the user Path,
# creates a Start Menu shortcut, and registers file associations (Open With /
# Settings -> Default Apps) for common code and text files. No admin required.
# Re-run any time to update (idempotent). PowerShell 5.1 compatible.
#
# The rest of the user-facing setup (Start Menu refresh on relocation, Win+R
# `neoism` App Paths registration, default config) is handled by the app's
# first-run bootstrap on launch - see neoism-frontend/desktop/src/bootstrap.rs.
[CmdletBinding()]
param(
    [string]$Version = "latest",
    [string]$Repo = "parkers0405/neoism",
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"

$Asset = "neoism-windows-x86_64.zip"
$InnerDir = "neoism-windows-x86_64"
$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\Neoism"
$ShortcutPath = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Neoism.lnk"
$AppPathsKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\neoism.exe"

# File-association registry layout (same shape the app's first-run bootstrap
# maintains - keep the extension list in sync with bootstrap.rs).
$DocProgId = "Neoism.Document"
$ClassesKey = "HKCU:\Software\Classes\Neoism.Document"
$VendorKey = "HKCU:\Software\Neoism"
$CapabilitiesKey = "HKCU:\Software\Neoism\Capabilities"
$RegisteredAppsKey = "HKCU:\Software\RegisteredApplications"
$AssocExtensions = @(
    ".md", ".markdown", ".json", ".jsonc", ".toml", ".yaml", ".yml", ".rs",
    ".py", ".js", ".jsx", ".ts", ".tsx", ".go", ".c", ".h", ".cpp", ".hpp",
    ".cs", ".java", ".rb", ".php", ".sh", ".ps1", ".lua", ".html", ".css",
    ".scss", ".sql", ".txt", ".log", ".ipynb", ".neodraw"
)

function Say([string]$Message) { Write-Host "==> $Message" -ForegroundColor Cyan }
function Warn([string]$Message) { Write-Host "warn: $Message" -ForegroundColor Yellow }

# --- user Path helpers (registry-backed, preserve REG_EXPAND_SZ) ----------

function Get-UserPathRaw {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Environment")
    if ($null -eq $key) { return "" }
    try {
        $options = [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
        return [string]$key.GetValue("Path", "", $options)
    } finally {
        $key.Close()
    }
}

function Set-UserPathRaw([string]$Value) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Environment")
    try {
        $key.SetValue("Path", $Value, [Microsoft.Win32.RegistryValueKind]::ExpandString)
    } finally {
        $key.Close()
    }
    Publish-EnvironmentChange
}

function Test-UserPathContains([string]$Dir) {
    foreach ($entry in ((Get-UserPathRaw) -split ";")) {
        if ($entry.Trim().TrimEnd("\") -ieq $Dir.TrimEnd("\")) { return $true }
    }
    return $false
}

# Broadcast WM_SETTINGCHANGE so newly opened terminals see the Path change
# without a re-login. Best effort - the registry holds the change either way.
function Publish-EnvironmentChange {
    try {
        $signature = '[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)] public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);'
        $native = Add-Type -MemberDefinition $signature -Name "EnvBroadcast" -Namespace "NeoismInstaller" -PassThru
        $result = [UIntPtr]::Zero
        # HWND_BROADCAST, WM_SETTINGCHANGE, SMTO_ABORTIFHUNG, 5s timeout
        [void]$native::SendMessageTimeout([IntPtr]0xFFFF, 0x001A, [UIntPtr]::Zero, "Environment", 2, 5000, [ref]$result)
    } catch {
        # Non-fatal.
    }
}

function Install-StartMenuShortcut([string]$TargetExe) {
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($ShortcutPath)
    $shortcut.TargetPath = $TargetExe
    $shortcut.WorkingDirectory = $env:USERPROFILE
    $shortcut.Save()
}

# Compare-before-set so re-runs are no-ops; handles "(default)" values too.
function Set-RegistryValue([string]$Path, [string]$Name, [string]$Value) {
    if (-not (Test-Path $Path)) { New-Item -Path $Path -Force | Out-Null }
    $query = $Name
    if ($Name -eq "(default)") { $query = "" }
    if (((Get-Item $Path).GetValue($query)) -ne $Value) {
        Set-ItemProperty -Path $Path -Name $Name -Value $Value
    }
}

# Registers Neoism for Open With / Settings -> Default Apps. UserChoice
# per-extension defaults are deliberately not touched (Windows protects them
# with a hash); the Capabilities registration is what makes Neoism pickable.
function Install-FileAssociations([string]$TargetExe) {
    Set-RegistryValue $ClassesKey "(default)" "Neoism Document"
    Set-RegistryValue (Join-Path $ClassesKey "DefaultIcon") "(default)" ($TargetExe + ",0")
    Set-RegistryValue (Join-Path $ClassesKey "shell\open\command") "(default)" ('"' + $TargetExe + '" "%1"')
    Set-RegistryValue $CapabilitiesKey "ApplicationName" "Neoism"
    Set-RegistryValue $CapabilitiesKey "ApplicationDescription" "Terminal, code editor, and notes workspace"
    foreach ($ext in $AssocExtensions) {
        Set-RegistryValue (Join-Path $CapabilitiesKey "FileAssociations") $ext $DocProgId
    }
    Set-RegistryValue $RegisteredAppsKey "Neoism" "Software\Neoism\Capabilities"
}

# --- uninstall ------------------------------------------------------------

if ($Uninstall) {
    Say "Uninstalling Neoism"
    if (Test-Path $InstallDir) {
        try {
            Remove-Item -Path $InstallDir -Recurse -Force
        } catch {
            throw "could not remove $InstallDir (close any running Neoism windows and re-run): $($_.Exception.Message)"
        }
        Say "removed $InstallDir"
    } else {
        Warn "$InstallDir not present; nothing to remove there"
    }
    if (Test-Path $ShortcutPath) {
        Remove-Item -Path $ShortcutPath -Force
        Say "removed Start Menu shortcut"
    }
    # The app's first-run bootstrap registers App Paths; clean that up too.
    if (Test-Path $AppPathsKey) {
        Remove-Item -Path $AppPathsKey -Recurse -Force
        Say "removed Win+R App Paths registration"
    }
    if (Test-Path $ClassesKey) {
        Remove-Item -Path $ClassesKey -Recurse -Force
        Say "removed $DocProgId file class"
    }
    if (Test-Path $VendorKey) {
        Remove-Item -Path $VendorKey -Recurse -Force
        Say "removed Default Apps capabilities"
    }
    $registered = Get-ItemProperty -Path $RegisteredAppsKey -Name "Neoism" -ErrorAction SilentlyContinue
    if ($null -ne $registered) {
        Remove-ItemProperty -Path $RegisteredAppsKey -Name "Neoism" -ErrorAction SilentlyContinue
        Say "removed RegisteredApplications entry"
    }
    if (Test-UserPathContains $InstallDir) {
        $kept = @()
        foreach ($entry in ((Get-UserPathRaw) -split ";")) {
            if ($entry -eq "") { continue }
            if ($entry.Trim().TrimEnd("\") -ieq $InstallDir.TrimEnd("\")) { continue }
            $kept += $entry
        }
        Set-UserPathRaw ($kept -join ";")
        Say "removed $InstallDir from the user Path"
    }
    Say "Done. Neoism has been uninstalled."
    return
}

# --- install --------------------------------------------------------------

# PowerShell 5.1 defaults can lack TLS 1.2, which GitHub requires.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

if ($Version -eq "latest") {
    $BaseUrl = "https://github.com/$Repo/releases/latest/download"
} else {
    $BaseUrl = "https://github.com/$Repo/releases/download/$Version"
}

Say "Installing Neoism (windows/x86_64) from $Repo ($Version)"

$TempDir = Join-Path $env:TEMP ("neoism-install-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

try {
    $ZipPath = Join-Path $TempDir $Asset
    Say "Downloading $Asset"
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$BaseUrl/$Asset" -OutFile $ZipPath
    } catch {
        throw "download failed - is there a release with ${Asset}? (try -Version vX.Y.Z): $($_.Exception.Message)"
    }

    # checksum verification - releases ship a per-asset .sha256 file
    $Expected = $null
    try {
        $ShaPath = Join-Path $TempDir "$Asset.sha256"
        Invoke-WebRequest -UseBasicParsing -Uri "$BaseUrl/$Asset.sha256" -OutFile $ShaPath
        $Expected = ((Get-Content -Path $ShaPath -Raw).Trim() -split "\s+")[0].ToLower()
    } catch {
        Say "checksum not verified (.sha256 asset unavailable)"
    }
    if ($Expected) {
        $Actual = (Get-FileHash -Path $ZipPath -Algorithm SHA256).Hash.ToLower()
        if ($Actual -ne $Expected) {
            throw "checksum mismatch for ${Asset}: expected $Expected, got $Actual - aborting"
        }
        Say "checksum OK"
    }

    Say "Extracting $Asset"
    $ExtractDir = Join-Path $TempDir "extract"
    Expand-Archive -Path $ZipPath -DestinationPath $ExtractDir -Force

    # The zip contains an inner neoism-windows-x86_64/ dir holding the exes.
    $SourceDir = Join-Path $ExtractDir $InnerDir
    if (-not (Test-Path (Join-Path $SourceDir "neoism.exe"))) {
        $Found = Get-ChildItem -Path $ExtractDir -Recurse -Filter "neoism.exe" | Select-Object -First 1
        if ($null -eq $Found) { throw "neoism.exe not found in $Asset" }
        $SourceDir = $Found.DirectoryName
    }

    Say "Installing to $InstallDir"
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    try {
        Copy-Item -Path (Join-Path $SourceDir "*.exe") -Destination $InstallDir -Force
    } catch {
        throw "could not copy binaries into $InstallDir (close any running Neoism windows and re-run): $($_.Exception.Message)"
    }
    Get-ChildItem -Path $InstallDir -Filter "*.exe" | ForEach-Object {
        Write-Host ("   " + $_.FullName)
    }

    if (Test-UserPathContains $InstallDir) {
        Say "user Path already contains $InstallDir"
    } else {
        $Raw = Get-UserPathRaw
        if ($Raw -eq "") {
            $NewPath = $InstallDir
        } else {
            $NewPath = $Raw.TrimEnd(";") + ";" + $InstallDir
        }
        Set-UserPathRaw $NewPath
        Say "added $InstallDir to the user Path"
    }

    Say "Creating Start Menu shortcut"
    Install-StartMenuShortcut (Join-Path $InstallDir "neoism.exe")

    Say "Registering file associations (Open With / Default Apps)"
    Install-FileAssociations (Join-Path $InstallDir "neoism.exe")

    Say "Done. Neoism ($Version) installed."
    Write-Host ""
    Write-Host "Launch it from the Start Menu (Neoism), or open a new terminal and run:"
    Write-Host "  neoism"
    Write-Host ""
    Write-Host "First launch finishes setup automatically (default config, Win+R 'neoism'"
    Write-Host "registration). Update later by re-running this installer."
} finally {
    Remove-Item -Path $TempDir -Recurse -Force -ErrorAction SilentlyContinue
}
