# Workspace worker packaging

This is a thin **Agent-only container intended to run inside one isolated VM per workspace**. The VM contains the whole standalone Neoism Agent runtime; ordinary file, shell, Git, process, MCP and plugin operations run locally there, subject to Agent permissions and installed capabilities. Tools do not allocate machines. There is no per-tool cloud sandbox or fallback to the application host. The application manages VM lifecycle through the provider-neutral [cloud-runtime library](../../neoism-cloud-runtime/README.md), outside the tool runtime.

This package does not deploy a cloud backend, application gateway, accounts or billing service. It is not a shipped or tested Boat deployment. The workspace daemon is an optional later addition, not part of this image. See [hosted architecture](../../neoism-agent/docs/hosted-control-plane.md).

## Build from existing artifacts

Run these commands from the repository root after obtaining Linux `neoism-agent` and `neoism-agent-lua-runner` artifacts from the existing release workflow. This Dockerfile does not invoke Cargo, build the workspace, or copy GUI assets. The binaries must include the paired worker startup flags; an older artifact will fail startup, not start a local server instead.

```sh
# Stage existing artifacts: the root .dockerignore excludes target/.
mkdir -p worker-artifacts
cp target/release/neoism-agent target/release/neoism-agent-lua-runner worker-artifacts/
docker build --file packaging/cloud-worker/Dockerfile --target agent-worker \
  --build-arg AGENT_BINARY=worker-artifacts/neoism-agent \
  --build-arg LUA_RUNNER_BINARY=worker-artifacts/neoism-agent-lua-runner \
  --tag neoism-agent-worker:local .
```

Both build arguments are **file paths relative to the repository-root build context**, not host absolute paths. They may point to another artifacts directory in that context. Check the root `.dockerignore` permits those files. Match Docker's target architecture to the binaries (e.g. add `--platform linux/amd64` for x86-64). Build artifacts against glibc compatible with Debian Bookworm (glibc 2.36); host-built binaries using newer symbols will not load in this image. Never put bootstrap files, signing keys, provider credentials or application secrets in the build context or image layers.

Runtime packages are Bash, CA certificates, curl, Git, jq, OpenSSH client, tini, GBM and xkbcommon plus their Debian dependencies. GBM/xkbcommon are Agent computer-use link dependencies, not a promise of a display server. No daemon renderer/fontconfig, Neovim, GUI bundle, desktop, browser, language server, Node, Python, compiler toolchain or Bubblewrap is supplied. Add only the tools your workspace needs in a derived image. A linked-library audit and loader smoke test of your actual artifacts are required before publishing; dependencies can vary with artifact features. For example:

```sh
docker run --rm --entrypoint /bin/sh neoism-agent-worker:local -c \
  'ldd /usr/local/bin/neoism-agent; ldd /usr/local/bin/neoism-agent-lua-runner; neoism-agent --version; neoism-agent-lua-runner --version; neoism-agent serve --help'
```

These are verification instructions for the **portable release image/artifact combination**, which has not been validated by the development-image run. A real development image build/run and manager/Docker end-to-end flow have passed; see [DEVELOPMENT.md](DEVELOPMENT.md) for the distinct image and opt-in integration command. Plugin-internal isolation requirements remain separate from whole-VM isolation; plugins requiring unavailable runtimes or OS facilities fail rather than gaining a fallback on the host.

## Controller provisioning

The controller must authorize the application tenant/workspace, allocate or resume its VM, attach workspace/state storage, then provision a public bootstrap and public verifier as two **read-only files outside `/workspace`** through an authenticated provisioning channel. The runtime identity and generation must agree with the controller's durable machine binding. Bootstrap is loaded at startup, not through `/config`.

| Bootstrap JSON field | Contract |
|---|---|
| `version` | `1` |
| `tenantId`, `workspaceId`, `runtimeId` | Nonblank controller-owned identities |
| `runtimeGeneration` | Positive integer; fence credentials to the current generation |
| `root` | Existing absolute canonical directory; this image requires `/workspace` |
| `expiresAt` | Future Unix seconds computed by the controller for this runtime binding |

The separate verification file contains **exactly 32 raw Ed25519 public-key bytes**, not hex/base64 or a PEM document. The controller creates a fresh private signing seed of exactly 32 bytes per runtime generation and derives the public verifier with `WorkspaceWorkerSigningKey::new(seed)?.verification_key().as_bytes()`. The private seed stays in the application's controller account/secret store: **never send it to the VM, mount it into the container, put it under workspace/state storage, or expose it to SDK clients**. The public verifier is not secret; its read-only, outside-workspace placement protects immutable controller identity, not confidentiality. Network isolation, application authorization and fencing retired VMs remain essential. Provider/model credentials belong in private state or a controlled credential service, never in the workspace tree or image.

For a disposable smoke-test setup, run the following **on the controller**, not inside the worker VM. The supplied [`provision.rs`](provision.rs) example uses the service API to derive the public verifier and writes only public `bootstrap.json` and `verification.key` outputs, with a computed one-hour binding expiry. It is a separate controller example, not a key-generation subcommand of `neoism-agent`. It needs Rust/Cargo, OpenSSL and the service API's Ed25519 implementation; no Python cryptography dependency is assumed. In production use controller-authorized IDs and the current generation from the durable binding, not a constant across replacements.

```sh
# Repository-root path on the controller; private data is outside the build context.
repo=$(pwd)
controller_dir=$(mktemp -d)
chmod 0700 "$controller_dir"
mkdir -m 0700 "$controller_dir/public" "$controller_dir/helper" "$controller_dir/helper/src"
umask 077
openssl rand -out "$controller_dir/signing.seed" 32
cp packaging/cloud-worker/provision.rs "$controller_dir/helper/src/main.rs"
cat > "$controller_dir/helper/Cargo.toml" <<EOF
[package]
name = "worker-provision"
version = "0.1.0"
edition = "2021"
[dependencies]
neoism-agent-service-api = { path = "$repo/neoism-agent/crates/neoism-agent-service-api" }
serde_json = "1"
EOF
cargo run --manifest-path "$controller_dir/helper/Cargo.toml" -- \
  "$controller_dir/signing.seed" "$controller_dir/public" \
  tenant-example workspace-example runtime-example 1
```

This is a documented controller helper invocation, not a build performed by this packaging change. Keep the private seed in the controller's protected secret store for token issuance. Transfer **only** the two public output files to the worker VM's `/srv/neoism-worker/control/` directory using your authenticated provisioning channel. Then, on the worker VM:

```sh
export WORKSPACE_DIR=/srv/neoism-worker/workspace
export STATE_DIR=/srv/neoism-worker/state
export WORKER_BOOTSTRAP_FILE=/srv/neoism-worker/control/bootstrap.json
export WORKER_VERIFICATION_KEY_FILE=/srv/neoism-worker/control/verification.key
sudo install -d -o 10001 -g 10001 -m 0700 "$WORKSPACE_DIR" "$STATE_DIR"
# The provisioning channel must create these public files before this step.
sudo chown 10001:10001 "$WORKER_BOOTSTRAP_FILE" "$WORKER_VERIFICATION_KEY_FILE"
sudo chmod 0400 "$WORKER_BOOTSTRAP_FILE" "$WORKER_VERIFICATION_KEY_FILE"
docker compose -f packaging/cloud-worker/compose.yaml config
docker compose -f packaging/cloud-worker/compose.yaml up -d
curl --fail http://127.0.0.1:4096/v2/health
```

Do not commit generated files. Pre-create bind sources; otherwise Docker may create a directory where a file was expected. Mounts must be readable by uid/gid `10001:10001` and not writable by the runtime. Entrypoint validates required paths, key size, bootstrap identity/expiry and writable directories, then `exec`s the stock CLI under tini. SIGTERM is forwarded; Compose allows 30 seconds before forced termination. No `--web` is used: with no GUI assets, this is API-only.

The equivalent stock CLI invocation is:

```sh
neoism-agent serve --hostname 0.0.0.0 --port 4096 \
  --worker-bootstrap /run/neoism-worker/bootstrap.json \
  --worker-verification-key-file /run/neoism-worker/verification.key
```

The flags require each other. Alternatively supply both `NEOISM_AGENT_WORKER_BOOTSTRAP` and `NEOISM_AGENT_WORKER_VERIFICATION_KEY_FILE` and invoke `neoism-agent serve`. Neither is a bearer token. Do not configure trusted remote bypasses or desktop/operator tokens for this deployment.

## Persistence and connectivity

| Path / variable | Purpose |
|---|---|
| `/workspace` | Writable working tree; bootstrap root and credential directory prefix |
| `/var/lib/neoism` | Durable, private single-workspace state mount; never share across tenants |
| `HOME=/var/lib/neoism/home` | Runtime home |
| `XDG_CONFIG_HOME=/var/lib/neoism/config` | Agent user configuration under `agent/` |
| `XDG_STATE_HOME=/var/lib/neoism/state` | State base |
| `NEOISM_AGENT_STATE_DIR=/var/lib/neoism/state/neoism-agent` | Explicit Agent session/state directory |
| `NEOISM_AGENT_AUTH_PATH=/var/lib/neoism/state/neoism-agent/auth.json` | explicit provider authentication file; never a controller signing-seed location |
| `XDG_CACHE_HOME=/var/lib/neoism/cache` | Cache |
| `XDG_DATA_HOME=/var/lib/neoism/data` | Data base |
| `/tmp`, `/run` | Bounded ephemeral tmpfs; controller files are nested read-only mounts |

Compose uses a read-only root filesystem, a nonroot user, no capabilities, no privilege escalation, 256 PIDs, 4 GiB memory and two CPUs. It does not mount a Docker socket or host devices. Workspace and state are writable; the root filesystem is read-only by design. These container limits are defense in depth, **not** a replacement for the dedicated VM. Rootless/user-namespace Docker can require different host ownership for bind mounts.

The example publishes only VM loopback port 4096. A separately managed gateway/proxy on the VM can reach that port; for a remote gateway use an authenticated private connection/tunnel, or deliberately bind a private VM interface with firewall restrictions. Public clients connect to the application gateway over TLS, never to an exposed plain HTTP worker port. The gateway authorizes application users, routes only to the current runtime generation and attaches short-lived worker credentials. TLS termination/certificates, VM firewalls, allowed egress destinations and gateway routing are controller responsibilities. The Compose bridge allows egress but does not implement an allowlist.

## Issue credentials in the controller

Use `neoism-agent-service-api` at the runtime's version. This controller API takes the **private 32-byte signing seed**, controller-authorized scopes/quotas and identity; it does not create users or decide entitlements. Only the derived public verifier reaches the VM. Resolve the canonical directory in the **worker filesystem namespace**, not against a controller host directory. The directory must exist within `/workspace`.

```rust,no_run
use neoism_agent_service_api::{
    ActorType, ServiceError, TenantQuotas, WorkspaceWorkerBootstrap,
    WorkspaceWorkerCredentialClaims, WorkspaceWorkerSigningKey,
};
use neoism_agent_service_api::workspace_worker::MAX_WORKSPACE_WORKER_TOKEN_TTL_SECS;
use std::{path::PathBuf, time::{SystemTime, UNIX_EPOCH}};

fn issue_worker_token(
    private_seed: &[u8],
    binding: &WorkspaceWorkerBootstrap,
    subject: String,
    actor_type: ActorType,
    authorized_scopes: Vec<String>,
    authorized_quotas: TenantQuotas,
) -> Result<String, ServiceError> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)
        .map_err(|_| ServiceError::new("clock before epoch"))?.as_secs() as i64;
    let claims = WorkspaceWorkerCredentialClaims {
        version: 1,
        tenant_id: binding.tenant_id.clone(),
        workspace_id: binding.workspace_id.clone(),
        runtime_id: binding.runtime_id.clone(),
        runtime_generation: binding.runtime_generation,
        subject,
        actor_type,
        directory_prefix: PathBuf::from("/workspace"),
        scopes: authorized_scopes,
        quotas: authorized_quotas,
        issued_at: now,
        expires_at: (now + MAX_WORKSPACE_WORKER_TOKEN_TTL_SECS).min(binding.expires_at),
    };
    WorkspaceWorkerSigningKey::new(private_seed)?.issue(&claims)
}
```

The maximum token lifetime is **300 seconds (five minutes)**. Tokens cannot outlive the bootstrap, and an expired bootstrap cannot issue a valid token. `issue` validates claim shape/TTL; the controller must authorize the actor and scopes and check the binding remains current. The worker independently verifies signature, identity, generation, time and canonical directory admission. Send the returned token in `Authorization: Bearer …`, and rotate credentials/reconnect streams before expiry. No local credential fallback is permitted.

Issue `agent:read` for ordinary read access, `agent:use` for conversation/task execution, or `workspace:admin` for configuration and scoped connector management. Missing or unknown scopes grant no API operations. These permissions do not isolate agents from each other's files on the shared workspace computer. State/secret-path guards are application admission checks, not OS isolation or a guarantee that same-user processes cannot access runtime files. All agents for this workspace share the same computer. The controller signing seed is absent from that computer entirely.

## Readiness and remaining integration

`GET /v2/health` is unauthenticated. The packaged check requires `healthy: true`, `deployment: "workspace-worker"`, `executionAvailable: true` and an unexpired bootstrap. Server health is expiry-aware; the explicit bootstrap expiry check is defense in depth. A healthy HTTP process is not proof that its immutable worker binding is still valid. Changing a mounted file does not refresh the in-memory binding; restart with controller-provisioned binding files. Do not edit files in place under a running instance and treat a health pass as proof of new identity.

Before declaring a deployment ready, the controller must also check the expected identity/generation with an authenticated scoped request, storage availability and required tools/credentials. The server becomes unready and exits when its immutable binding expires, and handles SIGTERM with bounded worker shutdown. SSE streams close when their credentials expire; renew and reconnect rather than assuming an established stream has unlimited validity. Docker marks unhealthy but does not restart unhealthy containers by itself; the controller must drain routing and reconcile/restart explicitly.

The provider-neutral launch and connection flow is implemented in `neoism-cloud-host`: it prepares public launch data, starts the worker through the selected provider, authenticates and verifies worker identity/root/generation, and issues bounded connection grants. `neoism-cloud-runtime` supplies the v2 provider bridge server/client and durable lifecycle registry. A real Docker development provider and end-to-end test exercise repository/worktree creation, multiple sessions, controller restart, stop/resume, persistent state and generation replacement. Docker is explicitly container isolation and does not enforce the requested disk budget; it is not presented as a production VM provider.

A production VM integration supplies a compatible `RuntimeProvider` directly or implements the versioned bridge contract. It must enforce its advertised isolation/resource limits and retain logical workspace/state storage when compute is replaced. No vendor is required by Neoism's core. Provider-specific networking, image distribution, storage backups and infrastructure credentials remain deployment integration responsibilities.

Application/deployment responsibilities: supply the cloud account and model/connector credentials, authorize workspace membership and actor scopes, retain controller signing seeds outside workers, operate the application gateway/TLS and token-issuance policy, and handle subscriptions/VM billing. Providers implementing a custom backend must honor the documented lifecycle and isolation contract.

The standalone worker's public-key bootstrap, authenticated session creation and expiry shutdown are covered by a real child-process integration test. A real development image build/run and brokered Docker end-to-end flow have also passed. This validates the container reference backend, not an unrelated production VM adapter or public application gateway.
