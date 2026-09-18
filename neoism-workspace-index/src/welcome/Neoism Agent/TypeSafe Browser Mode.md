# Experimental TypeSafe Browser Mode

TypeSafe is an **optional, off-by-default mode inside the built-in `computer` MCP**. It does not replace the main agent or normal desktop tools. The main agent supplies a narrow goal, explicit limits, and every exact text/select/URL value that may be used. TypeSafe's Jev model chooses among server-generated typed operations while Neoism executes a bounded DOM-only goal with a fresh observation after each action.

This follows the System One pattern: typed choices and probabilities, not generated scripts or a second full reasoning agent. It uses our existing attached Chromium/CDP or Firefox/BiDi browser, not a separate Playwright-managed browser. See [[Computer Use]] for browser attachment and the required computer-use permission.

## Enable and enter a key

In the agent GUI's **MCP catalog**, expand the **computer** entry and enable **Experimental: TypeSafe/Jev browser goals**. The API-key field appears when the mode is enabled. Save your TypeSafe API key there. It uses the existing server credential store; the key is never loaded back into the form or written into workspace configuration. Saving does not make a paid API request or validate the key. The first browser goal checks it.

The desktop Settings panel also exposes the enable toggle. For a headless/server setup, supply `TYPESAFE_API_KEY` in the agent server's environment. The environment variable takes precedence over a saved key. Do not send keys in chat, tool arguments, a committed `.env`, or workspace `config.json`.

Enablement is separate from the computer MCP itself and from session permissions. All three must allow the operation. Example global or workspace `config.json`:

```json
{
  "agent": {
    "mcp": {
      "computer": {
        "type": "local",
        "command": ["builtin", "computer"],
        "enabled": true
      }
    },
    "experimental": {
      "options": {
        "computer-typesafe": { "enabled": true }
      }
    }
  }
}
```

Set the experimental `enabled` field to `false` to return to ordinary computer tools. Disabling it does not delete the saved credential. Use **Remove saved TypeSafe key** to remove that credential; unset the server environment variable separately if present.

## Prefer a bounded goal

Choose the correct native window and tab explicitly using the normal computer tools. Then call:

```json
{
  "target": "<window token>",
  "tab": "<attached tab ID>",
  "goal": "Search for Rust async examples and open the most relevant result",
  "text_values": ["Rust async examples"],
  "max_steps": 4,
  "timeout_ms": 15000
}
```

The preferred tool is `computer.browser_goal`. Jev may choose compatible `click`, `fill`, `select`, guarded page `scroll`, optionally enabled `back`, and exact caller-supplied HTTP(S) navigation candidates. Fill values go in `text_values`, option values in `select_values`, and URLs in `navigate_urls`; all are used verbatim and Jev cannot invent text, URLs, scripts, passwords, or file inputs. `allow_back` defaults to false. `max_steps` is 1–8 (default 4), and `timeout_ms` is 1,000–30,000 (default 15,000). Use `"preview": true` to inspect only the first proposal; preview still sends the observation to TypeSafe and incurs API usage.

`computer.browser_step` remains available for compatibility and deliberate one-action `click`, `fill`, or `select` calls. When the mode is active, prefer goal delegation instead of repeatedly calling that single-step tool.

Each call:

1. Obtains a fresh, focused-page observation using `browser_observe`.
2. Generates only operations compatible with observed elements and caller-supplied values, then sends one bounded `jev-latest` request that jointly asks for the operation, completion evidence, and consequential-effect risk.
3. Validates the answer against the exact candidate distribution and existing confidence/probability thresholds. A no-match option is always available.
4. Rechecks cancellation, revocation, current computer/TypeSafe enablement, window focus, and observation freshness before every dispatch through `browser_act`.
5. Uses the post-action observation as the next fresh state and sends only a compact executed trace, never conversation history. It stops at the step/time budget or any safety/uncertainty terminal state.

## Interpret the result

- `preview`: no action performed. Inspect the nested decision status.
- `no_match`: no suitable offered element; observe or use normal tools.
- `ambiguous`: insufficient model certainty; inspect the page rather than retrying blindly.
- `needs_confirmation`: the model flagged potentially consequential effects. Confirm the actual action with the user before any manual action; do not use another tool to evade a refusal.
- `possibly_done`: model judgment only, not proof of task completion. Verify independently.
- `step_budget_exhausted` or a time-budget error: bounded execution stopped; completion is not implied.
- `partial_unknown`: an action may have executed, so the loop stops immediately and never replays it. Observe before any deliberate retry.
- A normal goal trace records attempted typed operations compactly. DOM dispatch is not verified task completion.

The experimental gate requires choice confidence at least 0.85 and selected probability at least 0.75, and declines when the risk probability is at least 0.1. These are initial engineering thresholds, not validated guarantees of safety. Model confidence never supplies authorization. Existing computer-use permissions and browser restrictions remain the enforcement boundary.

## Privacy, limits, and failures

**Using this mode transmits page content to an external provider.** Visible text, URL (including query parameters), title, element labels, ordinary field values, options, goal, supplied values/URLs, and a compact executed-action trace may contain private information. Do not use it on sensitive pages unless that disclosure is explicitly authorized. Password inputs are excluded by our existing browser observation, but this does not scrub secrets from all ordinary text fields, URLs, or page text.

No screenshots are sent to Jev. This is not a vision model and does not promise native desktop semantics: it controls only the attached page's bounded main-frame DOM, not desktop pixels, native dialogs, browser chrome, canvas, arbitrary applications, iframes, or shadow roots. The existing visible-element, password/upload, foreground, HTTP(S), and stale-reference restrictions still apply. Bounded observations can omit relevant candidates; `no_match` is a normal outcome.

Requests and responses are bounded to 128 KiB, use a fixed HTTPS endpoint with redirects disabled, and have an eight-second HTTP timeout. The client reuses connections. Authentication failures, rate limits, malformed responses, timeouts and cancellations do not trigger action retries. Another observation or user change can invalidate a decision before it is used.

No live latency or accuracy guarantee is claimed. `decisionMs` measures the external decision request, not total task duration or permission wait. Test representative tasks with your account before depending on the mode.

## Skill and references

The installed **TypeSafe skill** teaches the development agent how to build integrations; it is not the runtime itself. This mode implements the actual API calls in Neoism.

- [TypeSafe HTTP API](https://docs.typesafe.ai/api)
- [Choice primitive and no-match outcomes](https://docs.typesafe.ai/primitives/choice)
- [Function-calling cookbook](https://docs.typesafe.ai/cookbooks/function_calling)
- [Community jev-browser implementation](https://github.com/Ying-Kai-Liao/jev-browser), an architectural reference with self-reported benchmarks, not a dependency or an independently verified performance claim.
