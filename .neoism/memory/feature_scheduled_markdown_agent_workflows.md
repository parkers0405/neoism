---
name: "Scheduled Markdown agent workflows"
description: "Global workflows with no directory now default cross-platform to the OS home/user profile; project workflows still default to workspace root, with explicit relative/home/absolute overrides."
type: "feature"
scope: "project"
origin: "scope-aware default directory correction"
created: "2026-08-24"
updated: "2026-08-24"
---

# Scheduled Markdown agent workflows

Default directory semantics are scope-aware:
- project workflow source under workspace `.neoism/workflows`: omitted `directory` uses tracked workspace root;
- global workflow source under default Neoism config (`~/.config/neoism/workflows` on Linux), `$HOME/.neoism`, or `NEOISM_AGENT_CONFIG_DIR`: omitted `directory` uses OS home from `dirs::home_dir` (`$HOME` on Unix/macOS, user profile on Windows).

Explicit `directory` still accepts a relative path (resolved from the scope default), `~/...`, or absolute path. Selected directory must exist; normal session project discovery categorizes it by its Git worktree or global context.

Bundled docs explain this distinction and Welcome seed bumped to v10. Focused global-home and project/external directory tests pass; packages compile.
