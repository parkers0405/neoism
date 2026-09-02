---
name: project-ci-build-stack
description: "build-stack.yml CI facts — release profile OOMs 16GB GitHub runners, override CARGO_PROFILE_RELEASE_* env; env -u CI for fff-search"
metadata: 
  node_type: memory
  type: project
  originSessionId: c6967401-4270-436e-9d09-bcdf40124a24
  modified: 2026-08-27T05:54:27.514Z
---

`.github/workflows/build-stack.yml` (added 2026-07-01) builds everything install.sh builds, Linux only: cargo release build of neoism + neoism-workspace-daemon + neoism-agent, then npm ci + `npm run build` (which runs wasm-pack itself via scripts/build-wasm.sh). Run 2 green with 54MB binaries + 2MB web-dist artifacts.

**Why:** The workspace `[profile.release]` (lto=true, debug="full", codegen-units=1) OOM-kills 16GB hosted runners (exit 143/SIGTERM) while compiling the final `neoism` desktop crate — works locally only because the user's machine has 30GB. Local target/release is ~28GB, so runner disk is also tight with full debuginfo.

**How to apply:** Any CI job that release-builds the desktop crate on hosted runners must override the heavy codegen via env: `CARGO_PROFILE_RELEASE_DEBUG: "0"`, `CARGO_PROFILE_RELEASE_LTO: "false"`, `CARGO_PROFILE_RELEASE_CODEGEN_UNITS: "16"`. Also keep `env -u CI` on cargo build steps (fff-search's build script demands Zig when CI is set — same trick as release-neoism.yml). Nix Build and Test workflows were already failing on every commit before this (pre-existing; e.g. Test/macos fails on the sugarloaf test passing the `(FontLibrary, Option<SugarloafErrors>)` tuple from `FontLibrary::new()` to `Text::new()` on the macOS cfg path).

**Actions quota (private repo):** Windows bills 2x, macOS 10x — the old per-push cross-platform Test matrix drained the monthly quota and made release jobs fail INSTANTLY with 0 steps + annotation "spending limit needs to be increased" (looks like a mystery failure; check job annotations, not logs). Test now runs ubuntu-only per push, full matrix via workflow_dispatch. If a release run hits the wall: fix billing at github.com/settings/billing/spending_limit then `gh run rerun <id>` — the tag/version are already pushed, nothing is lost.

## Cutting a release (learned 2026-07-08, v0.6.3)
Release = push a `v*.*.*` tag → `.github/workflows/release-neoism.yml` builds Linux/macOS/Windows and uploads to a GitHub Release (draft that publishes once all legs land). The legacy `release.yml` + `.goreleaser.yaml` are Rio-inherited and DISABLED (manual dispatch only) — ignore them. Steps that match the repo's "release: vX.Y.Z" commit convention: (1) commit feature work; (2) bump the version in root `Cargo.toml` — `[workspace.package] version` + every internal `{ path = ..., version = "X" }` dep — but PRESERVE external pins that coincidentally match (notably `raw-window-handle = { version = "0.6.2" }`, which is NOT ours). Safe sed: `sed -i '/path =/ s/version="OLD"/version="NEW"/'` + `sed -i 's/^version = "OLD"$/version = "NEW"/'`; (3) `cargo metadata --format-version 1 >/dev/null` regenerates Cargo.lock with no compile; (4) commit `release: vX.Y.Z`; (5) push main (branch-protection PR rule is admin-bypassed on push); (6) `git tag -a vX.Y.Z -m vX.Y.Z && git push origin vX.Y.Z`. The `Test` + `Nix Build` workflows fail on pre-existing-broken targets — NOT release blockers; `Release Neoism` builds artifacts independently.

## v0.7.56–59 release-week lessons (2026-08-27)
- **Release notes are now automatic**: the publish job extracts the tag's `## [X.Y.Z]` section from CHANGELOG.md and sets it as the release body (`gh release edit --notes-file`) before undrafting. Notes live in the changelog — write them there.
- **cfg(windows) mut-binding class**: code inside `#[cfg(windows)]` blocks mutating non-mut bindings compiles fine on Linux (cfg'd-out blocks are parsed, not type-checked) and dies only on the Windows leg 40 min in. Hit TWICE: agent-server `ShellKind::Posix` gated variant + `pty.rs` filter_map arity (v0.7.57), then daemon `hidden_std_command`/`manager.rs` canonicalize loops (v0.7.58). Audit recipe before tagging: `git diff <last-good-windows-tag>..HEAD` filtered to files containing cfg(windows), check every touched gated region; agent-server compiling in a failed run proves those files. Cross-compile check from Linux still blocked (ring/zstd).
- **macOS DMG disk**: hdiutil ran out of disk after the 44-min build (v0.7.57). Fix in the DMG step: delete `target/*/release/{build,incremental}`, linked binaries >100M, and `~/.cargo/registry/src` — but KEEP `target/*/release/deps` (rust-cache's post-job save; the warm cache is the 20-min mac advantage).
- **cache-on-failure: true** on rust-cache (71b1018fe): failed legs now save their compile cache; retries (always a new SHA after the bump) start warm. Binaries can never move across versions — version is compiled in; mismatches break `neoism update`.
- Failed-release cleanup: cancel run, `gh release delete vX.Y.Z --yes` (a green leg may have attached a partial DRAFT), delete tag both sides, bump to next patch, rename the changelog section instead of duplicating it.

## Build-speed overhaul (2026-08-27, ea4c4aa6e)
- Repo is PUBLIC now: standard runners are free + 4-core; GitHub larger runners are NOT available to personal accounts (orgs w/ Team plan only) — 8-core needs an org transfer or a third-party runner app (BuildJet/Namespace-style).
- Measured v0.7.61: build step = windows 63min / mac 51 / linux 37, everything else noise. Caches were being EVICTED: 3 targets x 2-3GB target-dir tarballs vs the 10GB Actions cap on multi-release days (mac went 20min warm → 51 cold).
- Fix: sccache (mozilla-actions/sccache-action@v0.0.9) with GHA backend; auto-switches to R2 when SCCACHE_R2_{BUCKET,ENDPOINT,ACCESS_KEY_ID,SECRET_ACCESS_KEY} secrets exist (HAVE_R2_CACHE job-env pattern). rust-cache now cache-targets:false (registry/git only). Windows: Add-MpPreference Defender exclusions for workspace+.cargo+rustc/cargo/link/sccache.
- Works because release profile has incremental=false (sccache can't cache incremental). Version bumps still recompile all WORKSPACE members (fingerprint includes version) — only deps cache across releases; the members+link tail is the floor (~15-25min on 4-core windows).
- Release costs (public repo = standard runners free): only real spend would be larger runners. R2 free tier covers the cache.
