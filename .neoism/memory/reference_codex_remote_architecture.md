---
name: reference_codex_remote_architecture
description: "Codex desktop remote-SSH \"app-server\" architecture (2026) — near-identical to Neoism's daemon; basis for native-vs-Docker decision"
metadata: 
  node_type: memory
  type: reference
  originSessionId: 970ff237-e4ca-454b-a88c-c03ba599d030
---

Codex (OpenAI) shipped remote-SSH multi-machine in April 2026. Architecture is a near-mirror of `neoism-workspace-daemon` — strong validation.

- **app-server** = Rust JSON-RPC service exposing every primitive (threads/turns/filesystem/approvals/MCP). Runs ON THE HOST WHERE THE CODE IS — local or remote, SAME binary. (≈ neoism-workspace-daemon)
- **Thin client** (TUI/desktop/phone) = pure rendering layer; sends JSON-RPC, streams notifications. All file/command/agent work happens on the server host. (≈ neoism desktop/web clients)
- **Transport:** stdio when local; WebSocket when remote. Remote reach = `ssh -L 4500:127.0.0.1:4500 devbox` then `codex --remote ws://127.0.0.1:4500`, or `wss://` direct. (Neoism `DaemonEndpoint` already parses unix:// + ws://, desktop already takes --daemon-url → SSH-forward attach should ~work after Phase 0.)
- **Multiple devices** = "multiple TUIs against one server", each connects independently over ws. (≈ neoism SessionRegistry multi-subscriber broadcast)
- **Execution = NATIVE on host, NOT Docker** — sandboxed via bubblewrap (Linux) / Seatbelt (macOS). Desktop auto-detects hosts from `~/.ssh/config`.

DECISION for Neoism (native vs Docker): run the daemon NATIVE for home/home-server/SSH cases (zero-lag, direct fs, real shell — Codex model). Use Docker ONLY for the disposable cloud-burst case (Warp Oz model: `docker run` daemon provisioned from git + diff snapshot). Same binary, two launch modes = the "modular" the user wants. Docker around the HOME daemon would add fs indirection + NAT hop + tty quirks = worse feel.

Two reach mechanisms, identical ws path: SSH port-forward (works everywhere, no infra) and Tailscale (always-on upgrade). Nice UX to copy: read ~/.ssh/config, list hosts, auto `ssh -L` + launch remote daemon.

Sources: codex.danielvaughan.com/2026/04/17/codex-remote-ssh-app-server-architecture, developers.openai.com/codex/remote-connections, openai.com/index/work-with-codex-from-anywhere. Related: [[reference_warp_oz_architecture]], [[project_cross_laptop_status]]
