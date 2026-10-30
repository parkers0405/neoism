#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixture="$(mktemp -d)"
previous_pid=""
cleanup() {
  if [[ -n "$previous_pid" ]]; then
    kill -KILL "$previous_pid" 2>/dev/null || true
    wait "$previous_pid" 2>/dev/null || true
  fi
  rm -rf "$fixture"
}
trap cleanup EXIT
payload="$fixture/payload"
prefix="$fixture/installed"
mkdir -p "$payload/web/agent-gui" "$payload/licenses" "$prefix/bin"
for binary in neoism neoism-workspace-daemon neoism-agent; do
  printf '#!/bin/sh\necho migrated-%s\n' "$binary" > "$payload/$binary"
  chmod +x "$payload/$binary"
done
for resource in libneoism_gpuix_core.so libghostty-vt.so LICENSE.ghostty LICENSE NOTICE; do printf '%s\n' "$resource" > "$payload/$resource"; done
printf 'font license' > "$payload/licenses/font.txt"
printf '<html>Agent GUI</html>' > "$payload/web/agent-gui/index.html"
printf 'previous version\n' > "$prefix/bin/neoism"
# Incomplete bundles must leave the installed executable intact.
mv "$payload/libghostty-vt.so" "$fixture/ghostty"
if bash "$repo_root/scripts/install-gpuix-bundle.sh" "$payload" "$prefix" > /dev/null 2>&1; then echo 'Incomplete bundle was accepted' >&2; exit 1; fi
[[ "$(cat "$prefix/bin/neoism")" == 'previous version' ]]
mv "$fixture/ghostty" "$payload/libghostty-vt.so"
bash "$repo_root/scripts/install-gpuix-bundle.sh" "$payload" "$prefix"
[[ "$("$prefix/bin/neoism")" == migrated-neoism ]]
[[ -f "$prefix/bin/libneoism_gpuix_core.so" && -f "$prefix/bin/libghostty-vt.so" && -f "$prefix/bin/web/agent-gui/index.html" ]]
# Installing another version replaces the complete GUI tree instead of retaining stale assets.
printf stale > "$prefix/bin/web/agent-gui/stale.js"
bash "$repo_root/scripts/install-gpuix-bundle.sh" "$payload" "$prefix"
[[ ! -e "$prefix/bin/web/agent-gui/stale.js" ]]
# A running daemon/window must retain its executable-relative resources and
# be able to spawn itself after an upgrade. Use a real executable, as the app is.
cat > "$fixture/previous.c" <<'C'
#include <limits.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
int main(int argc, char **argv) {
  if (argc == 2 && strcmp(argv[1], "--child") == 0) {
    puts("previous-process-respawned");
    return 0;
  }
  if (argc != 3) return 1;
  FILE *ready = fopen(argv[1], "w");
  if (!ready) return 2;
  fclose(ready);
  while (access(argv[2], F_OK) != 0) usleep(1000);
  char executable[PATH_MAX];
  ssize_t length = readlink("/proc/self/exe", executable, sizeof(executable) - 1);
  if (length < 0) return 3;
  executable[length] = 0;
  execl(executable, executable, "--child", (char *)0);
  return 4;
}
C
cc "$fixture/previous.c" -o "$prefix/bin/neoism.next"
mv "$prefix/bin/neoism.next" "$prefix/bin/neoism"
"$prefix/bin/neoism" "$fixture/ready" "$fixture/continue" > "$fixture/previous.log" &
previous_pid=$!
for ((attempt=0; attempt<200; attempt++)); do
  if [[ -f "$fixture/ready" ]]; then break; fi
  sleep 0.01
done
[[ -f "$fixture/ready" ]]
bash "$repo_root/scripts/install-gpuix-bundle.sh" "$payload" "$prefix"
previous_executable="$(readlink "/proc/$previous_pid/exe")"
[[ -x "$previous_executable" && -f "$(dirname "$previous_executable")/web/agent-gui/index.html" ]]
[[ "$("$prefix/bin/neoism")" == migrated-neoism ]]
touch "$fixture/continue"
wait "$previous_pid"
previous_pid=""
[[ "$(cat "$fixture/previous.log")" == previous-process-respawned ]]
# Once that process exits, the next successful upgrade prunes the old bundle.
bash "$repo_root/scripts/install-gpuix-bundle.sh" "$payload" "$prefix"
[[ ! -e "$previous_executable" ]]
echo 'GPUIX bundle installation checks passed.'
