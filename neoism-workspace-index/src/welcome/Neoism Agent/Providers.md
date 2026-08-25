# Providers

Neoism builds its provider and model catalog from [Models.dev](https://models.dev), caches that catalog locally, and applies Neoism's runtime support and your connection state before presenting usable models.

A provider needs both a runtime adapter Neoism can execute and, when required, a usable connection.

## Connect a provider

Run `/connect` in an agent pane. Neoism opens a staged provider flow:

1. Choose a provider from **Popular** or the full provider catalog. Connected providers show a checkmark.
2. Choose one of the provider's authentication methods.
3. Enter an API key/token or complete the OAuth browser flow.

The same flow can disconnect a provider or replace its stored credential. Depending on the provider, Neoism may offer an API key, OAuth browser flow, pasted OAuth token, subscription flow, or environment connection.

Stored credentials live in Neoism Agent's local `auth.json`. On Unix it is written with mode `0600`; on Windows Neoism attempts an owner-and-SYSTEM-only ACL. Connecting again replaces the provider's stored credential, and removing the connection deletes it.

## Environment connections

Models.dev supplies standard environment variable names. A non-empty declared variable can make a provider available without storing another credential:

```sh
export ANTHROPIC_API_KEY="your-key"
export OPENAI_API_KEY="your-key"
```

Environment variables are read by the Neoism Agent process. Restart the agent server if its process started before the variable was exported. Do not commit keys to tracked shell files.

## Authentication methods

Neoism includes explicit handling for several provider families:

| Provider family | Connection forms |
|---|---|
| Catalog API providers | API key and OAuth token when advertised. |
| OpenAI | API key and OpenAI OAuth/Codex subscription flow. |
| GitHub Copilot | Device-code authorization and provider-specific variants. |
| Claude Code | Subscription bridge credentials. |
| xAI | API key plus the xAI OAuth flows returned by the server. |

The exact methods returned in the picker are authoritative because provider support can change independently of a Neoism release.

## Catalog and cache

Neoism fetches `https://models.dev/api.json` and keeps a local cache. The in-process catalog is refreshed on a five-minute window. If refresh fails and a cache exists, Neoism uses that cache.

Advanced overrides:

| Variable | Purpose |
|---|---|
| `NEOISM_AGENT_MODELS_URL` | Replace the Models.dev source base URL. |
| `NEOISM_AGENT_MODELS_PATH` | Load a catalog from a local JSON file. |
| `NEOISM_AGENT_MODELS_FETCH` | Control remote catalog fetching. |
| `NEOISM_AGENT_AUTH_PATH` | Override the stored credential file. |
| `NEOISM_AGENT_AUTH_CONTENT` | Supply an in-memory credential document. |

## Enable or disable providers

```jsonc
{
  "agent": {
    "enabledProviders": ["anthropic", "openai"],
    "disabledProviders": ["openrouter"]
  }
}
```

`enabledProviders` is an allowlist. `disabledProviders` removes matching IDs afterward. IDs come from the catalog and model picker.

## Compatible endpoints

Neoism derives provider API endpoints and adapter kinds from Models.dev. It also supports deployment-level base URL overrides; OpenAI supports `NEOISM_AGENT_OPENAI_BASE_URL`, and catalog adapters expose normalized provider-specific override variables.

Neoism's public config does **not** define an arbitrary `providers` package-overlay object. Do not use examples with `package`, `headers`, `body`, or nested custom model maps; those are not Neoism's configuration contract.

## Security

- Prefer the connection UI or environment variables over literal JSONC secrets.
- Do not place provider keys in project `neoism.json`.
- A compatible/custom endpoint can receive prompts, attachments, instructions, and tool results.
- Removing a credential does not delete past session content.

See [[Models]] and [[Troubleshooting]].