# Docker release hosting validation

Validated the published Linux amd64 `ghcr.io/parkers0405/neoism-daemon:0.7.150` image, digest `sha256:6d619715c051f9b0476fe06d530df607b3d51ceabe5d55df2feb571aaf23efac`, on November 13, 2026. No local application binaries were substituted into the image.

## Isolated deployment

Container `neoism-release-test-0150` runs the daemon and its managed Agent with two CPUs, a 2 GiB memory limit, and port `9876` published only as `127.0.0.1:19876`. Four dedicated named volumes preserve `/var/lib/neoism/{state,data,config,workspaces}`. No host home, repository, SSH agent, provider credentials, or Docker socket is mounted. The daemon declares `/var/lib/neoism/workspaces` explicitly at startup. The container remains running for manual testing.

Authentication is explicitly enabled with `NEOISM_REQUIRE_AUTH=1` and a generated test credential. Setting `NEOISM_DAEMON_TOKEN` alone does not enable mandatory authentication. Public deployments also need secure ingress; the localhost test uses plain WebSocket/HTTP.

## Automated checks

`scripts/test-docker-release.mjs` uses the published image and the real workspace handshake, then addresses the Agent through `/agent/workspaces/{workspace_id}/v2/...`. The old unscoped `/agent/v2/...` endpoint is not the remote workspace contract.

All of these checks passed:

- Daemon health, authenticated workspace resolution, and Agent readiness through the daemon.
- Anonymous Agent access rejected and hosted global configuration access forbidden.
- Live SSE events through the workspace-scoped proxy.
- Existing Agent stub-model loop invoking the real read tool on a container file, without paid model calls.
- Disconnect/rejoin preserving the same conversation.
- Graceful stop/start preserving workspace files and history.
- Forced process kill preserving completed history.
- A permission-blocked active turn resuming after graceful container shutdown.
- A permission-blocked active turn resuming after forced process kill.
- Explicit Stop preventing that turn from resuming after restart.

Recovery tests temporarily configure approval behavior using the operator credential on the container-private Agent loopback endpoint and restore the original configuration afterward. They do not bypass the hosted workspace API's management restrictions.

Rerun against the retained test container:

```sh
node scripts/test-docker-release.mjs --existing --recovery
```

Without `--existing`, the script creates its own container. Override `NEOISM_TEST_IMAGE`, `NEOISM_TEST_CONTAINER`, and `NEOISM_TEST_PORT` to test another release without replacing the retained instance. Tests intentionally restart/kill only their selected test container; do not point the script at a production container.

## Real desktop checks

Added `Docker Release 0.7.150` through the running Rune desktop's Servers > Add Server > Join form at `ws://127.0.0.1:19876/session`. The joined workspace opened at `/var/lib/neoism/workspaces`. A terminal command confirmed `uid=999(neoism)` and read the container-only fixture. The Agent sidebar displayed the same test conversations created through the hosted API, and the timeline displayed their saved tool result. Restarting Docker while joined was followed by successful automatic reconnection and another stub-model reply submitted from the GUI. The original desktop workspace was restored afterward; the test server and its workspace tab remain available.

Retrieve the isolated test credential locally if needed:

```sh
docker exec neoism-release-test-0150 printenv NEOISM_DAEMON_TOKEN
```

## Limits

This validates a single isolated native instance and the actual desktop connection path, not Azure infrastructure, TLS/reverse-proxy behavior, release-to-release migration, paid provider connectivity, tenant-scale load, arbitrary subprocess restoration, or exactly-once external effects. The daemon health endpoint is a liveness check; the harness separately checks workspace resolution and Agent readiness. A comprehensive host-facing safe-to-stop signal was not established by these tests.