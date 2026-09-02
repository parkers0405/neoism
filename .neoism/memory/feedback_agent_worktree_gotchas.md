---
name: feedback_agent_worktree_gotchas
description: Two recurring traps when running background worktree agents in this repo — stale bases and the shell cwd jumping into agent worktrees
metadata: 
  node_type: memory
  type: feedback
  originSessionId: 970ff237-e4ca-454b-a88c-c03ba599d030
---

Two traps with background worktree agents (both bit me in the Wave 6/7 sessions, 2026-06-09):

1. **Stale worktree bases.** `isolation: worktree` sometimes branches from an OLD commit (one 6B agent got a pre-Wave-5 `main`-era base and rebuilt existing code from scratch). **Why:** worktree creation doesn't reliably snapshot the current branch tip. **How to apply:** every agent prompt gets a mandatory STEP 0: `git log --oneline -1`, and if HEAD is not a descendant of the current work-branch tip, `git merge work_anywhere --no-edit` before reading any code. Before merging an agent branch, check `git merge-base` against the work branch.

2. **Shell cwd teleports into agent worktrees.** After a background agent completes (or sometimes on new turns), the persistent Bash cwd ends up inside `.claude/worktrees/agent-<id>/` — git commands then silently run on the AGENT's branch ("Already up to date" merges, wrong-tree test runs that still pass because the worktree has the code). **How to apply:** prefix every git/cargo verification with an explicit `cd /home/parkersettle/projects/neoism` (or `git -C`), and treat any surprising "Already up to date" as a cwd check trigger (`pwd` + `git branch --show-current`).
