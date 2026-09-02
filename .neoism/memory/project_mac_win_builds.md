---
name: project-mac-win-builds
description: "macOS ARM ships in releases; Windows compiles (guard only, unpublished, never runtime-tested) — the 8-attempt fix list and re-enable path"
metadata: 
  node_type: memory
  type: project
  originSessionId: c6967401-4270-436e-9d09-bcdf40124a24
---

As of 2026-07-02: **darwin-aarch64 ships in releases** (first in v0.4.3; single blocker was macOS-only `create_native_tab` constructing `Route` with a struct literal after it gained a private `redraw_request` field — Linux CI never compiled that cfg path). **Windows x86_64 compiles but is NOT published** — never run on a real machine; the daemon wiring is different on Windows (expects standalone loopback daemon ws://127.0.0.1:7878, never wired), parser bundle would need .dll builds, nvim/editor panes unproven.

**Windows fix inventory (8 CI attempts):** glslang via Khronos main-tot release download (choco package dead) + `GLSLANG_VALIDATOR` env + build.rs `which()` .exe handling; fff-search half-gated cfg (OnceLock/MMAP_THRESHOLD imports + constants); teletypewriter `STILL_ACTIVE` moved to `Foundation` in windows-sys 0.59 (i32 cast); `misc/windows/neoism.ico` regenerated (rio.ico deleted in rebrand, neoism-window build.rs hard-fails without it); teletypewriter `create_pty_with_spawn` alias over ConPTY `create_pty`; agent-cli `chat_terminal.rs` platform-split (unix termios impl vs Windows stubs); desktop trio (shell_detect stub i32 fd, `explicit_daemon_url` windows decl, `shell_pid` 0-sentinel in panes.rs).

**How to apply:** `.github/workflows/build-mac-win.yml` = dispatch-only compile guard (`-f only=windows|darwin|all`); iterate Windows there, never per-push (2x billing). Local iteration shortcut: `cargo check --target x86_64-pc-windows-msvc -p <crate>` works for pure-Rust crates (ring/libgit2 build scripts can't cross-compile). Re-enable Windows releases by uncommenting the matrix entry in release-neoism.yml (glslang step already ported) — only after runtime validation on real Windows. Related: [[project-ci-build-stack]], [[feature-first-run-bootstrap]].

**v0.7.0 released 2026-07-16:** user committed the feature wave themselves (a312f941 "pls"); I bumped workspace 0.6.9->0.7.0 (NOTE: internal path-dep pins in root Cargo.toml `version = "0.6.5"` must bump when crossing 0.7 — semver req <0.7.0 stops matching; sed all 16 pins), release: v0.7.0 commit, tag pushed -> release-neoism.yml (linux x86_64 + darwin aarch64, publish job undrafts when all legs upload) + docker-publish.yml (NEW: ghcr.io/parkers0405/neoism-daemon on v* tags, buildx+gha cache) + emoji release notes set via gh release edit on a pre-created draft (watch for softprops duplicate-draft edge). Agent-input chips fixed by subagent: chevron f078 clipped by measured-advance-narrower-than-ink clip (draw.rs base_clip +2px ink_slack); chip click toggles picker via NeoismAgentPicker.kind match.
