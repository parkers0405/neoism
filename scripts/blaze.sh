#!/usr/bin/env bash
# One pinned compiler and cache for application builds, tests, and CI.
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
frontend="$repo_root/neoism-frontend/gpuix"
compiler_home="$frontend/artifacts/blaze"
if [[ -n "${NEOISM_BLAZE_BIN:-}" ]]; then
  compiler="$NEOISM_BLAZE_BIN"
  if command -v cygpath >/dev/null 2>&1; then compiler="$(cygpath -u "$compiler")"; fi
else
  node "$repo_root/scripts/setup-blaze.mjs" >&2
  compiler="$compiler_home/bin/blaze"
  export RUSTUP_HOME="$compiler_home/rustup"
fi
export RUSTUP_NO_UPDATE_CHECK=1
export BLAZE_HOME="${BLAZE_HOME:-$frontend/artifacts/b}"
export BLAZE_CACHE="${BLAZE_CACHE:-$compiler_home/cache-main}"
unset RA_JIT_HOME RA_JIT_CACHE RA_JIT_REMOTE
export BLAZE_REMOTE="${BLAZE_REMOTE:-off}"
# Let the requested profile select its backend: release uses LLVM optimization.
# BLAZE_BACKEND remains an explicit caller override, as documented by Blaze.
if [[ "${1:-}" == test ]]; then
  explicit_profile=0
  for argument in "$@"; do
    case "$argument" in --release|--profile|--profile=*) explicit_profile=1 ;; esac
  done
  # Keep default development tests on one backend, including library targets.
  if (( ! explicit_profile )); then
    export BLAZE_BACKEND="${BLAZE_BACKEND:-cranelift}"
    export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-0}"
  fi
fi
# Keep completed service/index compiler workers from filling memory while the
# frontend loads. Active builds keep their worker; idle workers can be evicted.
export BLAZE_MEMORY_MAX="${BLAZE_MEMORY_MAX:-8G}"
export BLAZE_SPARE="${BLAZE_SPARE:-0}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
# Blaze's whole-program worker sizes its pools from CPU affinity, independently
# of Cargo's jobserver. Bound those pools on Linux to avoid large transient IR
# allocations exhausting memory on machines with many cores.
if [[ "$(uname -s)" == Linux ]] && command -v taskset >/dev/null 2>&1; then
  max_cpus="${NEOISM_BLAZE_CPUS:-4}"
  [[ "$max_cpus" =~ ^[0-9]+$ ]] || { echo 'NEOISM_BLAZE_CPUS must be a CPU count (0 means unlimited).' >&2; exit 2; }
  if ((max_cpus > 0)); then
    affinity="$(LC_ALL=C taskset -pc $$)"
    IFS=',' read -r -a ranges <<< "${affinity##*: }"
    selected="" count=0
    for range in "${ranges[@]}"; do
      for ((cpu=${range%-*}; cpu<=${range#*-} && count<max_cpus; cpu++)); do
        selected+="${selected:+,}$cpu"
        ((count+=1))
      done
      if ((count >= max_cpus)); then break; fi
    done
    exec taskset -c "$selected" "$compiler" "$@"
  fi
fi
exec "$compiler" "$@"
