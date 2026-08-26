# Models

A Neoism model is identified as `provider/model`, for example:

```text
anthropic/claude-sonnet-4-5
openai/gpt-5.2-codex
```

The model picker combines Neoism's current Models.dev catalog, runtime adapter support, and active provider connections. A catalog entry alone does not guarantee that Neoism can execute it.

## Select a model

Use `/model` in an agent pane or open the model picker from the agent header. The selection is stored on the session.

```jsonc
{
  "agent": {
    "model": "anthropic/claude-sonnet-4-5",
    "smallModel": "anthropic/claude-haiku-4-5",
    "variant": "high",
    "textVerbosity": "low"
  }
}
```

`model` is the normal default. `smallModel` is available for lightweight internal work. `variant` selects a provider/model variant when one exists.

`textVerbosity` controls response length for supported models. Valid values are `low`, `medium`, and `high`. Neoism defaults compatible non-chat GPT-5.x models to `low`.

## Resolution order

Neoism considers a session-selected model, the selected agent's model, the configured default model, and finally a usable connected catalog model. If none is usable, it opens the picker instead of silently calling an unrelated provider.

## Catalog metadata

Neoism consumes model metadata including display name, tool support, input/output modalities, context and output limits, cost/cache pricing, structured output, attachments, reasoning, temperature, family, release date, status, and variants.

## Reasoning variants

Use `/variant` for the current session:

```jsonc
{
  "agent": {
    "model": "openai/gpt-5.2-codex",
    "variant": "high"
  }
}
```

Values are model-specific. Use the canonical `variant` field.

## Per-agent models

```jsonc
{
  "agent": {
    "model": "anthropic/claude-sonnet-4-5",
    "agent": {
      "review": {
        "description": "Deep code reviewer",
        "mode": "subagent",
        "model": "openai/gpt-5.2-codex",
        "variant": "high"
      }
    }
  }
}
```

An agent can also set `temperature`, `topP`, and `maxSteps` where supported.

## Missing models

A model may be hidden because its provider is disconnected, Neoism lacks a runtime adapter, configuration disabled the provider, the catalog is unavailable, a subscription permits only a subset, or the configured ID no longer exists.

See [[Providers]] and [[Troubleshooting]].
