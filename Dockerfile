# Build the standalone agent GUI without requiring Node in the runtime image.
FROM node:22-bookworm-slim AS agent-gui
WORKDIR /src
COPY neoism-agent/sdk/typescript/ neoism-agent/sdk/typescript/
COPY scripts/package-agent-gui.mjs scripts/package-agent-gui.mjs
RUN npm --prefix neoism-agent/sdk/typescript ci \
    && npm --prefix neoism-agent/sdk/typescript run gui:build \
    && node scripts/package-agent-gui.mjs neoism-agent/sdk/typescript/packages/gui/dist /gui

FROM node:22-bookworm AS builder

# CPU fonts and Agent computer-use libraries; no frontend GPU renderer.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential clang git curl pkg-config cmake python3 ninja-build libfontconfig1-dev \
        libgbm-dev libxkbcommon-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .
# Blaze installs the pinned, checksum-verified compiler and its toolchain.
# Docker already isolates the build; nested user namespaces are unavailable
# under the default BuildKit seccomp policy.
RUN CARGO_BUILD_JOBS=2 BLAZE_SANDBOX=off \
    bash scripts/blaze.sh build --locked --release -p neoism-workspace-daemon -p neoism-agent

FROM debian:bookworm-slim AS runtime

# bash: bare `sh` gets no block-prompt integration, so commands look
# perpetually running to joined clients. Fontconfig (including its FreeType
# dependency) is used by daemon configuration/font discovery even on a headless host.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        git \
        libfontconfig1 \
        libgbm1 \
        libxkbcommon0 \
        openssh-client \
        tini \
    && rm -rf /var/lib/apt/lists/*

# neovim: remote editor panes spawn nvim ON the daemon host — without it
# joined clients get a blank editor. Debian's packaged 0.7.x is too old
# for the daemon's ide init commands (`vim/_meta.lua` errors abort the
# spawn), so install the current stable release tarball instead.
RUN curl -fsSL -o /tmp/nvim.tar.gz \
        https://github.com/neovim/neovim/releases/download/v0.12.3/nvim-linux-x86_64.tar.gz \
    && tar -xzf /tmp/nvim.tar.gz -C /opt \
    && ln -s /opt/nvim-linux-x86_64/bin/nvim /usr/local/bin/nvim \
    && rm /tmp/nvim.tar.gz

RUN useradd --system --create-home --home-dir /var/lib/neoism --shell /usr/sbin/nologin neoism \
    && mkdir -p /var/lib/neoism/state /var/lib/neoism/workspaces /var/lib/neoism/data /var/lib/neoism/config \
    && chown -R neoism:neoism /var/lib/neoism

COPY --from=builder /src/target/release/neoism-workspace-daemon /usr/local/bin/neoism-workspace-daemon
COPY --from=agent-gui /gui /usr/local/bin/web/agent-gui
COPY --from=builder /src/target/release/neoism-agent /usr/local/bin/neoism-agent
# Exercise the runtime loader in the slim image before publishing it. A builder
# dependency alone does not provide the shared libraries needed at startup.
RUN neoism-workspace-daemon --version && neoism-agent --version
USER neoism
ENV NEOISM_DAEMON_ADDR=0.0.0.0:9876 \
    NEOISM_DAEMON_DATA_DIR=/var/lib/neoism/data \
    NEOISM_CONFIG_DIR=/var/lib/neoism/config \
    NEOISM_WORKSPACES_DIR=/var/lib/neoism/workspaces \
    RUST_LOG=info,neoism_workspace_daemon=debug \
    TERM=xterm-256color \
    SHELL=/bin/bash

EXPOSE 9876
VOLUME ["/var/lib/neoism/state", "/var/lib/neoism/workspaces", "/var/lib/neoism/data", "/var/lib/neoism/config"]
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -fsS http://127.0.0.1:9876/health || exit 1

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["neoism-workspace-daemon", "--state-dir", "/var/lib/neoism/state"]
