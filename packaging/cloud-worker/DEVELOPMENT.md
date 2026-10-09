# Local development worker image

This image exercises the same standalone workspace worker as a VM deployment. Docker containers are an explicit development backend, not a claim of VM-grade isolation. CPU and memory controls can be enforced; ordinary Docker workspace/state volumes do not enforce the requested disk budget.

## Build

Use Linux Agent and Lua-runner executables from the existing development build, then run:

```sh
bash packaging/cloud-worker/build-development.sh neoism-agent-worker:development
```

The script stages only those executables, their `ldd` runtime library closure, and the worker startup scripts in a temporary build context. It does not send source files, configuration, credentials, or workspace contents to Docker. Override `NEOISM_AGENT_BINARY` and `NEOISM_AGENT_LUA_RUNNER_BINARY` to select explicit artifacts.

The development image includes host-compatible dynamic libraries because a native debug build may require a newer glibc than the portable release image. It is not a reproducible production release artifact or a replacement for the release-image build. The portable worker image remains `Dockerfile`.

## Provider construction and opt-in integration

The [workspace host construction guide](../../neoism-cloud-host/README.md#construction-and-authority) covers production `WorkspaceHost::new` and development `WorkspaceHost::new_development`. The reference Linux adapter is constructed with `DockerDevelopmentProvider::new(DockerConfig { id, controller_directory, image_catalog, allow_unenforced_disk: true })`; `controller_directory` must be absolute and private, and the image catalog maps trusted operator aliases to explicit image references. `DockerDevelopmentProvider::with_cli(config, cli)` accepts an absolute trusted Docker CLI path when `/usr/bin/docker` is not appropriate. Neither CLI paths nor image references come from model arguments or guest requests.

See the complete [manager/provider wiring and real integration test](../../neoism-cloud-host/tests/workspace_end_to_end.rs). It uses `WorkspaceHost::new_development`, a separate `FileSigningKeyStore`, launch policy and development-only endpoint policy; production construction rejects these container capabilities. The [fake-CLI adapter tests](../../neoism-cloud-host/tests/docker.rs) are separate from the actual Docker test.

After building the development image above, explicitly opt into the real test from the repository root:

```sh
NEOISM_DOCKER_TEST_IMAGE=neoism-agent-worker:development cargo test -p neoism-cloud-host --test workspace_end_to_end -- --ignored --test-threads=1
```

The test requires a working Linux Docker daemon and permission to create containers and volumes. It creates its own uniquely named test deployment and cleans up its test resources; it is ignored by default. One actual manager/Docker end-to-end run has passed, exercising signed readiness/connection grants, repository and worktree creation, multiple sessions, controller restart, stop/resume, retained data and fenced replacement. This does not validate the portable release-image/artifact combination or certify production infrastructure.

## Boundaries

The development provider must be explicitly selected. Workers run nonroot with read-only image files, dropped capabilities, a process limit, and independent workspace/state volumes. Never mount the Docker socket, controller signing store, or user home into a worker. Only the public launch descriptor and verification key belong in the worker's read-only control mount.

Machine destruction retains logical workspace/state storage so replacing a runtime does not erase conversations or project files. Deleting workspace data is a separate, explicit application/infrastructure operation.

A successful container test proves the generic bootstrap, authenticated Agent API, persistence, and lifecycle flow against this reference backend. It does not certify an unrelated infrastructure adapter's VM isolation, networking, storage durability, or quota implementation. Production providers must truthfully advertise and enforce their capabilities.
