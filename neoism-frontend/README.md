# Neoism frontend

`gpuix/` is the native application and the default frontend. `launcher/` provides the Cargo development entrypoint. The old desktop and WASM hosts are retired.

`shared/` retains the useful editor, Markdown, session layout, panel state, animation, and input policies. Its rendering entrypoints accept the portable `Paint` interface. `RecordingPainter` produces commands and CPU text rasters for GPUIX; it owns no GPU context or window.

CPU font discovery, shaping, rasterization, image data, and virtual document layout live in `../neoism-paint`. Styling data lives in `../neoism-customization`, without a scripting runtime. `../neoism-terminal-types` contains presentation/input data; Ghostty owns emulation in the client, and the daemon owns PTYs.

Run `make build`, `make run`, or `make dev` from the repository root. See [the GPUIX guide](gpuix/README.md) for native checks and implementation details.
