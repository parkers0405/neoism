---
name: reference_warp_oz_architecture
description: "Warp \"Oz\" cloud-agent architecture (2026) — the reference design Neoism's movable-daemon-home is modeled on"
metadata: 
  node_type: memory
  type: reference
  originSessionId: 970ff237-e4ca-454b-a88c-c03ba599d030
---

Warp shipped cloud-agent handoff in 2026 ("Oz" platform). This is the proven reference for Neoism's [[project_cross_laptop_status]] goal. (Earlier I wrongly believed Warp was local-first only — it isn't.)

How Warp Oz actually works:
- Cloud agents run in **full Linux sandboxes (containers)**, on Warp's cloud OR self-hosted ("managed worker daemon orchestrating Docker containers on your machines").
- **Handoff local→cloud** = (1) fork the agent conversation (full transcript, source unmodified), (2) snapshot **uncommitted changes — tracked + untracked** and apply on target, (3) carry attachments. Target repo must match local checkout or the diff fails to apply.
- Local session keeps running; laptop can close; agent keeps going in cloud.
- **Remote control** = publish session state to cloud + live event stream (a relay); steer from terminal/web/phone. Two-phase: upload state, then realtime stream.
- **Back-to-local is Warp's weak spot** (export/PR only) — Neoism's differentiator is a seamless `running_on_host_id` flip back to local, all on the user's own Tailscale (no third-party cloud).

KEY: Warp does NOT migrate live PTYs — it reconstructs in a fresh container from git + uncommitted-diff snapshot, and forks the conversation. Confirms Neoism's honest contract: files travel (git/diff), agents resume, raw shells respawn in cwd. Don't build live-PTY migration.

New glue Neoism needs vs what exists: snapshot uncommitted diff (tracked+untracked) → apply on target [NEW]; home-pointer control plane [NEW]; everything else has a primitive already (from-git provision, agent persistence/resume, cloud_auth, Dockerfile, web ProtocolClient).

Sources: docs.warp.dev/agent-platform/cloud-agents/overview, .../handoff/local-to-cloud, .../cli-agents/remote-control, .../cloud-agents/faqs
