# Compaction

Compaction keeps a long session usable when its accumulated messages approach the selected model's context limit. Neoism summarizes older context into a durable compaction part and continues from that summary plus newer messages.

## Manual compaction

Run `/compact` or `/summarize`. The Agent server generates a structured handoff summary containing the active goal, constraints, progress, decisions, next steps, and critical context.

The summary is stored in the session. It is not only transient text shown in the UI.

## Automatic compaction

Automatic compaction is enabled by default at **65% of the effective model context window**. Before every provider step (including steps inside tool loops), Neoism estimates the complete upcoming request: system instructions, conversation, attachments, tool calls/results, and tool definitions. This follows OpenCode v2's proactive request check rather than waiting for usage from the previous response.

Images are budgeted as image input, not as the text length of their base64 transport encoding. The current conservative estimate reserves 4,096 tokens per image; actual provider image accounting varies. Trigger checks and retained-history budgeting use the same estimator, so larger PNG/JPEG encodings do not by themselves force repeated compaction.

Configure it in the `agent` block of your global `config.json` or workspace `.neoism/config.json`:

```json
{
  "agent": {
    "compaction": {
      "auto": true,
      "threshold-percent": 65,
      "buffer": 20000,
      "keep": { "tokens": 8000 }
    }
  }
}
```

- `auto`: defaults to `true`. Set `false` to bypass both proactive compaction and automatic context-overflow recovery. Manual `/compact` still works. Long requests may then exceed the provider's context limit.
- `prune`: defaults to `true`. Independently removes older tool output from model context. To bypass both automatic summarization and tool-output pruning, set `auto: false` and `prune: false`.
- `threshold-percent`: a number from 1 through 100, including fractional percentages. Defaults to 65. This is a percentage of the effective context window, not of an already-reduced input budget.
- `buffer`: minimum token headroom, default 20,000. The trigger is capped by this headroom and the model's safe input/output limits. A 100% setting does not remove output headroom; use `auto: false` to disable automatic compaction.
- `keep.tokens`: recent context to retain alongside the summary, default 8,000. Set 0 to disable tail preservation. It is bounded by the context/trigger budget, and complete tool exchanges are preserved where possible.

These controls are also exposed in Settings under Agent. A 200,000-token context normally compacts at approximately 130,000 tokens with the default 65%. Estimates are approximate, not exact tokenizer counts. When model limits are unavailable, Neoism uses a 120,000-token fallback context budget.

### Model and agent overrides

The same `compaction` object can be placed on a provider model or an agent profile:

```json
{
  "agent": {
    "compaction": { "threshold-percent": 65 },
    "provider": {
      "openai": {
        "models": {
          "gpt-5": {
            "compaction": { "threshold-percent": 75 }
          }
        }
      }
    },
    "agent": {
      "explore": {
        "compaction": { "threshold-percent": 50 }
      },
      "reviewer": {
        "compaction": { "auto": false }
      }
    }
  }
}
```

Overrides merge field by field: agent profile overrides model, which overrides the global/workspace policy. Unspecified fields inherit. Workspace configuration overrides global configuration. A profile can explicitly re-enable automatic compaction with `auto: true`.

### OpenAI API versus Codex subscriptions

The context budget follows the selected OpenAI connection's authentication path. OpenAI API-key connections retain the API catalog limits; Codex/ChatGPT OAuth subscription connections use Neoism's subscription-specific limits. This covers **Sol (`gpt-5.6-sol`) and Astra (`gpt-6-astra`)**, as well as the GPT-5.4/5.5/5.6 and GPT-6 families. A large API context window must not delay compaction on a subscription connection.

Neoism currently applies conservative OAuth ceilings of 400,000 context tokens, 272,000 input tokens, and 128,000 output tokens for these families. A lower subscription catalog limit can reduce these further. These are application safety limits, not a guarantee of a plan's live allowance. At the default 65%, the 400k context gives a 260k target, but the 272k input limit minus the default 20k reserve lowers the effective trigger to **252k**. By comparison, an API catalog entry with a 1.05M context and 922k input gives a **682.5k** trigger. Actual thresholds follow the selected model's effective metadata and configured reserves.

Subscription request/token usage still counts toward context even though subscription model costs are not represented as API per-token charges. Compaction does not reset or bypass account usage quotas, rate limits, or subscription allowance windows.

### Continuation and recovery

After compaction, Neoism reloads durable history and rebuilds the request from the summary and retained recent context. It avoids compacting again when the summary already covers all messages. If the provider still reports a context overflow on an uncompacted attempt, Neoism can compact and retry once. It does not repeatedly compact/retry the same provider step, and cancellation stops continuation.

Legacy process-wide controls remain supported: `NEOISM_AGENT_AUTO_COMPACT=false` forces automatic compaction off; `NEOISM_AGENT_AUTO_COMPACT_TOKENS` overrides the token trigger (0 disables automatic compaction). `NEOISM_AGENT_COMPACTION_RESERVED_TOKENS` changes the safety reserve for models with a separate input limit. Prefer the configuration settings for normal use.

## What remains

After compaction, Neoism keeps:

- The structured summary of prior work.
- Messages newer than the compaction boundary.
- The session's agent/model metadata.
- Durable child-session and task references that remain in storage.

Original messages remain part of stored history, but the full old transcript is no longer necessarily included in each model request.

## What can be lost from model context

A summary cannot preserve every exact sentence or tool byte. Details are most at risk when they were never written to a file or captured as a durable decision.

For long work:

- Keep source-of-truth decisions in project notes or memory.
- Save exact commands, IDs, and paths when they matter.
- Ask the agent to produce a handoff summary before switching providers.
- Reattach a file or quote exact text when later work depends on it.

## Hidden compaction agent

Neoism uses an internal compaction agent with a low temperature. It is not a normal selectable implementation agent. The selected provider/model still determines whether compaction can run successfully.

## Failure behavior

Compaction can fail if no model is available, provider authentication expires, the provider rejects the context, or the session is interrupted. The existing session history is not deleted by a failed compaction.
See [[Sessions and Sharing]] and [[Memory]].
