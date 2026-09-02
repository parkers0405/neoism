---
name: "Neoism v0.7.29 release"
description: "v0.7.29 exact SHA, checks, and GitHub Actions release run"
type: "project"
scope: "project"
origin: "YOLO release workflow"
created: "2026-08-07"
updated: "2026-08-07"
---

# Neoism v0.7.29 YOLO release

- Release commit/tag exact SHA: `2b213cc6b6583f78fdeee6ac94fa1b7530b56c9e`.
- Commit: `release: v0.7.29`; pushed to `origin/main` and annotated tag `v0.7.29` points to the same SHA locally and remotely.
- GitHub Actions run: https://github.com/parkers0405/neoism/actions/runs/31210432888
- Trigger confirmed `in_progress` for `linux-x86_64`, `darwin-aarch64`, and `windows-x86_64`; no same-SHA reusable artifact run was found.
- Pre-release checks passed: cargo metadata; desktop timeline prepend/apply older/older request/timeline growth/fixed diff click tests; 14 shared diff scroll policy tests; fixed viewport test; 11 shared timeline tests; 81 shared Agent state tests; `cargo check -p neoism-ui`; `cargo check -p neoism --bin neoism`; `git diff --check`.
- Known unrelated warnings: unused `LineLength` import in desktop screen tests and unused `id` in neodraw test.
