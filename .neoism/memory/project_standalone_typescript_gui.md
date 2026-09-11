---
name: "Standalone TypeScript Neoism GUI"
description: "Standalone same-server TS GUI implemented; 138 tests, native commands/themes/assets, safe skill roundtrips, web launcher; dev :5176, installed Rust binary needs rebuild"
type: "project"
scope: "project"
origin: "neoism-agent"
created: "2026-09-10"
updated: "2026-09-10"
---

## Implemented
Standalone React/Vite TypeScript GUI in `neoism-agent/sdk/typescript/packages/gui` (`@neoism/gui` npm workspace), direct local @neoism/sdk client of SAME Rust agent server. No WASM or workspace protocol frontend. User explicitly forbids Playwright/browser tools; inspected source, used unit/SSR/SDK/HTTP tests only. No Electron shell yet.

UI: left New Chat/Skills/Workflows + searchable paginated Recents + sticky native plasma avatar/name/settings; exact single-letter N home hover + floating composer; chat-only right directory/session/usage/subagents; safe Markdown/highlighted copyable code/thinking/tools; scoped permission/question interaction cards; 101 generated native themes + licensed bundled fonts.

ALL 32 native command entries/52 spellings generated from Rust; `/models` alias added to native too. Slash and follow-up pickers ABOVE composer, NOT centered modals: `ComposerPanel.tsx` + CSS anchors and clamps viewport, focus restoration, Tab/ShiftTab+arrows navigation, Enter accept/Escape close. App puts model/agent/session/skill/directory/provider/MCP panels inside composer-anchor. `/cd` PATCHes current session, skill selection inserts `$name` draft without sending, account IDs persist server/provider and include connectionId/variant in prompts/commands, deleted selected accounts block silent billing fallback. Goals initial prompt, queue/MCP/permission actions, FX native prompt timing with cancellation. Native GPU FX translated SVG scenes. Browser /exit returns home.

## Backend + SDK
`neoism-agent web [--no-open] [--server URL]` (server flag attach-only), `serve --web`, in gui.rs/web_launcher.rs. Public inert static assets outside API auth, `/v2` never SPA, path/symlink bounds; `NEOISM_AGENT_GUI_ROOT` and installed/checkout dist discovery. Existing daemon server reused, no duplicate database/backend. `NEOISM_AGENT_MANAGEMENT_API=1` plus authenticated operator credential still required for skill writes.
Skill management GET additive `definition.bundle`/`bundleRevision` preserves info/content, full stable bounded bundle enables writable on-disk skill roundtrip with support files. Unsafe/binary/unrepresentable bundles explicit read-only. GUI falls back matching version for older servers; save scope/revision immutable.
SDK facade now exposes history query cursor + command arguments/model/agent options. HTTP and WS keep base URL path prefix, consistent SSE.
History currently returns empty cursor field; GUI falls back final descending message ID on full pages, guards repeated cursors. useChat single-flight history, session epochs, idle/focus/periodic reconciliation; state avoids duplicate racing deltas and deleted-message resurrection, runtime branches keep busy after root idle. Recents independently scoped from active metadata; single-flight creation, stale server/session guards, deep links.

## Verified
138 GUI tests passed, production Vite+TS build passed, SDK contract/plugin/consumer suites passed, native theme/command/avatar/font generator checks passed, npm audit zero vulnerabilities. Rust `cargo check -p neoism-agent -p neoism-agent-server --tests`, 2 GUI route tests + 2 launcher tests + 13 management tests passed. Existing caller.rs unused-import warning only. HTTP production shell/assets smoke test passed. Read-only actual :4096 server listing/history pagination confirmed no overlap. No visual browser inspection, live OAuth completion, or provider-billed prompts/workflows executed.

## Running / commands
Current session started Vite at `http://127.0.0.1:5176` via background job `job_08981e8920012UgeyBDEmJVWza`, 24h lifetime. Port5174 already had unrelated listener left untouched. Command: `npm run dev -w @neoism/gui -- --host 127.0.0.1 --port 5176 --strictPort` from SDK root. `npm run gui:dev -- --host ...` nested npm forwarding FAILS; use workspace command for flags.
Dev endpoint defaults :4096 via import.meta.env.DEV, arbitrary frontend ports supported; VITE_NEOISM_AGENT_URL override. Production same-origin. Installed neoism-agent binary still older (`web --help` unrecognized), so user normal Rust rebuild needed for web launch; running full Neoism agent also needs restart for additive bundle/static behavior. Dev GUI works existing server now.
No commits. Preexisting flake.nix untouched. Docs GUI README and neoism-agent/gui-server.md.
