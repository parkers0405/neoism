---
name: "v0.7.100 Windows cleanup + Docker Fontconfig — FIXED v0.7.101"
description: "v0.7.101 published green: Docker missing fontconfig runtime; Windows live Process.Path double-read cleanup race; tests + native MSI and image health passed"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-14"
updated: "2026-09-14"
---

v0.7.100 d58bdc276317889dbd0e876d17985315df39bd78 failed in two distinct stages (actual logs inspected): Docker run 34796746545 compiled successfully then `neoism-workspace-daemon --version` failed loader exit 127 missing libfontconfig.so.1 (NOT libgbm); Windows release 34796746540 compiled, passed native terminal and GUI (gui-passed.txt), built MSI, then cleanup in windows-installed-gui-smoke.ps1 line 145 failed because live Process.Path getter returned null on second read after process exit.
Fix commit 87395dea9: Dockerfile adds runtime libfontconfig1 (FreeType transitive); scripts/windows-validation-process.ps1 Test-InstalledProcess snapshots Path once, handles inaccessible getter, compares separator-bounded install prefix. GUI smoke uses helper. New scripts/windows-validation-process-tests.ps1 covers getter race/null/throw/case/sibling/trailing separator; release workflow runs it in Windows PowerShell 5.1.
Released v0.7.101 commit aa6e053f18d734826cf273c0ffb745d90c78c374. main/tag/image revision exact match; old v0.7.100 untouched. App run 34803768506 SUCCESS all platforms + publish, public latest verified 2026-09-14 05:35Z. Actual assets Linux tarball, macOS tarball+DMG, Windows Neoism-x86_64.msi + checksums (not Windows zip). Docker run 34803768443 SUCCESS published 0.7.101/latest digest sha256:5420df71e0b06aa3de03308344b21a39ce8e6934935e39c3bafa582d9bb633b2. Fresh published image pull versions and non-root daemon /health passed. Diagnostic existing original MSI run 34803768211 also SUCCESS with corrected smoke (no retag/republication of old binary). SDK unchanged/manual workflow not dispatched. Workspace version bump only 32 local Cargo.lock packages. No local release build. Windows job took 1h47m46s; compilation finishing isn't full release completion—must wait MSI smoke and publish gate.
