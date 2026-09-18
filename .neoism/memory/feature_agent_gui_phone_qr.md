---
name: "feature-agent-gui-phone-qr"
description: "Agent GUI topbar QR opens same workspace/chat on phone via Tailscale pairing, not workspace web UI"
type: "feature"
scope: "project"
origin: "agent"
created: "2026-09-15"
updated: "2026-09-15"
---

Agent GUI phone share is daemon `/agent-gui`, never neoism-frontend/web.

Operator (loopback only) POSTs `/agent-gui/share` with workspace id or unique directory. Private workspaces require explicit `share_workspace`. Ready response is Tailscale HTTP `/agent-gui/?pair=&workspace=&session=` plus qrcode crate SVG. Pairing code is 60s single-use preapproved ReadFiles; claim cannot escalate. Phone GUI claims `/pair/claim`, lists `/agent-workspaces`, joins `/agent/workspaces/:id`. Tokens never in URL/hash. Agent :4096 and `/__neoism/gui` stay loopback.

Vite `/assets` rewritten to `/agent-gui/`. Installed assets still `web/agent-gui`.

Verified: cargo check -p neoism-workspace-daemon -p neoism-agent-server --tests; cargo test filter phone_share/html_assets/tailnet_self/share_url/preapproved; GUI tsc, 45 focused tests, vite build.

Live phone still needs Tailscale on both devices, daemon not loopback-only, GUI dist installed, and a restart of the running daemon.
