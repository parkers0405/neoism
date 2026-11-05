# Native terminal visual fixture

From `neoism-frontend/gpuix`, launch with an existing verified debug display addon and core:

```sh
NEOISM_GPUIX_DISPLAY_DEBUG=1 NEOISM_TERMINAL_PREVIEW_PORT=47831 ./node_modules/.bin/bun scripts/launch-terminal-preview.ts
```

The window renders production `TerminalView` alone. There is no preview banner, debug badge, mode bar, extra button, or hidden laid-out control. All control runs over a private authenticated loopback transport. The transport simulates daemon output; no real shell, daemon, session recovery, or user appearance configuration is used. Display pin/checksum/capability verification remains unchanged. The scripts never build or install anything.

The samples show colored Git status, concise check output, and a normal no-match `grep` exit. No staged assertion failure is presented as an application error. The controller switches bottom/top/flow by remounting the production view against the retained session; its run endpoint streams controlled check output. The fixture enables production hover animation.

Attach and save callbacks record only in-memory fixture context and a reactive saved-command list; no real agent, persistence, or workspace state is touched. Native automation enters the paperclip/download/filter/vertical-overflow toolbar without click-to-pin, verifies attach/save callbacks through the private status endpoint, and captures the saved icon state in `saved-command.png`. Copy output and reuse are verified inside More, not as top-level toolbar actions. The controller manifest exposes the fixture-local callback records for inspection.

The fixture requests native window focus at launch and explicitly focuses command input before ghost capture. Native ghost geometry is correctly absent while the OS window is inactive; another development window taking focus can therefore produce an honest completion-capture timeout. The script does not repeatedly steal focus to conceal interference.

For a final bottom/failed-block-hover capture that closes its window immediately afterward and never starts a persistent controller, append `--final-hover` to the launch command. Use `NEOISM_TERMINAL_PREVIEW_OUTPUT=/tmp/neoism-terminal-preview-final` to keep those final PNGs separate.

The controller prints its endpoint and PID and stays alive until terminated. Send SIGTERM to that PID to close the owned window too. Its endpoint is loopback-only; the internal fixture endpoint is separately token-authenticated and never printed with its token. To use a random controller port, omit `NEOISM_TERMINAL_PREVIEW_PORT`.

```sh
curl http://127.0.0.1:47831/
curl http://127.0.0.1:47831/mode -H 'Content-Type: application/json' -d '{"mode":"top"}'
curl http://127.0.0.1:47831/run -H 'Content-Type: application/json' -d '{}'
curl http://127.0.0.1:47831/capture -H 'Content-Type: application/json' -d '{"name":"inspection"}'
curl http://127.0.0.1:47831/call -H 'Content-Type: application/json' -d '{"method":"getTree","params":{}}'
```

Screenshots are written to `/tmp/neoism-terminal-preview-clean/{bottom,top,flow,hover,hover-left,tooltip,filter,more,ghost,tab-menu,running,running-completed,input-after-run}.png`. Override the output directory with `NEOISM_TERMINAL_PREVIEW_OUTPUT`. Native automation verifies unselected hover entry/exit, filter and overflow opening, and absence of stuck toolbar state; results and errors are returned in the manifest. The runner uses real native command input for ghost/Tab captures and private IPC for mode/run changes. The native screenshot API is tried first; if the installed display addon omits screenshot support, existing `hyprctl`/`grim` capture only this fixture's actual compositor rectangle, and the original capture error remains visible in the manifest.

For a direct interactive launch with no controller, use `NEOISM_GPUIX_DISPLAY_DEBUG=1 NEOISM_GPUIX_CORE="$PWD/native/target/debug/libneoism_gpuix_core.so" NEOISM_TERMINAL_INPUT_POSITION=bottom ./node_modules/.bin/bun scripts/preview-block-terminal.tsx`. Choose `top` or `flow` in the position environment variable; add `--maximized` for a larger native window. No runtime GUI controls are inserted into the pane.