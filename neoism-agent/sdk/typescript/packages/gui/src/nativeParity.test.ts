import { describe, it, expect, vi, afterEach } from "vitest";
import { goals, mcp, type NeoismClient } from "@neoism/sdk";
import { nativeCommand } from "./nativeCommands";
import { runMcpAction, type McpPlugin } from "./mcpActions";
import { FX, scheduleFx } from "./fx";

function mockClient(plugin: object = {}) {
    const client = {
        plugins: { use: vi.fn(async () => plugin) },
        sessions: { prompt: vi.fn(async () => ({})) },
        interactions: { questions: { list: vi.fn(async () => [{ id: "q", questions: [{}, {}] }]), reply: vi.fn(async () => true) } },
    };
    return { raw: client, client: client as unknown as NeoismClient };
}
describe("native command SDK operations", () => {
    it("ensures a session, persists the goal, then starts exactly its initial prompt", async () => {
        const order: string[] = [];
        const set = vi.fn(async () => { order.push("set"); return {}; });
        const {client, raw} = mockClient({set});
        const ensureSession = vi.fn(async () => { order.push("ensure"); return "new-session"; });
        const sendPrompt = vi.fn(async () => { order.push("prompt"); });
        await nativeCommand("goal", "  ship it  ", {client, directory: "/work", show: vi.fn(), ensureSession, sendPrompt});
        expect(order).toEqual(["ensure", "set", "prompt"]);
        expect(raw.plugins.use).toHaveBeenCalledWith(goals, {directory: "/work"});
        expect(set).toHaveBeenCalledWith("new-session", {text: "ship it"});
        expect(sendPrompt).toHaveBeenCalledWith("new-session", "ship it");
        expect(raw.sessions.prompt).not.toHaveBeenCalled();
    });
    it("does not prompt when persistence fails", async () => {
        const {client, raw} = mockClient({set: vi.fn(async () => { throw Error("offline"); })});
        await expect(nativeCommand("goal", "ship", {client, id: "s", directory: "/w", show: vi.fn()})).rejects.toThrow("offline");
        expect(raw.sessions.prompt).not.toHaveBeenCalled();
    });
    it.each(["", "clear", "pause", "resume"])("goal %s never starts a turn", async (args) => {
        const {client, raw} = mockClient({get: vi.fn(), clear: vi.fn(), set: vi.fn()});
        await nativeCommand("goal", args, {client, id: "s", directory: "/w", show: vi.fn()});
        expect(raw.sessions.prompt).not.toHaveBeenCalled();
    });
    it("splits multi-question answers without repeating text or retaining empty segments", async () => {
        const {client, raw} = mockClient();
        await nativeCommand("answer", " First ; ; Second ", {client, id: "s", directory: "/w", show: vi.fn()});
        expect(raw.interactions.questions.list).toHaveBeenCalledWith("s");
        expect(raw.interactions.questions.reply).toHaveBeenCalledWith("q", [["First"], ["Second"]]);
    });
    it("keeps semicolons for a single question", async () => {
        const {client, raw} = mockClient();
        raw.interactions.questions.list.mockResolvedValue([{id: "q", questions: [{}]}]);
        await nativeCommand("answer", "a;b", {client, id: "s", directory: "", show: vi.fn()});
        expect(raw.interactions.questions.reply).toHaveBeenCalledWith("q", [["a;b"]]);
    });
    it("opens the MCP GUI through the existing result title", async () => {
        const catalog = vi.fn(async () => ({})); const show = vi.fn();
        const {client, raw} = mockClient({catalog});
        await nativeCommand("mcp", "", {client, directory: "/work", show});
        expect(raw.plugins.use).toHaveBeenCalledWith(mcp, {directory: "/work"});
        expect(catalog).toHaveBeenCalledWith("/work");
        expect(show).toHaveBeenCalledWith("MCP servers", {});
    });
});
function mockMcp(writable = true, connects = true) {
    const raw = {
        catalog: vi.fn(async () => ({server: {configWritable: writable}})),
        configure: vi.fn(), connect: vi.fn(async () => connects), disconnect: vi.fn(),
        startAuth: vi.fn(async () => ({authorizationUrl: "https://auth.example", oauthState: "state"})), removeAuth: vi.fn(),
    };
    return {raw, plugin: raw as unknown as McpPlugin};
}
describe("MCP action SDK calls", () => {
    it.each(["enable", "disable"] as const)("%s persists enabled in the selected directory", async action => {
        const {raw, plugin} = mockMcp();
        await runMcpAction(plugin, "/work", "server", action);
        expect(raw.configure).toHaveBeenCalledWith("server", {enabled: action === "enable"}, "/work");
    });
    it("false connect initiates authentication, not silent success", async () => {
        const {raw, plugin} = mockMcp(true, false);
        expect(await runMcpAction(plugin, "/work", "server", "connect")).toEqual({authorizationUrl: "https://auth.example", oauthState: "state"});
        expect(raw.connect).toHaveBeenCalledWith("server", "/work");
        expect(raw.startAuth).toHaveBeenCalledWith("server", "/work");
    });
    it("connected runtime does not start OAuth", async () => {
        const {raw, plugin} = mockMcp(); await runMcpAction(plugin, "/work", "server", "connect");
        expect(raw.startAuth).not.toHaveBeenCalled();
    });
    it.each(["disconnect", "auth", "logout"] as const)("readonly config permits runtime action %s", async action => {
        const {raw, plugin} = mockMcp(false);
        await runMcpAction(plugin, "/work", "server", action);
        expect(raw[action === "auth" ? "startAuth" : action === "logout" ? "removeAuth" : "disconnect"]).toHaveBeenCalledWith("server", "/work");
    });
    it("readonly config rejects enabled mutation", async () => {
        const {raw, plugin} = mockMcp(false);
        await expect(runMcpAction(plugin, "/work", "server", "enable")).rejects.toThrow("read-only");
        expect(raw.configure).not.toHaveBeenCalled();
    });
});
describe("native FX scheduling", () => {
    afterEach(() => vi.useRealTimers());
    it.each(Object.keys(FX) as (keyof typeof FX)[])("%s dispatches only at the native key moment", kind => {
        vi.useFakeTimers(); const send = vi.fn(), done = vi.fn();
        const cancel = scheduleFx(kind, send, done);
        vi.advanceTimersByTime(FX[kind].promptAt*1000-1); expect(send).not.toHaveBeenCalled();
        vi.advanceTimersByTime(1); expect(send).toHaveBeenCalledExactlyOnceWith(FX[kind].prompt);
        vi.advanceTimersByTime(FX[kind].seconds*1000); expect(done).toHaveBeenCalledOnce(); cancel();
    });
    it("cleanup or a changed selection prevents delayed dispatch", () => {
        vi.useFakeTimers(); const send = vi.fn(), done = vi.fn();
        scheduleFx("piss", send, done)();
        scheduleFx("disco", send, done, () => false);
        vi.runAllTimers(); expect(send).not.toHaveBeenCalled(); expect(done).not.toHaveBeenCalled();
    });
});
