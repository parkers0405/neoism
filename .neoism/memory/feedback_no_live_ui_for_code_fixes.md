---
name: "Code fixes must not interrupt the live GUI"
description: "Fix repository code without touching the user's active app/window unless explicitly requested"
type: "feedback"
scope: "project"
origin: "user"
created: "2026-10-15"
updated: "2026-10-15"
---

For repository coding fixes, stay in the code. Do not switch the user's live Neoism tabs, focus their window, type verification commands, or otherwise operate the active GUI unless they explicitly request live UI testing/control. During the workspace-dropdown/terminal-CWD bug, I switched to Terminal and ran pwd as an attempted immediate check; the user objected ('WTF ARE YOU DOING', 'ITS IN THE DAMN REPO'). I promised to leave the live window alone. A screenshot bug report is not a request for a live-window workaround. Reproduce with isolated PTY/tests and repository tools instead, preserving their interactive state.
