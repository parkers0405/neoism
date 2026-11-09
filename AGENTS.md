# Agent instructions

## Frontend: read `rune/` first

The frontend is being rebuilt as a native Rust app on [Rune](https://github.com/anomalyco/rune), on the
`rune-frontend` branch. Before working on any UI:

1. Read [`rune/README.md`](rune/README.md): the plan, the look (theme, fonts, chrome) and the exact scroll and
   animation behavior to reproduce.
2. Look at the screenshots in [`rune/reference/`](rune/reference/). They are the current shipping app and the
   source of truth for how every surface must look.
3. Build UI in `rune/app` with Rune elements. Do not add new painting to `neoism-ui`/Sugarloaf or new GPUiX
   (`neoism-frontend/gpuix`) features; both are kept only as the reference until the cutover step in the plan.
4. Check your work headlessly: render the Rune app to PNG and compare it with the matching reference screenshot.

## Backend

The workspace daemon (`neoism-workspace-daemon`), agent server (`neoism-agent`) and the wire protocol
(`neoism-protocol`) are unchanged by the frontend rewrite. The Rune app is a daemon client.

## Performance

Follow the rules in `rune/README.md` ("Data and performance rules") and measure frame pacing with presentation
timestamps as described in `docs/gpuix-live-rendering-baseline.md`.
