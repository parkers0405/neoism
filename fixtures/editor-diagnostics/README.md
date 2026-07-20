# Editor diagnostics scroll fixtures

These files are deliberately broken. They are manual fixtures for checking
status-line LSP selection, inline diagnostic positioning, diagnostics popups,
and scroll-away/scroll-back behavior.

- `godot/scroll_errors.gd`: a 221-line GDScript project with intentional
  errors near the top, middle, and bottom. Neoism's built-in Godot adapter
  connects to the editor-owned language server on TCP port 6005, so open this
  project in Godot before testing the file in Neoism. It must show Godot only,
  never a Rust or workspace-borrowed status/diagnostic.
- `godot/scroll_scene.tscn`: verifies the default Godot scene icon and opens
  the long script.
- `rust/src/main.rs`: a standalone Rust crate with intentional errors spread
  through more than 200 lines. Open it with rust-analyzer installed, scroll an
  error off screen, then back. The diagnostic must stay attached to its source
  line and return when the line returns.
- `docker/Dockerfile`: a 216-line Dockerfile with real Docker-language-server
  lint/parse failures near the top, middle, and bottom. It must route only to
  Docker's language server.
- `nix/flake.nix`: a 217-line flake with undefined names near the top,
  middle, and bottom. It must route only to nil, never rust-analyzer.
- `typescript/scroll_errors.ts` and `javascript_scroll_errors.js`: standalone 200-plus-line
  TypeScript/JavaScript files with strict project settings and intentional type
  errors near the top, middle, and bottom. Both must attach to the shared
  TypeScript adapter and publish diagnostics while typing, without waiting for save.

Expected errors are 4 for Rust and at least 3 each for GDScript, Nix, Docker,
TypeScript, and JavaScript. In every file, scroll each error off screen and back repeatedly; its
lens must remain attached to its source row and the status count must not
oscillate.

If the Rust status is red and no diagnostics appear, open its popup: the
fixture requires a working `rust-analyzer`, not merely a shim present on PATH.
If Godot is disconnected, open the fixture project in Godot and verify its
Network > Language Server port is 6005 (or set Neoism's configured endpoint).

Do not “fix” the marked errors; they are the purpose of the fixtures.
