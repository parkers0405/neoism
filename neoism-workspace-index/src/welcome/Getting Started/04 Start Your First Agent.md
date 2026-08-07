# Start Your First Agent

Neoism agents are workspace participants with persistent conversations and tool access, not a detached chat box. An agent session is associated with the active workspace directory, so begin after opening the project you want it to inspect.

## Open an agent pane

Press `Alt + A`, or open the command palette and choose **New Agent Pane**. The pane contains a conversation timeline and a composer for your request.

Start with a bounded, verifiable task. For example:

> Explain how configuration is loaded in this workspace. Cite the relevant files and do not edit anything.

Then progress to an implementation request that states the desired outcome, constraints, and how you want it verified. Mention files that must not change when that matters.

## Review tool permissions

Agents can use tools such as project search, file reads and edits, language-server queries, and shell commands. When a tool requires approval, Neoism displays a permission request in the conversation. Read the proposed operation and approve or deny it there.

Global permission defaults live in the `agent.permission` block of `config.json`. Keep destructive or broad operations on `"ask"` until you are comfortable with the workflow:

```jsonc
{
  "agent": {
    "permission": {
      "edit": "ask",
      "bash": "ask",
    },
    "dangerously-skip-permissions": false,
  },
}
```

`/yolo` auto-approves prompts for the current session until toggled again. The persistent `dangerously-skip-permissions` setting is broader. Both are intentionally explicit; neither is needed for normal onboarding.

## Choose a model and provider

Run `/connect` to open Neoism's provider flow. Choose a provider, select one of the authentication methods it offers, then enter an API key or finish the OAuth/subscription flow. Run `/connect` again to replace a credential or disconnect that provider.

The active model is displayed in the agent UI and can be changed with the model picker or `/model`. Neoism builds the picker from its provider catalog, supported runtime adapters, and your active connections. Authentication options depend on the selected provider.

Do not copy a model identifier from an example unless it is available in your model picker. Selecting from the picker keeps the configured provider and model capabilities aligned.

## Useful conversation controls

- `/model` opens model selection.
- `/hints` toggles the helper row below the composer and saves that preference.
- `/yolo` toggles session-only automatic approval.
- **Rename Agent** and **Delete Agent** are available from the agent controls.
- **Export Agent** writes a conversation export; **Import Agent** restores a supported export.

The agent can coordinate sub-agents, use project language-server information, and retain memory, but it should still report what changed and what it verified. Review edits in the editor or Git diff panel (`Alt + G`) before committing them.

Next: [[05 Connect Another Device|Connect Another Device]].