import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { MessageWithParts } from "@neoism/sdk";
import { responseMetadata, displayDuration, displayModel, parseAssistant } from "./responseMetadata";
import { Timeline } from "./Timeline";
import { Markdown, codeLanguage, codeText, copyCode } from "./Markdown";
const user = { info: { id: "u", sessionId: "s", role: "user", author: { name: "Parker" }, time: { created: 1000 } }, parts: [{ id: "up", type: "text", text: "Question" }] };
const info = { id: "a", sessionId: "s", role: "assistant", agent: "build", mode: "plan", modelId: "openai/gpt-5.6", providerId: "openai", parentId: "u", tokens: { output: 100 }, time: { created: 2000, streamed: 3000, completed: 8000 }, finish: "stop" };
const assistant = (patch: Record<string, unknown> = {}, parts: unknown[] = [{ id: "ap", type: "text", text: "Final answer" }]) => ({ info: { ...info, ...patch }, parts });
const footer = (patch: Record<string, unknown> = {}) => responseMetadata([user, assistant(patch)])[1];
describe("response metadata", () => {
    it("uses message fields, parent duration and streaming throughput", () => {
        expect(footer()).toBe("Build · GPT-5.6 · 7.0s · 100.0 tok/s");
        expect(footer({ agent: "", mode: "code_review" })).toContain("Code Review");
        expect(footer({ time: { created: 2000, completed: 500 } })).toBe("Build · GPT-5.6 · 0ms");
        expect(responseMetadata([assistant()])[0]).toBe("Build · GPT-5.6 · 6.0s");
    });
    it.each(["tool-calls", "unknown"])("suppresses intermediate %s unless terminal error exists", finish => {
        expect(footer({ finish })).toBeUndefined();
        expect(footer({ finish, error: null })).toBeUndefined();
        expect(footer({ finish, error: { data: { message: "Failed" } } })).toContain("Build");
    });
    it("matches native absent finish behavior and requires completed", () => {
        expect(footer({ finish: undefined })).toContain("Build");
        expect(footer({ time: { created: 2000 } })).toBeUndefined();
    });
    it.each(["agent", "mode", "modelId", "providerId", "parentId", "finish"])("rejects unknown %s values", field => {
        for (const value of [null, {}, [], 42, true]) expect(parseAssistant({ ...info, [field]: value })).toBeUndefined();
    });
    it("never coerces unknown times or token counts into rates", () => {
        for (const value of [undefined, null, "100", {}, -1, Infinity, NaN, 0.5]) {
            expect(footer({ tokens: { output: value } })).not.toContain("tok/s");
            expect(footer({ time: { ...info.time, streamed: value } })).not.toContain("tok/s");
            expect(footer({ time: { ...info.time, created: value } })).toBeUndefined();
            expect(footer({ time: { ...info.time, completed: value } })).toBeUndefined();
        }
        expect(footer({ parentId: "missing" })).not.toContain("tok/s");
        expect(footer({ tokens: { output: 0 } })).not.toContain("tok/s");
        expect(footer({ time: { ...info.time, streamed: 1000 } })).not.toContain("tok/s");
    });
    it("sums every step in this turn, not elapsed wall time or previous turns", () => {
        const step = assistant({ finish: "tool-calls", tokens: { output: 50 }, time: { created: 1100, streamed: 1600, completed: 1900 } }, [{ type: "tool", tool: "bash" }]);
        expect(responseMetadata([user, step, assistant()])).toEqual([undefined, undefined, "Build · GPT-5.6 · 7.0s · 100.0 tok/s"]);
        expect(responseMetadata([user, assistant({ time: { created: 1100, completed: 1900 } }), assistant()])[2]).not.toContain("tok/s");
        expect(responseMetadata([user, step, user, assistant()])[3]).toBe(footer());
        expect(responseMetadata([user, assistant({}, [{ type: "tool" }])])[1]).toBeUndefined();
    });
    it.each(['Neoism', 'Agent'])("traces %s background subtask continuation to the human request", (prefix) => {
        const task = assistant({}, [{ type: "tool", tool: "task", state: { metadata: { sessionID: "child" } } }]);
        const runtime = { info: { id: "runtime", role: "user", system: `${prefix} runtime notification: background subagent completion.`, time: { created: 6000 } }, parts: [{ type: "text", text: "Done\ntask_id: child (complete)" }] };
        expect(responseMetadata([user, task, runtime, assistant({ parentId: "runtime" })])[3]).toBe("Build · GPT-5.6 · 7.0s · 100.0 tok/s");
    });
    it("formats native duration units and model names", () => {
        expect([0, 999, 1000, 59999, 60000, 3661000, 90000000].map(displayDuration)).toEqual(["0ms", "999ms", "1.0s", "60.0s", "1m 0s", "1h 1m", "1d 1h"]);
        expect(displayModel("openai/gpt-5.6-codex")).toBe("GPT-5.6 Codex");
        expect(displayModel("anthropic/claude-sonnet-4.6")).toBe("Claude Sonnet 4.6");
    });
    it("omits user/assistant headings, keeps author metadata, attaches footer only below last text", () => {
        const a = assistant({}, [{ id: "1", type: "text", text: "First text" }, { id: "2", type: "reasoning", text: "Safe thought", time: { start: 0, end: 1 } }, { id: "3", type: "text", text: "Last text" }]);
        const html = renderToStaticMarkup(<Timeline messages={[user, a] as unknown as MessageWithParts[]} busy={false} loading={false} loadOlder={() => {}} />);
        expect(html).not.toContain('class="message-label"');
        expect(html.match(/<footer/g)).toHaveLength(1);
        expect(html).toContain('Last text</p></div><footer');
        expect(html).toContain('100.0 tok/s</footer></article>');
        expect(html).not.toContain("Safe thought");
        const toolOnly = renderToStaticMarkup(<Timeline messages={[assistant({}, [{ id: "t", type: "tool", tool: "bash", state: { status: "completed", output: "ok" } }])] as unknown as MessageWithParts[]} busy={false} loading={false} loadOlder={() => {}} />);
        expect(toolOnly).not.toContain("<footer");
        expect(toolOnly).not.toContain('class="tc-card"');
    });
});
describe("code cards", () => {
    it("gets language from code child, preserves highlights, defaults to text", () => {
        const html = renderToStaticMarkup(<Markdown text={'```js\nconst n = 1;\n```'} />);
        expect(html).toContain('neo-code-header"><span>js</span>');
        // SSR is plain until the visible code block's Tree-sitter worker replies.
        expect(html).toContain('class="neo-syntax"');
        expect(html).toContain('aria-label="Copy code"');
        expect(renderToStaticMarkup(<Markdown text={'```\nplain\n```'} />)).toContain('neo-code-header"><span>text</span>');
        expect(codeLanguage(<code className="hljs language-rust">hello</code>)).toBe("rust");
    });
    it("copies exact text including trailing newline and converts failures to visible error text", async () => {
        const value = codeText(<code><span className="hljs-keyword">const</span>{" x = 1;\n\n  "}</code>);
        expect(value).toBe("const x = 1;\n\n  ");
        const write = vi.fn(async () => {});
        expect(await copyCode(value, write)).toBeUndefined();
        expect(write).toHaveBeenCalledWith(value);
        expect(await copyCode(value, async () => { throw new Error("denied"); })).toContain("Could not copy code");
        expect(await copyCode(value, () => { throw new Error("unavailable"); })).toContain("permissions");
    });
});
