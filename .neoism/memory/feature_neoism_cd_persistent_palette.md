---
name: "neoism cd and persistent wrapped directory palette"
description: "Global deterministic Alt+D, dynamic animated directory rows, parent-shell cd, exact source-session echo suppression"
type: "feature"
scope: "project"
origin: "session"
created: "2026-08-10"
updated: "2026-08-11"
---

---
name: "neoism cd and persistent wrapped directory palette"
description: "Global deterministic Alt+D, dynamic animated directory rows, parent-shell cd, exact source-session echo suppression"
type: "feature"
scope: "project"
origin: "session"
created: "2026-08-10"
updated: "2026-08-11"
---

Implemented `neoism cd` terminal control and persistent Alt+D command sheet. CLI dispatch recognizes cd before file/open behavior (including explicit argv0), supports HOME/no operand, relative cwd, absolute, ~/..., OLDPWD, canonical directory validation, rejects multiarg/control. Desktop and daemon PTY configs pass scoped NEOISM=1 through neoism-terminal-pty/teletypewriter; Unix/Flatpak and Windows ConPTY child env override paths preserve inherited env/TERM. OSC 777 remains the fallback for shells without integration.

Alt+D is app-global on desktop/web. Target order is focused pane-associated live terminal, current-workspace MRU live terminal, other live terminal fallback, then PTY creation; route/session is captured so focus changes cannot redirect submit. Desktop tracks last rendered terminal per current workspace, web tracks last active session and continues palette opening after async PTY creation. Alt+Shift+D owns markdown draw mode so plain Alt+D is unambiguous.

Directory rows compose current, parent, home, declared workspace root, recents, then host-measured filesystem completions, deduped by canonical target. The workspace root is an option only: terminal changes never reroot declared workspace/file tree. Tab/Shift+Tab cycles through rows via existing cursor and list-scroll springs while typing remains normal. Accepted changes preserve the captured target and update only its cwd; shell-resolved HOME/OLDPWD waits for authoritative OSC 7. Remote ~/ paths use remote HOME with shell-specific literal quoting rather than desktop HOME.

Generated bash/zsh/fish and PowerShell integrations now define a `neoism` parent-shell wrapper: only `neoism cd [directory]` invokes builtin cd/Set-Location (including no operand and `cd -`), emits OSC 7 after success, and every other invocation delegates to the executable. This removes executable→OSC→reinjection duplicate command/history behavior. A real isolated bash test verifies parent cwd and OLDPWD behavior with spaces.

Palette PTY injection has one-shot exact source-session echo suppression: `SyntheticEchoFilter` in terminal core for desktop and a per-session TypeScript filter for web. It handles fragmented command echoes and LF/CRLF, fails closed on mismatch, preserves prompt/output/user bytes, and uses no clear/erase sequence. Web filter map entries are removed on completion/mismatch.

Verification: native core/shared/PTY/daemon/desktop checks and WASM check pass; web typecheck and all 163 tests pass; focused command palette, target resolver, shell generation, real bash wrapper, and synthetic echo tests pass; git diff --check passes. Full shared lib tests pass 2323/2323, but pre-existing unrelated `shared/tests/buffer_tabs.rs` expectations fail (520 vs old 220 cap). Full backend test run has unrelated `agent.enabled-providers` descriptor failure. No release build run.
