---
name: "bug_windows_agent_powershell_quoting"
description: "Windows bash tool used PowerShell -Command; JSON quoting died"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-11"
updated: "2026-09-11"
---

---
name: windows-agent-powershell-encodedcommand
description: "Windows bash/background_task spawned PowerShell with -Command, so JSON quotes/braces died in CreateProcess quoting. Fix: UTF-16LE -EncodedCommand (same recipe as PTY hook, no -NoExit), failure label uses display_name, Windows system prompt + bash tool description tell the model it is PowerShell/curl.exe."
type: bug
origin: conversation
created: 2026-08-11
updated: 2026-08-11
---

# Windows agent bash quoting (PowerShell -EncodedCommand)

## Symptom
curl.exe JSON-RPC bodies on Windows became `{'jsonrpc':...}` / unquoted keys. Server returned JSON-RPC -32700; curl then treated `{...}` as a glob. Exit 1 was reported as `bash command failed` even though the host was PowerShell.

## Cause
`neoism-agent-server` `platform_shell.rs` ran `pwsh.exe`/`powershell.exe` with `-Command` + the raw model string. CreateProcess quoting ate quotes/braces. Interactive PTY already used UTF-16LE `-EncodedCommand`.

## Fix
- `ShellRuntime::command_args`: PowerShell one-shot argv is `-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand <utf16le-b64>` of `$OutputEncoding=UTF8; {command}`. No `-NoExit`. POSIX/cmd unchanged.
- Shared by `bash.rs` (`apply_command(..., false)`) and `background_job.rs` (`apply_command(..., true)`).
- Failure text uses `runtime.display_name()` (`PowerShell command failed...`).
- Windows bash tool description + system-prompt fragment: tool name is still `bash`, host is PowerShell; use `curl.exe` / `--data-raw`.

## Tests
`powershell_uses_encoded_command_and_preserves_json`, `posix_and_cmd_keep_plain_command_argv` in `platform_shell.rs`.

Do not rewrite PTY interactive injection or validation scripts that still use `-Command` on purpose.
