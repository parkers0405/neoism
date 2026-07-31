#!/usr/bin/env bash
# neoism-nuke — stop EVERYTHING neoism on this host.
#
# `pkill neoism` is not enough, by design:
#   * The hosted server runs in a DOCKER container (neoism-server-*), whose
#     restart policy respawns the daemon the instant pkill kills it — you must
#     stop the container, not the process.
#   * Wrapper / entrypoint processes (tini, `bash -c '… neoism-workspace-daemon …'`,
#     docker-proxy) have names that do NOT contain "neoism", so `pkill neoism`
#     (which matches the 15-char process NAME) skips them. `pkill -f` matches
#     the full command line and catches them.
#   * Daemons are launched detached (setsid / reparented to `systemd --user`),
#     so they outlive the GUI — but they are still killable by name/-f.
#
# This tears down: docker server containers, the workspace daemon(s), the agent
# server, and the GUI — then verifies nothing survived.
set -uo pipefail

say() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*"; }

# --- (1) Docker: stop the hosted-server containers (the respawners) ---------
if command -v docker >/dev/null 2>&1; then
  containers="$(docker ps -a --filter 'name=neoism' --format '{{.ID}} {{.Names}}' 2>/dev/null)"
  if [ -n "$containers" ]; then
    say "stopping neoism docker containers:"
    printf '   %s\n' "$containers"
    # `down` (compose) tears down the whole project incl. restart policy;
    # fall back to a direct stop/rm for any stragglers.
    docker compose down --remove-orphans >/dev/null 2>&1 || true
    echo "$containers" | awk '{print $1}' | xargs -r docker rm -f >/dev/null 2>&1 || true
  else
    say "no neoism docker containers running"
  fi
else
  warn "docker not found — skipping container teardown"
fi

# --- (2) Processes: match by FULL command line, not just the 15-char name ---
# Order: agent server, then daemon, then the GUI. SIGTERM first, then SIGKILL
# for anything that ignores it.
patterns=(
  'neoism-agent'
  'neoism-workspace-daemon'
  'neoism-lsp'
  'target/[a-z]*/neoism'   # dev build of the GUI binary
  '/neoism$'               # installed GUI binary
)

kill_pattern() {
  local sig="$1" pat="$2" pids
  pids="$(pgrep -f "$pat" 2>/dev/null | grep -vw "$$" | tr '\n' ' ')"
  [ -z "${pids// /}" ] && return 0
  say "kill -$sig  ($pat):  $pids"
  # shellcheck disable=SC2086
  kill "-$sig" $pids 2>/dev/null || true
}

for pat in "${patterns[@]}"; do kill_pattern TERM "$pat"; done
sleep 1
for pat in "${patterns[@]}"; do kill_pattern KILL "$pat"; done

# --- (3) Verify ------------------------------------------------------------
sleep 1
survivors="$(pgrep -af neoism 2>/dev/null | grep -vw "$$" | grep -v 'neoism-nuke')"
if [ -n "$survivors" ]; then
  warn "still alive (likely a docker restart loop — check 'docker ps'):"
  printf '   %s\n' "$survivors"
  exit 1
fi
say "clean — no neoism processes remain"
