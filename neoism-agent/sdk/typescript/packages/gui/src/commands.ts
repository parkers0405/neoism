import type { SlashCommand } from "./types";
export function filterCommands(
    commands: readonly SlashCommand[],
    text: string,
): SlashCommand[] {
    const q = text.replace(/^\//, "").toLowerCase();
    return commands.filter(
        (c) =>
            [c.name, ...c.aliases].some((n) =>
                n.replace(/^\//, "").toLowerCase().includes(q),
            ) || c.description.toLowerCase().includes(q),
    );
}
export function moveSelection(index: number, delta: number, length: number) {
    return length ? (index + delta + length) % length : 0;
}
export function parseCommand(text: string, catalog: readonly SlashCommand[]) {
    const input = text.trim().replace(/^\//, "");
    const raw = input.match(/^\S*/)?.[0] || "";
    const args = input.slice(raw.length).trimStart();
    const found = catalog.find((c) =>
        [c.name, ...c.aliases].some((n) => n.replace(/^\//, "") === raw),
    );
    return {
        name: (found?.name || raw).replace(/^\//, ""),
        args,
        original: text.trim(),
    };
}
export interface CommandHost {
    local(name: string, args: string): Promise<boolean>;
    forward(command: string): Promise<void>;
    confirm(message: string): boolean;
}
export async function executeCommand(
    text: string,
    catalog: readonly SlashCommand[],
    host: CommandHost,
) {
    const command = parseCommand(text, catalog);
    if (
        ["yolo", "dangerously-skip-permissions", "skip-permissions"].includes(
            command.name,
        ) &&
        !host.confirm(
            "Change permission bypass mode? Enabling bypass lets the agent execute destructive commands without asking. Only enable it in a trusted, isolated workspace. You can re-enable checks from the warning banner.",
        )
    )
        return;
    if (!(await host.local(command.name, command.args)))
        await host.forward(command.original);
}
