---
name: "NixOS Wayland: fixed flake and runtime launcher published"
description: "PUSHED main ae51c5e67: #neoism-runtime supplies tarball libraries without Rust rebuild; published target verified; full isolated nix-ld GUI/reload tests passed"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-17"
updated: "2026-10-17"
---

# NixOS Wayland: fixed flake and runtime launcher, PUBLISHED

Published to origin/main with explicit user permission: commit `ae51c5e67a5bec747b79c730b091ae7714dccfa7` (`fix(nix): supply release runtime and repair source packaging`). Only flake.nix, pkgNeoism.nix, README.md and .github/workflows/nix-build.yml committed. Unrelated memory files untouched/uncommitted. Remote main verified via git ls-remote; direct GitHub-main Nix evaluation resolves EXACT tested runtime derivation `/nix/store/6arj5b8d8zw57jlxzj4b08d0ah3ak8v7-neoism-runtime.drv`.

## User command

From extracted Linux release directory, with programs.nix-ld.enable=true and configured NixOS graphics drivers:

```sh
nix run --refresh github:parkers0405/neoism/main#neoism-runtime -- ./neoism
```

Runtime-only target fetches matching libraries and builds a tiny script, NOT Rust/Neoism. No new release tag or tarball rebuild required. Normal `nix run github:parkers0405/neoism/main` still builds the source package unless cached. Main push does not trigger release-neoism.yml; release runs only on version tag/manual dispatch.

## Error and workaround pitfall

NixOS tarball user reported WaylandError(Connection(NoWaylandLib)): missing Wayland client dlopen, before Vulkan. Plain ldd may falsely list dependencies missing because it doesn't honor nix-ld's NIX_LD_LIBRARY_PATH. Initial suggested `nix build --print-out-paths nixpkgs#wayland` was wrong: outputs both manual and library paths, captured newline breaks library path! Force #wayland.out for single-library workaround; runtime launcher is preferred.

## Production patch

- Linux packages.neoism-runtime uses source package runtimeDependencies; exports matching NIX_LD and prefixes NIX_LD_LIBRARY_PATH and LD_LIBRARY_PATH, then execs supplied binary. No-argument usage exits64. Keep helpers beside executable. Host GPU drivers not bundled.
- checks.runtime-libraries executes ctypes.CDLL for Wayland client/cursor/EGL, Vulkan, xkbcommon, GBM, D-Bus, fontconfig, stdc++, zlib, EGL, GL (12 libs).
- pkgNeoism: adds libgbm/dbus/zlib, glslang shader compiler; builds/tests four binaries (neoism, daemon, agent, lua-runner); source fileset includes optional interactive-artifacts manifest, assets/omarchy, README and docs/lua-api.lua; longDescription metadata path corrected.
- Existing autoPatchelfHook/runtimeDependencies already adds RPATH for dlopen! Do not claim no Nix RPATH mechanism. Ordinary Ubuntu release tarballs bypass that fixup.
- CI adds Nix/lock/source/assets paths, all-platform evaluation, real runtime test, native source build with --max-jobs 1 --cores 2. Old --build-max-jobs was valid alias, only redundant.
- README documents both routes and source package limitations. Existing source package feature policy unchanged (no-default-features plus x11/wayland); bundled release web/CEF assets remain absent.

## Verification

All-system no-build flake check passes four systems/toolchain variants. Executed x86_64-linux runtime flake check passes all12 libraries. Actual nix run local target with downloaded v0.7.127 --version works. Pinned Nix ELF loader resolves all direct dependencies of all4 release binaries.

Full GUI test PASSED with real nix-ld2.0.6 and actual launcher: changed ONLY test copies' ELF interpreters to nix-ld to emulate NixOS loading, mounted read-only Nix store in isolated headless Weston container, registered Nix Mesa26.0.2 under standard /run/opengl-driver. Nix Vulkan loader1.4.341 and LLVM21.1.8/Lavapipe initialize swapchain/application. Two fresh profiles plus relaunch pass, durations30.6/13.8/19.0sec, five config reloads total, both Bash PIDs survive, no SIGFPE/1x2/NoWaylandLib. No Node/npm/Bun/Cargo/rustc/nvim/Git on test PATH. Report `/tmp/neoism-nix-headless-final/report.json`, logs/traces nearby. Actual loaded graphics mappings confirmed Nix store paths.

Earlier Nix headless tests failed AFTER Wayland at Vulkan because Ubuntu test container lacked NixOS driver registration. Nix Vulkan loader SYSCONFDIR=/run/opengl-driver/share; adding Ubuntu manifest alone could not find its relative library. Correct fix was test Nix driver layout, NOT production Ubuntu /usr/lib fallback or bundled drivers. Failed logs retained /tmp/neoism-nix-headless-test and /tmp/neoism-nix-headless-driver-test.

No full Rust/source Nix package compilation or release rebuild was performed. No live user GUI touched. This was isolated GUI under Nix runtime, not a full NixOS installation or user's physical GPU: don't guarantee all hardware/driver configs. Nixpkgs pin b40629efe5d6ec48dd1efba650c797ddbd39ace0. No lockfile changes. Prefer Git-backed local flake refs; plain workspace path may snapshot live .tmp sockets.
