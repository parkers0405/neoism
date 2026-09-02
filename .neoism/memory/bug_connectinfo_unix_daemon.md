---
name: bug-connectinfo-unix-daemon
description: blank editor + empty workspace picker root cause — ConnectInfo<SocketAddr> 500s over the embedded unix daemon
metadata: 
  node_type: memory
  type: project
  originSessionId: 970ff237-e4ca-454b-a88c-c03ba599d030
---

The desktop's embedded daemon serves the SAME axum router as the TCP/Tailscale path, but over a **unix socket via raw hyper** (`embedded_daemon.rs` `serve_connection_with_upgrades`), which never injects `ConnectInfo<SocketAddr>`. A handler that extracts a bare `ConnectInfo<SocketAddr>` therefore rejects with **HTTP 500 on every connection** over the embedded daemon.

This was the root cause of BOTH the blank nvim editor AND the empty workspace picker: `session_upgrade` required `ConnectInfo<SocketAddr>`, so the desktop's `/session` upgrade to its own in-process daemon 500'd, the `DaemonClient` retried forever, and no editor/workspace frame ever crossed the socket. Everything downstream (nvim spawn, file load, GridUpdate forwarding, redraw conversion) worked in isolation — the failure was the handshake.

**Fix:** make `ConnectInfo` optional and fall back to loopback: `peer_addr: Option<ConnectInfo<SocketAddr>>` → `.map(|ConnectInfo(a)| a).unwrap_or_else(|| SocketAddr::from(([127,0,0,1],0)))`.

**How to apply:** any new daemon handler that needs the peer address must use `Option<ConnectInfo<SocketAddr>>` (or the embedded server must inject the extension), or it will 500 over the unix socket. Reproduce with the in-crate test `embedded_daemon::tests::desktop_daemonclient_streams_file_redraw_over_unix` (spawns the embedded daemon + a real `DaemonClient` over unix + `send_editor(OpenBuffer)`; asserts the file text streams back). Related: [[reference_codex_remote_architecture]], [[project_cross_laptop_status]].
