import { mcp, type NeoismClient } from "@neoism/sdk";
export type McpPlugin = Awaited<ReturnType<typeof getMcp>>;
export type McpCatalog = Awaited<ReturnType<McpPlugin["catalog"]>>;
export const getMcp = (client: NeoismClient, directory: string) => client.plugins.use(mcp, { directory });
export type McpAction = "enable" | "disable" | "connect" | "disconnect" | "auth" | "logout";
/** Revalidate before mutation; a read-only config still allows runtime/auth actions. */
export async function runMcpAction(plugin: McpPlugin, directory: string, name: string, action: McpAction) {
    const entry = (await plugin.catalog(directory))[name];
    if (!entry) throw new Error(`MCP server ${name} is no longer available.`);
    if (action === "enable" || action === "disable") {
        if (!entry.configWritable) throw new Error("This MCP configuration is read-only.");
        await plugin.configure(name, { enabled: action === "enable" }, directory);
    } else if (action === "connect") {
        if (!(await plugin.connect(name, directory))) return plugin.startAuth(name, directory);
    } else if (action === "disconnect") await plugin.disconnect(name, directory);
    else if (action === "auth") return plugin.startAuth(name, directory);
    else await plugin.removeAuth(name, directory);
}
