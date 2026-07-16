FROM rust:1.92-bookworm AS builder

# The daemon links the sugarloaf renderer: shaderc-sys builds shaderc from
# source (cmake + python3), and sugarloaf's build.rs shells out to a GLSL →
# SPIR-V compiler (glslangValidator from glslang-tools).
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        cmake python3 ninja-build glslang-tools \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .
# Capped parallelism: an uncapped release build inside the container
# saturates every host core (opt-level=3, codegen-units=1 on the heavy
# deps) and starves the desktop the developer is actively using.
RUN CARGO_BUILD_JOBS=4 cargo build --release -p neoism-workspace-daemon

FROM debian:bookworm-slim AS runtime

# bash: bare `sh` gets no block-prompt integration, so commands look
# perpetually running to joined clients.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        git \
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
