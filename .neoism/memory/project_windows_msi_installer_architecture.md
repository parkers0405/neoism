---
name: "Windows MSI installer architecture"
description: "Windows now uses a per-user WiX MSI with installer-owned integration and MSI self-update"
type: "project"
scope: "project"
origin: "2026-07-09 session"
created: "2026-08-17"
updated: "2026-08-17"
---

## Decision

Windows distribution is a per-user WiX v4 MSI, not an ambiguous ZIP/self-install hybrid. Canonical install path: `%LOCALAPPDATA%\Programs\Neoism`. The MSI owns all three executables, user PATH, Start Menu, App Paths, file associations/capabilities, context menu, major upgrades, repair, rollback, and uninstall. AppData is preserved on uninstall.

## Source

- `misc/windows/neoism.wxs`: WiX v4 per-user package with stable UpgradeCode/components and `MajorUpgrade Schedule="afterInstallInitialize"`.
- `.github/workflows/release-neoism.yml`: builds `Neoism-x86_64.msi`, tests synthetic 0.0.1 -> current major upgrade with actual binary replacement, validates integration/uninstall/data preservation, publishes MSI + SHA-256. Optional Authenticode signing uses `WINDOWS_SIGN_PFX` and `WINDOWS_SIGN_PFX_PASSWORD` secrets.
- `.github/workflows/build-mac-win.yml`: MSI build/install/uninstall validation.
- `install.ps1`: thin MSI downloader/checksum verifier/msiexec bootstrap; no longer copies binaries or owns registry state.
- `neoism-frontend/desktop/src/bootstrap.rs`: Windows silent self-install removed; bootstrap is Unix-only.
- `neoism-frontend/desktop/src/main.rs`: Windows `neoism update` downloads/checks MSI, launches detached PowerShell, waits for updater exit, force-closes every Neoism GUI/daemon/agent process, then runs passive transactional `msiexec`. Linux/macOS paths unchanged.

## Verification constraints

Linux-local checks can validate Rust, YAML, XML, and WiX parsing only. WiX explicitly supports binding on Windows only; MSVC cross-check needs `lib.exe`. The workflows are the authoritative native Windows compile/install/upgrade/uninstall gates.
