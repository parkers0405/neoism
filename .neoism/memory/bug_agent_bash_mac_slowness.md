---
name: bug_agent_bash_mac_slowness
description: Agent bash tool slow on mac — login-shell-per-command + double git status; fixed with env caching + cheaper snapshot
metadata: 
  node_type: memory
  type: project
  originSessionId: 61dc832e-208d-41c5-a0b4-dbc86d0b1599
---

**Symptom:** neoism-agent `bash` tool sluggish on macOS. Two structural per-command costs (independent of the fff content-index fix, which only removed background watcher/scan contention — see [[project_fff_search_dependency]]):

1. **Login shell every command.** `bash.rs` ran `$SHELL -lc <command>` — the `-l` re-sources the whole login profile (`path_helper`, Homebrew, nvm/pyenv/rbenv, oh-my-zsh) on EVERY invocation = 0.5–2s startup per command on mac.
2. **Double `git status`.** `snapshot::bash_before` + `bash_after` each ran `git status --porcelain=v1 -z --untracked-files=all` (+ `git show HEAD:path` per changed file) for the diff/undo timeline — twice per command, whole-repo worktree walk, slow on big/churning repos.

**FIX (landed 2026-07-16, cargo-check clean, branch better_workspace):**
- `tool_support/bash.rs`: capture the login env exactly ONCE per process (`static LOGIN_ENV: OnceCell`, `login_shell_env()`/`capture_login_env()` run `$SHELL -lc env`, `parse_env()` handles multi-line values, drops volatile `PWD/OLDPWD/SHLVL/_`), then run each command with a NON-login `-c` shell + `.envs(login_env)`. First bash call pays the login cost once; rest are fast. Order: login_env base → TERM/NEOISM → context.env (caller wins). Known minor tradeoff: profile-exported bash functions (`BASH_FUNC_*` — contain `%`, rejected by is_env_name) and per-invocation profile logic are not carried; aliases were already unavailable (`-c` non-interactive). PATH/tool env (the point) is preserved.
- `snapshot.rs` `git_status_states`: `git --no-optional-locks status --porcelain=v1 -z --untracked-files=normal` (was `--untracked-files=all`, no `--no-optional-locks`). `normal` stops recursing whole untracked dirs; new files under a fresh untracked dir surface as the dir and are gracefully skipped (FileState::from_path errs on a dir → `.ok()?` → skipped, no crash).

Not yet done (offered if still slow): gate the snapshot entirely behind an env for max speed.

Related: [[project_fff_search_dependency]], [[bug_fs_watch_node_modules_storm]].
