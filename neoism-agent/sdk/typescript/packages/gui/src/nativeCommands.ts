import { goals, mcp, type NeoismClient } from "@neoism/sdk";
export interface NativeContext {
    client: NeoismClient;
    id?: string;
    directory: string;
    show(title: string, value: unknown): void;
    /** Controller supplies these to preserve selection/model settings and guard stale sessions. */
    ensureSession?(): Promise<string>;
    sendPrompt?(id: string, text: string): Promise<unknown>;
}
/** Native operations which don't depend on GUI state. Unknown commands are deliberately not swallowed. */
export async function nativeCommand(
    name: string,
    args: string,
    { client, id, directory, show, ensureSession, sendPrompt }: NativeContext,
): Promise<boolean> {
    if (name === "mcp" || name === "mcps") {
        const plugin = await client.plugins.use(mcp, { directory });
        show("MCP servers", await plugin.catalog(directory));
        return true;
    }
    if (
        ![
            "goal",
            "permit",
            "answer",
            "reject",
            "deny",
            "skill",
            "skills",
        ].includes(name)
    )
        return false;
    if (name === "skill" || name === "skills") {
        const skills = await client.catalog.skills.list(directory);
        const [action, skillName] = args.split(/\s+/);
        if (action === "list") {
            show("Skills", skills);
            return true;
        }
        if (action === "info") {
            const skill = skills.find((s) => s.name === skillName);
            if (!skill)
                throw new Error(
                    "Usage: /skill info <name>. The skill must be available on the server.",
                );
            show("Skill", skill);
            return true;
        }
        if (!skills.some((s) => s.name === action))
            throw new Error(
                `Skill “${action}” is not available on this server.`,
            );
        if (!id) throw new Error("Open a chat before invoking a skill.");
        await client.sessions.prompt(id, {
            prompt: `Use the ${action} skill. ${args.slice(action.length).trim()}`,
        });
        return true;
    }
    args = args.trim();
    const startsGoal = name === "goal" && !!args && !["clear", "pause", "resume"].includes(args);
    if (startsGoal && !id && ensureSession) id = await ensureSession();
    if (!id) throw new Error("Open a chat first.");
    if (name === "goal") {
        const plugin = await client.plugins.use(goals, { directory });
        const result = !args
            ? await plugin.get(id)
            : args === "clear"
              ? await plugin.clear(id)
              : args === "pause" || args === "resume"
                ? await plugin.set(id, { paused: args === "pause" })
                : await plugin.set(id, { text: args });
        if (startsGoal) {
            if (sendPrompt) await sendPrompt(id, args);
            else await client.sessions.prompt(id, { prompt: args });
        }
        show("Goal", result);
        return true;
    }
    if (name === "permit") {
        const [first = "once", second] = args.split(/\s+/).filter(Boolean);
        const aliases: Record<string, "once" | "always" | "reject"> = {
            once: "once",
            allow: "once",
            yes: "once",
            always: "always",
            all: "always",
            reject: "reject",
            deny: "reject",
            no: "reject",
        };
        const reply = aliases[first] || "once";
        const requested = second || (!aliases[first] ? first : undefined);
        const pending = await client.interactions.permissions.list(id);
        const permission = requested
            ? pending.find((p) => p.id === requested)
            : pending[0];
        if (!permission) throw new Error("No matching pending permission.");
        if (
            reply === "always" &&
            !confirm("Always allow this permission pattern?")
        )
            return true;
        await client.interactions.permissions.reply(permission.id, reply);
        show("Permission", `${permission.id}: ${reply}`);
        return true;
    }
    const questions = await client.interactions.questions.list(id);
    if (name === "answer") {
        if (!args) throw new Error("Usage: /answer <text>");
        const question = questions[0];
        if (!question) throw new Error("No pending questions.");
        await client.interactions.questions.reply(
            question.id,
            question.questions.length <= 1
                ? [[args.trim()]]
                : args.split(";").map((answer) => answer.trim()).filter(Boolean).map((answer) => [answer]),
        );
        show("Question", "Answer submitted.");
        return true;
    }
    const question = args ? questions.find((q) => q.id === args) : questions[0];
    if (question) {
        await client.interactions.questions.reject(question.id);
        show("Question", "Rejected.");
        return true;
    }
    const permissions = await client.interactions.permissions.list(id);
    const permission = args
        ? permissions.find((p) => p.id === args)
        : permissions[0];
    if (!permission) throw new Error("No matching pending interaction.");
    await client.interactions.permissions.reply(permission.id, "reject");
    show("Permission", "Rejected.");
    return true;
}
