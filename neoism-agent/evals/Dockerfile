# The runner records the final image ID and the native binary's SHA-256.
ARG BASE_IMAGE=ubuntu@sha256:534baea6a22c03a63003dbc8dbe78fe34bc0d7e595d9a9dc9834884ff530eb55
FROM ${BASE_IMAGE}
ARG CLAUDE_VERSION=2.1.292
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates git python3 nodejs npm ripgrep curl \
    libgbm1 libxkbcommon0 zlib1g \
    && rm -rf /var/lib/apt/lists/* \
    && npm install --global @anthropic-ai/claude-code@${CLAUDE_VERSION} \
    && npm cache clean --force
COPY neoism-agent /usr/local/bin/neoism-agent
COPY worker.py verify.py candidate_client.py candidate_worker.py /opt/eval/
RUN neoism-agent --version && claude --version \
    && mkdir /workspace /verifier \
    && chmod 755 /usr/local/bin/neoism-agent
ENV HOME=/tmp/home PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 \
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
    TOKIO_WORKER_THREADS=2 RAYON_NUM_THREADS=2
WORKDIR /workspace
USER 1000:1000
CMD ["python3", "/opt/eval/worker.py", "preflight"]
