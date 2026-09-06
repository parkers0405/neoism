$ErrorActionPreference = "Stop"
. "$PSScriptRoot/windows-validation-process.ps1"
$evidence = (New-Item -ItemType Directory -Force 'windows-validation').FullName
Start-Transcript -Path "$evidence/msi-transcript.log"
$script:msiStep = 0
function Invoke-Msi([string]$arguments) {
    $script:msiStep++
    Invoke-CheckedProcess 'msiexec.exe' "$arguments /L*v `"$evidence/msi-$script:msiStep.log`"" `
        "$evidence/msiexec-$script:msiStep" 180 @(0, 3010, 1641)
}
$fixture = (Resolve-Path "Neoism-upgrade-fixture.msi").Path
$installer = (Resolve-Path "Neoism-x86_64.msi").Path
$installDir = Join-Path $env:LOCALAPPDATA "Programs\Neoism"
$userData = Join-Path $env:LOCALAPPDATA "neoism\installer-preserve-test"
New-Item -ItemType Directory -Force (Split-Path $userData) | Out-Null
Set-Content $userData "preserve"

$upgraded = $false
try {
Invoke-Msi "/i `"$fixture`" /qn /norestart"
$fixtureHash = (Get-FileHash (Join-Path $installDir "neoism.exe") -Algorithm SHA256).Hash
Invoke-Msi "/i `"$installer`" /qn /norestart"
$upgraded = $true
$packagingDir = Join-Path $PWD 'target/x86_64-pc-windows-msvc/release'
foreach ($binary in @('neoism.exe', 'neoism-workspace-daemon.exe', 'neoism-agent.exe')) {
    $sourceHash = (Get-FileHash (Join-Path $packagingDir $binary) -Algorithm SHA256).Hash
    $installedHash = (Get-FileHash (Join-Path $installDir $binary) -Algorithm SHA256).Hash
    $signature = Get-AuthenticodeSignature (Join-Path $installDir $binary)
    [pscustomobject]@{ Binary = $binary; PackagingSHA256 = $sourceHash; InstalledSHA256 = $installedHash;
        Signature = "$($signature.Status)"; Signer = "$($signature.SignerCertificate.Thumbprint)" } |
        ConvertTo-Json -Compress | Add-Content "$evidence/installed-binaries.jsonl"
    if ($sourceHash -ne $installedHash) { throw "$binary differs from the packaging input" }
    if ($env:HAVE_WINDOWS_SIGNING -eq 'true' -and $signature.Status -ne 'Valid') {
        throw "$binary installed signature is not valid"
    }
}

foreach ($binary in @("neoism.exe", "neoism-workspace-daemon.exe", "neoism-agent.exe")) {
  if (-not (Test-Path (Join-Path $installDir $binary))) { throw "$binary was not installed" }
}
if (-not (Test-Path (Join-Path $installDir "web\index.html"))) { throw "web UI was not installed" }
Invoke-CheckedProcess (Join-Path $installDir 'neoism.exe') '--version' "$evidence/version" 30
$version = (Get-Content "$evidence/version.stdout.log" | Out-String).Trim()
if ($version -notmatch "neoism") { throw "installed executable did not report its version" }
$appPath = (Get-Item "HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\neoism.exe").GetValue('')
if ($appPath -ne (Join-Path $installDir "neoism.exe")) { throw "App Paths registration is wrong" }
$registeredVersion = (Get-ItemProperty "HKCU:\Software\Neoism").InstalledVersion
$expectedVersion = ((cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages |
  Where-Object { $_.name -eq "neoism" }).version
if ($registeredVersion -ne $expectedVersion) { throw "MSI major upgrade did not replace the fixture" }
$installedHash = (Get-FileHash (Join-Path $installDir "neoism.exe") -Algorithm SHA256).Hash
if ($installedHash -eq $fixtureHash) { throw "MSI major upgrade did not replace neoism.exe" }
$shortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Neoism\Neoism.lnk"
if (-not (Test-Path $shortcut)) { throw "Start Menu shortcut was not installed" }
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($userPath -split ';').TrimEnd('\') -notcontains $installDir.TrimEnd('\')) {
  throw "Neoism install directory was not added to the user PATH"
}

& "$PSScriptRoot/windows-installed-gui-smoke.ps1" -InstallDir $installDir -Evidence $evidence
} finally {
    try {
        $remove = if ($upgraded) { $installer } else { $fixture }
        Invoke-Msi "/x `"$remove`" /qn /norestart"
    } finally { Stop-Transcript }
}
if (Test-Path (Join-Path $installDir "neoism.exe")) { throw "Neoism remained after uninstall" }
if (Test-Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\neoism.exe") { throw "App Paths registration remained after uninstall" }
if (Test-Path $shortcut) { throw "Start Menu shortcut remained after uninstall" }
if (Test-Path "HKCU:\Software\Classes\Directory\Background\shell\Open Neoism here") { throw "context menu remained after uninstall" }
if (Test-Path "HKCU:\Software\Neoism\Capabilities") { throw "Default Apps capabilities remained after uninstall" }
$registeredApp = Get-ItemProperty "HKCU:\Software\RegisteredApplications" -Name "Neoism" -ErrorAction SilentlyContinue
if ($null -ne $registeredApp) { throw "RegisteredApplications entry remained after uninstall" }
$userPathAfter = [Environment]::GetEnvironmentVariable("Path", "User")
if (($userPathAfter -split ';').TrimEnd('\') -contains $installDir.TrimEnd('\')) {
  throw "Neoism install directory remained on the user PATH after uninstall"
}
if (-not (Test-Path $userData)) { throw "uninstall removed Neoism user data" }
Remove-Item $userData -Force
