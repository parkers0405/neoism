---
name: bug-joined-workspace-daemon-credential-401
description: "Joined workspace 401 'invalid daemon credential' — daemon/agent signing-key drift; FIXED: on-disk token is the live trust root on both sides"
metadata:
  type: project
---

Joined workspace agent 401'd (guest lost host model/history/thinking), user saw "invalid daemon token" (2026-08-28, fixed f4ffb56d6).

**Chain:** guest device-bearer → host daemon `/agent` reverse proxy (mints fresh 60s `daemon_credential` per request, `AGENT_CREDENTIAL_LIFETIME_SECS=60` — per-request minting means the short lifetime is FINE) → host agent verifies via `CallerPolicy` (caller.rs). Both daemon and agent read `NEOISM_DAEMON_TOKEN` **once at process start**, but the canonical file (`$XDG_RUNTIME_DIR/neoism/daemon-token`, tmpfs — wiped on logout) rotates, and the DEV desktop profile uses a different file (`state_root/daemon-token`). Stale env on either side = signature mismatch = permanent 401 until restart.

**Fix:** file is the live trust root. Daemon re-reads the file when minting (server.rs `mint_agent_credential`, env fallback); agent retries a failed signature against the canonical file (`canonical_daemon_token_from_disk` in caller.rs; windows suffix mirrors desktop `per_user_suffix`). Tests: `signature_mismatch_falls_back_to_canonical_token_file`, `agent_proxy_denies_unauthenticated_and_mints_scoped_identities` (now scopes XDG_RUNTIME_DIR — the real machine token file otherwise shadows the test env key: any test touching minting must sandbox XDG_RUNTIME_DIR).

**Immediate user unblock without new binaries:** restart the daemon (or app) on the HOST so env re-reads the current file.

Related: [[project-joined-workspace-ssh-model]], [[bug-hosted-workspace-badge-and-join-adopt]]
