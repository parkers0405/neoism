---
name: "Local Bash lease must use approved workdir"
description: "External Bash workdir rejected by local lease after permission approval"
type: "bug"
scope: "project"
origin: "session"
created: "2026-09-23"
updated: "2026-09-23"
---

2026-08-05: Screenshot from fresh binary/subagent: `process working directory /home/parkersettle/projects/neoism is outside workspace /home/parkersettle/projects/testing_neoism` for Bash with external `workdir`. Root cause: Bash existing_project_path had canonicalized + granted external_directory, but execution_request NativeLocal materialized workspace.local_path from context.cwd (session workspace), while ProcessSpec.cwd was the permitted external workdir; LocalExecutionLease::exec ensure_within_root rejected it. Fix: after approved workdir + shell permission checks, bash_tool sets NativeLocal request.workspace.local_path=Some(cwd.clone()) before acquire; hosted sandbox provider request unchanged, local lease containment check remains. Regression in skip_permissions_allows_move_chat_to_external_project executes joined workspace Bash with exact external_directory `path/*` grant and verifies pwd output, then explicit deny rejects. Verified single test, 16 interaction tests, cargo check -p neoism-agent-server, git diff --check. Background task directly spawns Command after permission checks and did not share this lease bug. Existing running Neoism requires rebuild/restart to test new code.
