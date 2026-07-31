#!/usr/bin/env bash
# neoism-nuke — stop all HOST-side neoism processes (GUI, workspace daemon,
# agent). It deliberately does NOT touch anything running inside a container:
# containerized servers own their own lifecycle (docker/podman manage them),
# so we never signal into them.
#
# Why `pkill neoism` alone is not enough:
#   * the daemon/agent process NAMES are truncated to 15 chars
#     ("neoism-workspac"), so a name match misses them — need `pkill -f`
#     (full command line);
#   * but `-f` also reaches the containerized daemon (its host-visible pid),
#     so we filter those out by PID namespace / cgroup.
set -uo pipefail

say()  { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*"; }

host_pidns="$(readlink /proc/self/ns/pid 2>/dev/null || true)"

# 0 = normal host process; 1 = lives inside a container (leave it alone).
is_host_process() {
  local pid="$1" ns
  ns="$(readlink "/proc/$pid/ns/pid" 2>/dev/null || true)"
  # A different PID namespace => containerized.
  if [ -n "$host_pidns" ] && [ -n "$ns" ] && [ "$ns" != "$host_pidns" ]; then
    return 1
  fi
  # A docker/containerd/podman/lxc/kube cgroup => containerized (also covers
  # `--pid=host` containers that share our PID namespace).
  if grep -qaE 'docker|containerd|libpod|/lxc|kubepods' "/proc/$pid/cgroup" 2>/dev/null; then
    return 1
  fi
  return 0
}

# Host-side neoism pids by full command line, excluding self + containers.
# `pgrep -x neoism` catches the GUI (installed or `target/*/neoism`, both have
# comm "neoism"); the daemon/agent need `-f`.
collect_host_pids() {
  { pgrep -x neoism; pgrep -f neoism-workspace-daemon; pgrep -f neoism-agent; } \
    2>/dev/null | sort -un | while read -r pid; do
      [ "$pid" = "$$" ] && continue
      is_host_process "$pid" && printf '%s\n' "$pid"
    done
}

pids="$(collect_host_pids | tr '\n' ' ')"
if [ -z "${pids// /}" ]; then
  say "no host-side neoism processes running"
else
  say "SIGTERM: $pids"
  # shellcheck disable=SC2086
  kill -TERM $pids 2>/dev/null || true
  sleep 1
  pids="$(collect_host_pids | tr '\n' ' ')"   # re-collect; some exited
  if [ -n "${pids// /}" ]; then
    say "SIGKILL: $pids"
    # shellcheck disable=SC2086
    kill -KILL $pids 2>/dev/null || true
  fi
fi

# Report. Containerized neoism daemons are NOT survivors — they're intentional.
sleep 1
host_left="$(collect_host_pids | tr '\n' ' ')"
if [ -n "${host_left// /}" ]; then
  warn "still alive on host: $host_left"
  exit 1
fi
say "host clean — no host-side neoism processes remain"

container_left="$(pgrep -f neoism-workspace-daemon 2>/dev/null \
  | while read -r p; do is_host_process "$p" || printf '%s ' "$p"; done)"
if [ -n "${container_left// /}" ]; then
  say "(left untouched: containerized daemon pid(s) ${container_left}— managed by their container)"
fi
exit 0
