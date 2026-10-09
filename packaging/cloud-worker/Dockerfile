# Agent-only runtime: supply Linux binaries built by the existing release workflow.
# All COPY paths are relative to the repository-root build context.
FROM debian:bookworm-slim AS agent-worker
ARG AGENT_BINARY=worker-artifacts/neoism-agent
ARG LUA_RUNNER_BINARY=worker-artifacts/neoism-agent-lua-runner

# GBM/xkbcommon are linked by Agent computer-use backends, even headless.
# jq is used for bootstrap/readiness checks; no renderer, daemon, GUI or nvim.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash ca-certificates curl git jq libgbm1 libxkbcommon0 openssh-client tini \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 neoism \
    && useradd --uid 10001 --gid 10001 --home-dir /var/lib/neoism/home --no-create-home --shell /bin/bash neoism \
    && mkdir -p /workspace /var/lib/neoism/home /var/lib/neoism/config /var/lib/neoism/state /var/lib/neoism/cache /var/lib/neoism/data \
    && chown -R 10001:10001 /workspace /var/lib/neoism

COPY ${AGENT_BINARY} /usr/local/bin/neoism-agent
COPY ${LUA_RUNNER_BINARY} /usr/local/bin/neoism-agent-lua-runner
COPY packaging/cloud-worker/entrypoint.sh /usr/local/bin/worker-entrypoint
COPY packaging/cloud-worker/healthcheck.sh /usr/local/bin/worker-healthcheck
RUN chmod 0755 /usr/local/bin/neoism-agent /usr/local/bin/neoism-agent-lua-runner /usr/local/bin/worker-entrypoint /usr/local/bin/worker-healthcheck

ENV HOME=/var/lib/neoism/home \
    XDG_CONFIG_HOME=/var/lib/neoism/config \
    XDG_STATE_HOME=/var/lib/neoism/state \
    XDG_CACHE_HOME=/var/lib/neoism/cache \
    XDG_DATA_HOME=/var/lib/neoism/data \
    NEOISM_AGENT_STATE_DIR=/var/lib/neoism/state/neoism-agent \
    NEOISM_AGENT_AUTH_PATH=/var/lib/neoism/state/neoism-agent/auth.json \
    TERM=xterm-256color \
    SHELL=/bin/bash
USER 10001:10001
WORKDIR /workspace
EXPOSE 4096
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 CMD ["/usr/local/bin/worker-healthcheck"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/worker-entrypoint"]
