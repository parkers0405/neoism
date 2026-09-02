---
name: feature-first-run-bootstrap
description: Untar-and-run self-bootstrap — desktop app installs parsers/terminfo/launcher on first launch; legacy Rio welcome window removed
metadata: 
  node_type: memory
  type: project
  originSessionId: c6967401-4270-436e-9d09-bcdf40124a24
---

Neoism tarball installs are self-sufficient (added 2026-07-01): `neoism-frontend/desktop/src/bootstrap.rs` spawns a background thread every launch that installs whatever is missing — bundled tree-sitter parsers/queries from `runtime/` next to the binary (built by `scripts/build-treesitter-runtime.sh` in CI, packed by build-stack.yml; `RUNTIME_VERSION` marker vs `.bundled-runtime-version` in the data dir gates re-copy), terminfo via `tic` into `~/.terminfo`, and the Linux desktop entry + icons (embedded via include_bytes!, Exec rewritten to current_exe). Lua nvim runtime files were ALREADY embedded/self-installed by neoism-backend performer/nvim.rs — same pattern.

First-launch double-window bug fixed in main.rs: `ConfigError::PathNotFound` now writes the default config silently and boots to Terminal. Previously it flowed through `report_error` → first window became `RoutePath::Welcome` (legacy Rio "press enter" screen, drawn pac-man logos in router/routes/welcome.rs) AND the daemon materialized a second window because `unbound_native_window_for_daemon` only adopts `RoutePath::Terminal` windows.

**Gotchas:** verify with a sandboxed fresh `$HOME` — but sandbox path must be SHORT (unix socket SUN_LEN limit kills launch under the long scratchpad path); sandboxing `XDG_RUNTIME_DIR` also hides the Wayland socket so no window appears (bootstrap still completes) — `scripts/fresh-run.sh` does both correctly (absolute WAYLAND_DISPLAY passthrough). Remaining external runtime deps: nvim + ripgrep (step 2 = auto-download static builds, not built yet).

**Install/update surfaces (all bootstrap-first now):** root `./install.sh` rewritten lean (build binaries + runtime bundle into BIN_DIR only; obsolete flags warn); `scripts/install.sh` = curl download installer (also places runtime/) — the PUBLIC servable copy lives at neoism-dist repo root (`curl raw.githubusercontent.com/parkers0405/neoism-dist/main/install.sh | bash`; source repo is private so raw URLs into it 404); after editing, re-mirror via `gh api -X PUT repos/parkers0405/neoism-dist/contents/install.sh` with sha, raw CDN caches ~5min; `neoism update` self-update pulls latest from public parkers0405/neoism-dist and swaps binaries + runtime/ next to exe; `scripts/release.sh X.Y.Z` bumps version+lock, tags (tag MUST equal CARGO_PKG_VERSION or update loops), push triggers release-neoism.yml → publishes to neoism-dist. Super+Enter Hyprland bind points at the untarred release binary in the repo (gitignored). Related: [[project-ci-build-stack]].
