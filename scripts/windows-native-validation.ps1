# Native runner only; inherits the exact production release-profile overrides.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/windows-validation-process.ps1"
$evidence = (New-Item -ItemType Directory -Force 'windows-validation').FullName
Start-Transcript -Path "$evidence/native-transcript.log"
$oldCI = $env:CI
try {
    # Same fff-search workaround as the production build; no compiler/linker overrides.
    Remove-Item Env:CI -ErrorAction SilentlyContinue
    Get-ChildItem Env: | Where-Object Name -Match '^(CARGO_PROFILE_RELEASE_|RUSTFLAGS$|RUSTC_WRAPPER$)' |
        Format-Table -AutoSize | Out-String | Set-Content "$evidence/build-environment.txt"
    Invoke-CheckedProcess (Get-Command cmd.exe).Source '/d /c ver' "$evidence/cmd" 30
    Invoke-CheckedProcess (Get-Command powershell.exe).Source `
        '-NoLogo -NoProfile -Command "$PSVersionTable; if ($PSVersionTable.PSVersion.Major -ne 5) { exit 1 }"' "$evidence/windows-powershell" 30
    $env:NEOISM_TEST_PWSH = (Get-Command pwsh.exe).Source
    Invoke-CheckedProcess $env:NEOISM_TEST_PWSH `
        '-NoLogo -NoProfile -Command "$PSVersionTable; if ($PSVersionTable.PSVersion.Major -ne 7) { exit 1 }"' "$evidence/pwsh" 30
    Get-Command cmd.exe, powershell.exe, pwsh.exe | Select-Object Name, Source, Version |
        ConvertTo-Json | Set-Content "$evidence/shells.json"
    # Exercise normal production profile loading on the isolated hosted runner.
    $env:NEOISM_TEST_POWERSHELL_DEFAULT_PROFILE = '1'
    $common = 'test --profile release --locked --target x86_64-pc-windows-msvc'
    # All targets: includes helper tests, resize and process-wide handle-count probes.
    # Do not filter failures away on native Windows (Wine limitations do not apply).
    $failed = @()
    foreach ($package in @('teletypewriter', 'neoism-terminal-pty', 'neoism-ui')) {
        $args = "$common -p $package"
        if ($package -eq 'neoism-ui') {
            $args += ' --features sugarloaf/wgpu --test windows_composer -- --ignored --test-threads=1 --nocapture'
        } else {
            $args += ' -- --include-ignored --test-threads=1 --nocapture'
        }
        try { Invoke-CheckedProcess 'cargo.exe' $args "$evidence/$package" 1800 }
        catch { $failed += "${package}: $_"; Write-Warning $_ }
    }
    if ($failed.Count) { throw ($failed -join "`n") }
    'Native release-profile tests passed; NOT GUI keyboard/GPU acceptance.' | Set-Content "$evidence/native-passed.txt"
} finally {
    $env:CI = $oldCI
    Stop-Transcript
}
