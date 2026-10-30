#!/usr/bin/env bash
# Only provision disposable GitHub-hosted Linux VMs, never developer machines.
set -euo pipefail
if [[ "${GITHUB_ACTIONS:-}" != true || "${RUNNER_ENVIRONMENT:-}" != github-hosted || "${RUNNER_OS:-}" != Linux ]]; then
  echo 'Blaze runner provisioning applies only to GitHub-hosted Linux VMs.'
  exit 0
fi
swapfile="${RUNNER_TEMP:?}/neoism-blaze.swap"
if swapon --show=NAME --noheadings --raw | grep -Fxq -- "$swapfile"; then exit 0; fi

# These SDKs are unused by the Rust/Node/C build. Reclaim their image space for
# compiler state and swap before restoring caches or building Docker images.
sudo rm -rf /usr/local/lib/android /usr/share/dotnet /usr/share/swift \
  /opt/ghc /usr/local/.ghcup /opt/microsoft/powershell \
  /opt/hostedtoolcache/CodeQL /usr/lib/jvm /usr/local/share/boost
if command -v docker >/dev/null 2>&1; then sudo docker image prune --all --force; fi
sudo install -m 0600 /dev/null "$swapfile"
sudo fallocate -l 32G "$swapfile"
sudo mkswap "$swapfile"
sudo swapon "$swapfile"
free -h
df -h "$RUNNER_TEMP"
