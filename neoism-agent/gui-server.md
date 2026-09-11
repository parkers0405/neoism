# Standalone agent GUI server

## Commands and URL

```sh
neoism-agent web --no-open
# Verified GUI URL: http://127.0.0.1:4096/
```

Omit `--no-open` to attempt the platform browser launcher (`xdg-open`, `open`,
or Windows `rundll32`). No browser dependency is required. The URL is always
printed; browser-launch failure is nonfatal.

`web` first probes the **existing** `/v2/health`. If a healthy agent is present,
it verifies that `/` serves the standalone GUI, reports/opens that URL, and exits.
It does not open a database or start another backend in this branch. If that
agent lacks GUI assets, it returns an actionable error: install/build the GUI
and restart the existing agent supervisor with the updated binary. Do not start
another agent beside the workspace daemon: in full Neoism the workspace daemon
is the sole port-4096 supervisor; the desktop is a client.

If the local connection is refused, `web` starts the **same** standalone agent
server in the foreground with GUI assets enabled, waits for health and GUI
verification, then reports/opens the URL. Ctrl-C stops it. An occupied foreign
endpoint, failed health response, timeout, or bind failure does not trigger a
second server or another port. `--hostname` (loopback IP only for auto-start) and
`--port` (nonzero) select another local endpoint.

```sh
# Attach-only: never starts a backend, even if the endpoint is unavailable.
neoism-agent web --server http://127.0.0.1:4096 --no-open

# Explicit foreground server; require GUI assets, do not open a browser.
neoism-agent serve --web --hostname 127.0.0.1 --port 4096
```

`--server` accepts an HTTP(S) origin, not a proxy subpath. Credentials, queries,
and fragments are rejected. Probes do not follow redirects or use HTTP proxies.
Neither probe needs a bearer token. A success requires both agent health and
an HTML GUI response carrying the server's `X-Neoism-Agent-Gui: 1` marker.

## Asset installation

Build the frontend separately (see the GUI package's build scripts). The Rust
server never installs npm packages or builds frontend sources automatically.
It serves `index.html`, static assets with known inert MIME types, and the SPA
index for extensionless client routes. Missing asset files return 404.

Asset discovery order:

1. `NEOISM_AGENT_GUI_ROOT` — explicit dist directory containing `index.html`.
   An invalid override is an error, never silently replaced by another root.
2. Relative to the running executable: `agent-gui`,
   `share/neoism-agent/agent-gui`, `../share/neoism-agent/agent-gui`,
   `../share/agent-gui`.
3. Source checkout: `neoism-agent/sdk/typescript/packages/gui/dist` (relative
   to the server crate's compile-time manifest directory).

Normal `serve` and the library's standard `listen()` also discover assets
without requiring `--web`. If no assets exist, they remain API-only. This lets
the updated daemon-supervised agent serve an installed GUI automatically.
`serve --web` and auto-starting `web` require assets and fail early when absent.
Only point the override at a trusted **built dist**, never a workspace or home
directory: its supported static files are deliberately public. Canonical-path
checks reject symlinks outside the root, dotfiles, traversal, and ambiguous
encoded/platform paths. GUI assets are not embedded in the Rust executable.

## Authentication, management, and skill writes

The GUI is a client of the existing `/v2` API, **not a second engine**. Static
GET/HEAD responses are outside API bearer authentication. All `/v2` requests,
including unknown routes, retain the original API router, authentication and
plugin fallback; no `/v2` route falls back to SPA HTML. Existing API CORS policy
is unchanged; the launcher adds no origins or credentials. Same-origin hosting
needs no additional CORS configuration.

Enter the API credential through the GUI's credential controls. Nothing places
a token in a URL or injects it into HTML/assets. For full Neoism, the daemon's
`/agent` proxy uses its existing paired/local authentication and scoped-token
flow; this direct-origin launcher neither mints nor replaces those credentials.
Use an appropriate credential for the direct agent endpoint.

Management is **not enabled by `web` or `--web`**. To enable GUI skill writes,
explicitly set `NEOISM_AGENT_MANAGEMENT_API=1` in the environment of the process
that actually starts the agent, and configure authenticated local-operator
access (for a standalone server, a nonempty `NEOISM_AGENT_TOKEN` supplied through
a secure environment/secret mechanism). Restart the actual supervisor when
changing its environment. Supply that credential to the GUI, not the launcher
URL. Management handlers continue to require authenticated local-operator
claims; enabling the environment flag alone does not permit anonymous writes.
Hosted tenant credentials remain subject to the existing management restrictions.
The same management skill routes, revision/conflict checks and workspace scopes
apply; no GUI-only write endpoint was added.

For remote exposure use explicit `serve --web` and the existing authentication
and network configuration. `web` never auto-starts a non-loopback listener and
never overrides the server's remote-auth safety checks.

## Verification

`cargo check -p neoism-agent -p neoism-agent-server --tests` checks the server,
CLI and tests without a release build. Execute the targeted tests with:

```sh
cargo test -p neoism-agent-server gui::tests
cargo test -p neoism-agent web_launcher::tests
```

Coverage includes public GUI/API routing separation, missing assets,
traversal/encoded-path rejection, symlink escape protection, and
origin-only/no-secret launcher URLs.
