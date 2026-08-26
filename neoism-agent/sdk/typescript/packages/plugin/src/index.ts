/**
 * Author API + runtime host for Neoism serve plugins (`neoism-plugin/2`).
 *
 * A serve plugin is a long-lived process the agent server spawns per
 * workspace. Declare tools, hooks, and event subscriptions with
 * {@link definePlugin}, then hand the definition to {@link runPlugin}:
 *
 * ```ts
 * import { definePlugin, runPlugin } from "@neoism/plugin";
 *
 * await runPlugin(definePlugin({
 *   tools: [{
 *     id: "todo_count",
 *     description: "Count TODO markers in the workspace",
 *     parameters: { type: "object", properties: {} },
 *     async execute(_input, context) {
 *       return { output: `workspace: ${context.directory}` };
 *     },
 *   }],
 *   hooks: {
 *     "chat.options"(_context, value) {
 *       return { ...value, temperature: 0 };
 *     },
 *   },
 *   events: {
 *     namespaces: ["session."],
 *     handler(event) { console.error("saw", event.type); },
 *   },
 * }));
 * ```
 *
 * Configure it in the workspace `plugins` map with `serve`, `entry`, or
 * `npm` options; the server handles spawning, sandboxing, and lifecycle.
 */
import type { Event } from "@neoism/sdk-core";
import { createHttpClient } from "@neoism/sdk-http";

export interface ToolContext {
  directory: string;
  sessionId?: string;
  /** SDK client bound to the local agent server, when the host provided one. */
  client?: ReturnType<typeof createHttpClient>;
}

export interface ToolResult {
  output: string | unknown;
  title?: string;
  metadata?: unknown;
}

export interface PluginTool {
  id: string;
  description: string;
  /** JSON Schema for the tool input. */
  parameters: unknown;
  execute(input: unknown, context: ToolContext): ToolResult | Promise<ToolResult>;
}

export type HookHandler = (
  context: unknown,
  value: unknown,
) => unknown | Promise<unknown>;

export interface InitializeContext {
  pluginId: string;
  directory: string;
  /** The `config` object from this plugin's workspace configuration entry. */
  config: unknown;
  client?: ReturnType<typeof createHttpClient>;
}

export interface NeoismPlugin {
  name?: string;
  version?: string;
  tools?: PluginTool[];
  /** Keyed by hook name, e.g. "chat.options", "tool.before", "shell.env". */
  hooks?: Record<string, HookHandler>;
  events?: {
    /** Event-type prefixes to receive, e.g. ["session.", "message."]. */
    namespaces?: string[];
    handler?(event: Event): void | Promise<void>;
  };
  initialize?(context: InitializeContext): void | Promise<void>;
}

export function definePlugin(plugin: NeoismPlugin): NeoismPlugin {
  return plugin;
}

interface HostFrame {
  id?: number | null;
  method: string;
  params: Record<string, unknown>;
}

/**
 * Serve the plugin over stdio until the host shuts it down. Never resolves in
 * normal operation.
 */
export async function runPlugin(plugin: NeoismPlugin): Promise<void> {
  const { stdin, stdout } = await import("node:process");
  const readline = await import("node:readline");

  const serverUrl = process.env["NEOISM_AGENT_SERVER_URL"];
  const client = serverUrl
    ? createHttpClient({
        baseUrl: serverUrl,
        ...(process.env["NEOISM_AGENT_TOKEN"]
          ? { token: process.env["NEOISM_AGENT_TOKEN"] }
          : {}),
      })
    : undefined;
  const tools = new Map((plugin.tools ?? []).map((tool) => [tool.id, tool]));
  let directory = process.env["NEOISM_WORKSPACE_DIR"] ?? ".";

  const reply = (id: number, result: unknown) => {
    stdout.write(`${JSON.stringify({ id, result })}\n`);
  };
  const fail = (id: number, error: unknown) => {
    stdout.write(
      `${JSON.stringify({ id, error: error instanceof Error ? error.message : String(error) })}\n`,
    );
  };

  const handle = async (frame: HostFrame) => {
    const { id, method, params } = frame;
    try {
      switch (method) {
        case "initialize": {
          directory = String(params["directory"] ?? directory);
          await plugin.initialize?.({
            pluginId: String(params["pluginId"] ?? ""),
            directory,
            config: params["config"],
            ...(client ? { client } : {}),
          });
          if (typeof id === "number") {
            reply(id, {
              protocol: "neoism-plugin/2",
              name: plugin.name,
              version: plugin.version,
              tools: (plugin.tools ?? []).map((tool) => ({
                id: tool.id,
                description: tool.description,
                parameters: tool.parameters,
              })),
              hooks: Object.keys(plugin.hooks ?? {}),
              eventNamespaces: plugin.events?.namespaces ?? [],
            });
          }
          return;
        }
        case "tool.invoke": {
          const tool = tools.get(String(params["tool"]));
          if (!tool) throw new Error(`unknown tool ${String(params["tool"])}`);
          const result = await tool.execute(params["input"], {
            directory: String(params["directory"] ?? directory),
            ...(typeof params["sessionId"] === "string"
              ? { sessionId: params["sessionId"] }
              : {}),
            ...(client ? { client } : {}),
          });
          if (typeof id === "number") {
            reply(id, {
              output:
                typeof result.output === "string"
                  ? result.output
                  : JSON.stringify(result.output),
              title: result.title,
              metadata: result.metadata,
            });
          }
          return;
        }
        case "hook.invoke": {
          const hook = plugin.hooks?.[String(params["hook"])];
          const value =
            hook === undefined
              ? params["value"]
              : await hook(params["context"], params["value"]);
          if (typeof id === "number") reply(id, value);
          return;
        }
        case "event": {
          await plugin.events?.handler?.(params as unknown as Event);
          return;
        }
        case "shutdown": {
          if (typeof id === "number") reply(id, {});
          process.exit(0);
        }
      }
    } catch (error) {
      if (typeof id === "number") fail(id, error);
      else console.error(error);
    }
  };

  const lines = readline.createInterface({ input: stdin });
  for await (const line of lines) {
    if (!line.trim()) continue;
    let frame: HostFrame;
    try {
      frame = JSON.parse(line) as HostFrame;
    } catch {
      console.error("neoism-plugin: skipping malformed frame");
      continue;
    }
    void handle(frame);
  }
}
