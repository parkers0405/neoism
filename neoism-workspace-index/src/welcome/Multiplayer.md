# Multiplayer & Remote

A workspace shouldn't be trapped inside one desktop process. Neoism is built so the same workspace can move between the native app, a browser, a phone, or another laptop.

## How it works

- `neoism-workspace-daemon` owns PTYs, workspace state, pairing tokens, and web/mobile sessions.
- The web client connects over WebSocket using `neoism-protocol`.
- Desktop and web share UI policy through `neoism-ui`, so the workspace feels consistent across devices.
- Your display name in presence comes from `[neoism] display-name` (see [[Configuration/Configuration|Configuration]]); set a `cursor-style = "rainbow"` and collaborators see your caret sweep colors in sync.

## Connect another device

1. Join the host machine to **Tailscale**.
2. Run the daemon (a prebuilt install already runs it for you):
   ```sh
   cargo run -p neoism-workspace-daemon
   ```
   It listens on `127.0.0.1:7878` and serves `/session`.
3. From a phone, tablet, or laptop on the same tailnet, open the web client and point it at `ws://<tailscale-ip>:7878/session`.

Tailscale keeps the workspace reachable to your own devices without exposing it to the open internet.

## Pairing & auth

By default the daemon accepts local clients. To require pairing tokens (minted by the desktop app), start it with:

```sh
NEOISM_REQUIRE_AUTH=1 cargo run -p neoism-workspace-daemon
```

Collaborative code and Markdown/notes editing are part of the direction here — one workspace, every screen.
