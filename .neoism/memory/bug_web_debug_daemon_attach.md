---
name: "web debug daemon attach"
description: "Web hardcoded :7878 so debug attached to the head daemon; now ?daemon=/same-origin plus daemon-served UI."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-08-20"
updated: "2026-08-20"
---

Web was hardcoding `ws://127.0.0.1:7878/session` (the head daemon). Debug desktop `configure_debug_service_isolation` remaps `NEOISM_DAEMON_TCP_PORT` to an ephemeral port (same pattern as `NEOISM_SERVER` for the agent). The browser cannot read process env.

Fix (2026-04-18):
- `resolveDaemonTarget`: `?daemon=` / `?url=` wins and auto-connects; same-origin daemon pages use `/session`; Vite :5173 stays on injected/head default.
- Start Web Server opens `http://127.0.0.1:{NEOISM_DAEMON_TCP_PORT}/` when a built UI is installed; checkout fallback is Vite preview with `?daemon=ws://127.0.0.1:{port}/session`.
- Daemon serves `NEOISM_WEB_ROOT` / `share/neoism/web` / repo `web/dist` as fallback so production does not need `npm`.
- `install.sh` copies dist to `$PREFIX/share/neoism/web`.
- ProtocolClient actually appends `?token=`.
- `desktop_daemon_url` / host summary advertise `NEOISM_DAEMON_TCP_PORT`, not hardcoded 7878.

Vite without query still defaults to 7878 (head). Pass `NEOISM_DAEMON_TCP_PORT` / `VITE_NEOISM_DAEMON_URL` or open via Start Web Server.
