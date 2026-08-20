#!/usr/bin/env bash
# Neoism standalone workspace daemon — one-shot "work from anywhere" launcher.
#
# Brings the daemon up reachable over Tailscale (or any LAN) with pairing-token
# auth required. Mirrors the Phase 0 hardening: bind a real address (not just
# loopback), require auth, and make sure the daemon token file exists so a
# remote client can present `?token=<secret>` / a pairing token.
#
#   ./scripts/neoism-daemon.sh            # tailnet IP if present, else 0.0.0.0
#   NEOISM_BIND=192.168.1.20 ./scripts/neoism-daemon.sh   # force a bind addr
#   NEOISM_PORT=9000          ./scripts/neoism-daemon.sh   # override the port
#   NEOISM_LOG=debug          ./scripts/neoism-daemon.sh   # override RUST_LOG
#
# The daemon itself mints/loads the token on startup (see
# neoism-workspace-daemon::daemon_token); we resolve + print the path here too
# so the operator can `cat` it to copy the secret to a phone/web client.
#
# Refactored for better error handling, added support for custom log levels,
# and improved comments for open-source contributors.
set -euo pipefail

PORT="${NEOISM_PORT:-7878}"
LOG_LEVEL="${NEOISM_LOG:-info}"

say() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
err() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# --- (a) resolve the bind address ----------------------------------------
# Prefer the host's Tailscale IPv4 (first line of `tailscale ip -4`) so the
# daemon is reachable across the tailnet. Fall back to 0.0.0.0 (all
# interfaces) when tailscale isn't installed or has no address yet. An
# explicit NEOISM_BIND always wins. Added explicit error path for tailscale.
resolve_bind() {
  if [ -n "${NEOISM_BIND:-}" ]; then
    printf '%s' "$NEOISM_BIND"
    return
  fi
  if command -v tailscale >/dev/null 2>&1; then
    local ip
    ip="$(tailscale ip -4 2>/dev/null | head -n1 | tr -d '[:space:]')"
    if [ -n "$ip" ]; then
      printf '%s' "$ip"
      return
    else
      say "tailscale installed but no IP returned; falling back to all interfaces"
    fi
  fi
  # No tailnet address — bind every interface so a LAN / ssh -L reach still
  # works. Auth is required (below), so this is not an open door.
  printf '0.0.0.0'
}

BIND="$(resolve_bind)"
ADDR="${BIND}:${PORT}"

# --- (b) require pairing-token auth for the non-loopback bind ------------
# Locked decision: any non-loopback bind must enforce auth. The daemon's
# `Hello` handshake + legacy `?token=` path both honour this gate.
export NEOISM_REQUIRE_AUTH=1

# --- (c) ensure the daemon token exists + show its path ------------------
# The daemon mints/loads this on startup, but we compute the same path here
# so the operator can read the secret out of band. Path mirrors
# neoism-workspace-daemon::daemon_token::daemon_token_path.
if [ -n "${XDG_RUNTIME_DIR:-}" ]; then
  TOKEN_PATH="${XDG_RUNTIME_DIR}/neoism/daemon-token"
else
  TOKEN_PATH="/tmp/neoism-$(id -u)/daemon-token"
fi
mkdir -p "$(dirname "$TOKEN_PATH")"
chmod 700 "$(dirname "$TOKEN_PATH")" 2>/dev/null || true

# --- (c.5) advertise this host's dialable URL (Wave 4E) ------------------
# Clients resolve a workspace's `running_on_host_id` -> this URL to re-dial
# when a workspace is promoted/demoted here. The daemon puts it on its
# bootstrap host's `HostSummary.daemon_url` (via NEOISM_HOST_URL). Use the
# resolved bind IP; skip for a bare 0.0.0.0 bind (not dialable) unless the
# operator set NEOISM_HOST_URL explicitly.
if [ -z "${NEOISM_HOST_URL:-}" ] && [ "$BIND" != "0.0.0.0" ]; then
  export NEOISM_HOST_URL="ws://${BIND}:${PORT}/session"
fi

say "bind address : ${ADDR}"
say "host url     : ${NEOISM_HOST_URL:-(unset — set NEOISM_HOST_URL for remote re-dial)}"
say "auth         : NEOISM_REQUIRE_AUTH=1 (pairing token / bearer required)"
say "token file   : ${TOKEN_PATH}"
say "             : read it with  cat '${TOKEN_PATH}'  (created on first start, mode 0600)"
say "log level    : ${LOG_LEVEL}"

# --- (d) exec the daemon -------------------------------------------------
# `exec` so signals (Ctrl-C / SIGTERM) reach the daemon directly and this
# wrapper leaves no extra process behind. RUST_LOG defaults to info but an
# operator-set value is respected.
export RUST_LOG="${RUST_LOG:-$LOG_LEVEL}"
say "starting neoism-workspace-daemon …"
exec cargo run -p neoism-workspace-daemon -- --addr "${ADDR}"
