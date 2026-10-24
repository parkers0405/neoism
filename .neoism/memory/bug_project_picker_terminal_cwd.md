---
name: "Project dropdown must rebase the actual terminal"
description: "Agent dropdown must change real terminal cwd; native queued rebase validated with real PTY; old metadata-only policy superseded"
type: "bug"
scope: "project"
origin: "user"
created: "2026-10-15"
updated: "2026-10-15"
---

Latest explicit user requirement overrides the old 'no shell input injected' project-picker behavior: selecting a directory from the AGENT SIDEBAR dropdown MUST change the existing terminal's REAL working directory too, not merely workspace/Explorer/status metadata. The reported failure was workspace ~/projects/neoism while cargo actually executed in /home/parkersettle. Do not fake composer cwd; it must remain actual terminal cwd.

Native fix is in screen/bridges/conversation_workspaces.rs commit_workspace_directory(), with queue/pump helpers in conversation_workspaces/terminal_rebase.rs and real PTY tests under terminal_rebase/pty_tests.rs. Capture stable workspace, route, PID/rich-text identity; use focused terminal or workspace PRIMARY BACKING PTY when Agent/editor covers it. `has_non_terminal_surface()` does NOT mean no live backing shell: excluding it caused the exact user case to fail. The explicit UI commit queues actual shell cd through the normal command path with literal shell-specific quoting and strong idle/prompt/foreground/alternate-screen/passthrough checks. Busy targets get no control input. Preserve composer draft and gate submission until real cwd and prompt acknowledgment; stale or replaced targets cannot redirect to new PTYs. Actual Linux /proc cwd proof may update terminal cwd metadata; never write the selected path cosmetically. Same workspace/grid, PTY, environment, jobs, scrollback and focus are retained. Ambient root synchronization/broadcasts must not execute directory changes on all peers.

Native validation collected: six focused tests including controlling real Bash PTY home -> quote/space-containing repository -> actual pwd/cargo locate-project, same PID, environment/functions/background job/draft preserved, Agent-focused primary route selection, busy/alt-screen no-input deferral. cargo check -p neoism --bin neoism passed. No release build. Protocol/daemon/web ChangeTerminalDirectory request+correlated replies were being completed separately; do not assume an always-rejecting daemon is a complete successful feature.

User strongly rejected my attempt to operate their live window/run pwd; code fixes must stay in repository tools unless explicitly asked for live UI control. I promised not to touch their active GUI again. See feedback_no_live_ui_for_code_fixes.md.
