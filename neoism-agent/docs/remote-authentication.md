# Remote authentication

The browser and Agent do not have to run on the same computer. Start authentication against the Agent that will run your conversations; open the authorization link in your own browser. Credentials remain in that Agent's credential store. Persist its state directory across container replacements.

## Model providers

Use `/connect` in the Agent UI, or use the same authentication implementation from a terminal:

```sh
neoism-agent auth login codex --no-open
neoism-agent auth login xai --method headless --no-open
```

`--no-open` prints the URL without trying to launch a browser and requests device authorization for OpenAI and xAI. Enter the displayed code in your browser. These device flows do not need an incoming callback, a published OAuth port, or a browser in Docker. Providers can require account-side permission to use device authorization.

Hosted provider requests automatically use device authorization for OpenAI and xAI browser methods. Self-hosted Docker images set `NEOISM_AGENT_HEADLESS=true` for the same behavior, including UI requests through a workspace daemon. Set this variable on other headless deployments too. GitHub Copilot already uses device authorization. Other providers retain their advertised API-key, token, or provider-specific flow; this does not turn an arbitrary provider into a device-code provider.

For a CLI on your own computer, select a remote Agent with `--server https://your-agent.example`. Supply its explicitly scoped access token using `NEOISM_SERVER_TOKEN`. Remote authenticated CLI requests require HTTPS and do not follow redirects. The local `NEOISM_AGENT_TOKEN` is only used for loopback destinations, never automatically sent to a remote server.

## MCP servers

Use `/mcp` in the UI or:

```sh
neoism-agent mcp auth SERVER --no-open
```

MCP OAuth generally uses an authorization-code callback rather than device authorization. The authorization link can be opened on another computer, but the callback must reach the Agent that started the flow. `--no-open` alone does not provide callback routing.

For a joined workspace, use its daemon-proxied callback. For a custom deployment, configure the MCP entry's `oauth.redirectUri` to the HTTPS callback routed to the correct Agent. The provider must accept that redirect URI; dynamic registration support varies. Do not derive public callback URLs from untrusted forwarded headers or expose an unrestricted unauthenticated Agent proxy.

Alternatively, set `NEOISM_AGENT_MCP_OAUTH_PUBLIC_BASE_URL` on the Agent to its externally routed HTTPS base URL, including any workspace/generation prefix. Neoism appends `/v2/plugins/dev.neoism.mcp/<encoded-server-name>/auth/callback`. This also covers authentication started by a CLI inside the container. An explicit MCP `oauth.redirectUri` takes precedence. The deployment gateway must forward that exact browser GET callback without requiring a bearer token; the Agent admits it only through a matching, one-time OAuth state. All other Agent routes must remain protected. This setting supplies a callback address, not a gateway or TLS certificate.

If a provider requires a fixed localhost callback, a local port forward can instead connect the browser computer's callback port to the remote listener. Publishing a Docker port on Azure is not equivalent to forwarding localhost on your computer. Prefer device authorization when the model provider supports it.