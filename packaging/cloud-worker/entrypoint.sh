#!/bin/sh
set -eu
umask 077
fail() { printf '%s\n' "worker startup: $*" >&2; exit 1; }
[ "$#" -eq 0 ] || fail 'arguments are not accepted; this image only serves a workspace worker'
: "${NEOISM_AGENT_WORKER_BOOTSTRAP:?controller must supply a bootstrap path}"
: "${NEOISM_AGENT_WORKER_VERIFICATION_KEY_FILE:?controller must supply a public verification-key path}"

# The CLI performs authoritative binding validation. These checks catch mount errors
# early without logging the bootstrap contents or public verification key.
for file in "$NEOISM_AGENT_WORKER_BOOTSTRAP" "$NEOISM_AGENT_WORKER_VERIFICATION_KEY_FILE"; do
    case "$file" in /*) ;; *) fail 'bootstrap/key paths must be absolute' ;; esac
    [ -f "$file" ] && [ -r "$file" ] || fail 'bootstrap/key must be readable regular files'
    resolved=$(realpath -e "$file") || fail 'cannot resolve controller file'
    case "$resolved" in /workspace|/workspace/*) fail 'controller files must be outside /workspace' ;; esac
    [ ! -w "$file" ] || fail 'controller files must be read-only to the worker'
done
[ "$(wc -c < "$NEOISM_AGENT_WORKER_VERIFICATION_KEY_FILE")" -eq 32 ] || fail 'public verification key must contain exactly 32 raw public bytes'
[ -d /workspace ] && [ "$(realpath -e /workspace)" = /workspace ] || fail '/workspace must be an existing canonical directory'
[ -w /workspace ] || fail '/workspace must be writable by uid 10001'
jq -e --argjson now "$(date +%s)" '
    .version == 1 and .root == "/workspace" and
    ([.tenantId, .workspaceId, .runtimeId] | all(type == "string" and test("\\S"))) and
    (.runtimeGeneration | type == "number" and . > 0 and floor == .) and
    (.expiresAt | type == "number" and . > $now and floor == .)
' "$NEOISM_AGENT_WORKER_BOOTSTRAP" >/dev/null || fail 'invalid or expired bootstrap'

# Keep persistence and provider credentials out of the working tree. These path
# guards are not OS isolation between agents sharing the workspace computer.
[ "$HOME" = /var/lib/neoism/home ] || fail 'unexpected HOME'
[ "$XDG_CONFIG_HOME" = /var/lib/neoism/config ] || fail 'unexpected config path'
[ "$XDG_STATE_HOME" = /var/lib/neoism/state ] || fail 'unexpected state path'
[ "$XDG_CACHE_HOME" = /var/lib/neoism/cache ] || fail 'unexpected cache path'
[ "$XDG_DATA_HOME" = /var/lib/neoism/data ] || fail 'unexpected data path'
[ "$NEOISM_AGENT_STATE_DIR" = /var/lib/neoism/state/neoism-agent ] || fail 'unexpected Agent state path'
[ "$NEOISM_AGENT_AUTH_PATH" = /var/lib/neoism/state/neoism-agent/auth.json ] || fail 'unexpected provider-auth path'
for dir in "$HOME" "$XDG_CONFIG_HOME/agent" "$NEOISM_AGENT_STATE_DIR" "$XDG_CACHE_HOME" "$XDG_DATA_HOME"; do
    mkdir -p "$dir" || fail 'cannot create persistent directories'
    case "$(realpath -e "$dir")" in /var/lib/neoism/*) ;; *) fail 'persistent directory escapes state root' ;; esac
    [ -w "$dir" ] || fail 'persistent directories must be writable by uid 10001'
done

# No web launcher, GUI assets, alternate local server or authentication bypass.
exec neoism-agent serve --hostname 0.0.0.0 --port 4096 \
    --worker-bootstrap "$NEOISM_AGENT_WORKER_BOOTSTRAP" \
    --worker-verification-key-file "$NEOISM_AGENT_WORKER_VERIFICATION_KEY_FILE"
