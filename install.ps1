# Neoism Windows installer bootstrap.
#
#   irm https://raw.githubusercontent.com/parkers0405/neoism/main/install.ps1 | iex
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Version v0.8.0
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Uninstall
#
# Downloads the per-user WiX MSI, requires its published SHA-256 checksum, and
# lets Windows Installer own upgrades, rollback, PATH, shortcuts, associations,
# and uninstall. PowerShell 5.1 compatible; no administrator rights required.
[CmdletBinding()]
param(
    [string]$Version = "latest",
    [string]$Repo = "parkers0405/neoism",
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"
$Asset = "Neoism-x86_64.msi"

function Say([string]$Message) {
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Invoke-Msi([string]$Arguments) {
    $process = Start-Process -FilePath "msiexec.exe" -ArgumentList $Arguments -Wait -PassThru
    if ($process.ExitCode -notin @(0, 3010, 1641)) {
        throw "Windows Installer exited with code $($process.ExitCode)"
    }
    return $process.ExitCode
}

if ($Uninstall) {
    $uninstallRoots = @(
        "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall",
        "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall"
    )
    $entry = $null
    foreach ($root in $uninstallRoots) {
        if (-not (Test-Path $root)) { continue }
        $entry = Get-ChildItem $root | Get-ItemProperty | Where-Object {
            $_.DisplayName -eq "Neoism" -and
            $_.Publisher -eq "Neoism contributors" -and
            $_.WindowsInstaller -eq 1 -and
            $_.PSChildName -match '^\{[0-9A-Fa-f-]{36}\}$'
        } | Select-Object -First 1
        if ($null -ne $entry) { break }
    }
    if ($null -eq $entry) {
        Say "Neoism is not installed."
        return
    }
    Say "Uninstalling Neoism"
    $exitCode = Invoke-Msi "/x $($entry.PSChildName) /passive /norestart"
    Say "Neoism uninstalled. Configuration and user data were preserved."
    if ($exitCode -in @(3010, 1641)) { Say "Windows requested a restart to finish cleanup." }
    return
}

[Net.ServicePointManager]::SecurityProtocol =
    [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

if ($Version -eq "latest") {
    $baseUrl = "https://github.com/$Repo/releases/latest/download"
} else {
    $baseUrl = "https://github.com/$Repo/releases/download/$Version"
}

$tempDir = Join-Path $env:TEMP ("neoism-install-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tempDir -Force | Out-Null

try {
    $msiPath = Join-Path $tempDir $Asset
    $shaPath = "$msiPath.sha256"
    Say "Downloading Neoism ($Version)"
    Invoke-WebRequest -UseBasicParsing -Uri "$baseUrl/$Asset" -OutFile $msiPath
    Invoke-WebRequest -UseBasicParsing -Uri "$baseUrl/$Asset.sha256" -OutFile $shaPath

    $expected = ((Get-Content -Path $shaPath -Raw).Trim() -split "\s+")[0].ToLower()
    $actual = (Get-FileHash -Path $msiPath -Algorithm SHA256).Hash.ToLower()
    if ($expected -notmatch '^[0-9a-f]{64}$' -or $actual -ne $expected) {
        throw "checksum mismatch for ${Asset}: expected $expected, got $actual"
    }
    Say "Checksum verified"

    $signature = Get-AuthenticodeSignature -FilePath $msiPath
    if ($signature.Status -eq "Valid") {
        Say "Publisher signature verified: $($signature.SignerCertificate.Subject)"
    } elseif ($signature.Status -ne "NotSigned") {
        throw "invalid installer signature: $($signature.Status)"
    } else {
        Write-Warning "This release is not Authenticode-signed; verification is limited to its published checksum."
    }

    Say "Installing Neoism for the current user"
    $exitCode = Invoke-Msi "/i `"$msiPath`" /passive /norestart"
    Say "Neoism installed in $env:LOCALAPPDATA\Programs\Neoism"
    Write-Host "Launch Neoism from the Start Menu or open a new terminal and run: neoism"
    if ($exitCode -in @(3010, 1641)) { Say "Windows requested a restart to finish installation." }
} finally {
    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}