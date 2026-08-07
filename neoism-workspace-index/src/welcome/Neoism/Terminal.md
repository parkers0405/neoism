# Terminal

Neoism's terminal is a native GPU-rendered PTY surface, not a simulated command prompt. Interactive shells, alternate-screen applications, mouse reporting, OSC links, job control, and terminal graphics run against a host-side session owned by the workspace daemon.

## Open terminals

Use `Ctrl+Shift+T` on Linux/Windows or `Cmd+T` on macOS. A new terminal starts in the active workspace root. Splitting creates another pane/session without redefining the workspace.

Configure a shell in the `terminal` domain:

```jsonc
{
  "terminal": {
    "shell": {
      "program": "/bin/fish",
      "args": ["--login"]
    }
  }
}
```

Omit the block to use Neoism's platform default.

## Working directory

Each terminal tracks its own current directory. `cd` affects that shell only. New project-level content, Explorer, search, and agents remain rooted at the declared workspace directory.

## Scrollback and selection

- `Shift+PageUp` / `Shift+PageDown`: page through history.
- `Shift+Home` / `Shift+End`: top or bottom.
- Terminal context menu: copy, paste, clear, and search.

Scrollback is bounded host-side history, not durable terminal recording. A daemon restart does not checkpoint a running process or guarantee its old output can be reconstructed.

## Command blocks and background processes

Shell integration can mark prompts/commands with OSC 133 boundaries, allowing Neoism to present command-aware output. A process placed in the shell's background remains part of that PTY. Agent `background_task` jobs are different: they are tracked by Neoism Agent and have their own status/cancellation UI.

## Links, paths, and graphics

Neoism recognizes terminal hyperlinks/path hints and supports terminal image protocols handled by the renderer. Applications remain responsible for emitting valid escape sequences.

## Vi mode and cursor

Terminal Vi mode and the editor's Vim layer are separate. Toggle Neoism's Vim layer with `Alt+Shift+Space` where applicable. Terminal applications can still use their own modes.

Configure cursor appearance in the terminal domain:

```jsonc
{
  "terminal": {
    "cursor": {
      "shape": "block",
      "blinking": false
    },
    "scroll": {
      "multiplier": 3.0
    }
  }
}
```

## Remote terminals

The PTY runs on the daemon host. Desktop, browser, and mobile clients attach to that host-owned session; they do not move the process onto the viewing device. Disconnecting a client does not intentionally stop the shell.

See [[Neoism Daemon/Sessions and Persistence]] and [[Neoism Daemon/Remote Devices and Pairing]].

## Troubleshooting

- A missing terminal after a daemon restart is not recoverable as a live process; create a new session.
- A persisted tab can reference a stale session ID. Recreate/reattach instead of reopening it indefinitely.
- If input appears captured, dismiss the workspace picker or modal with `Esc` and refocus the terminal.
- If the web terminal stops updating, check daemon connectivity before changing shell configuration.
