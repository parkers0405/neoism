import { describe, it, expect, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { Markdown } from "./components/Markdown";
import { nativeCommand } from "./nativeCommands";
import type { NeoismClient } from "@neoism/sdk";
import { errorMessage } from "./types";
describe("safe markdown", () => {
    it("contains wide tables in a horizontal scroll region with following text outside it", () => {
        const html = renderToStaticMarkup(<Markdown text={"| Name | Value |\n| --- | --- |\n| first | second |\n\nAfter the table."} />);
        expect(html).toContain('class="markdown-table-scroll"');
        expect(html).toContain('aria-label="Scrollable table"');
        expect(html.indexOf("</table>")).toBeLessThan(html.indexOf("After the table."));
    });
    it("does not execute raw HTML or javascript links", () => {
        const html = renderToStaticMarkup(
            <Markdown
                text={
                    "<script>alert(1)</script>\n\n[unsafe](javascript:alert%281%29)\n\n<img src=x onerror=alert(1)>"
                }
            />,
        );
        expect(html).not.toContain("<script");
        expect(html).not.toContain("javascript:");
        expect(html).not.toContain("onerror");
    });
    it("highlights code and supplies copy affordance", () => {
        const html = renderToStaticMarkup(
            <Markdown text={'```js\nconst x = "hello";\n```'} />,
        );
        expect(html).toContain("Copy code");
        // SSR stays escaped/plain; visible blocks load Tree-sitter asynchronously.
        // Real WASM captures are covered by syntax/engine.test.ts.
        expect(html).toContain('class="neo-syntax"');
        expect(html).toContain("hello");
    });
    it("does not load remote tracking images", () =>
        expect(
            renderToStaticMarkup(
                <Markdown text={"![tracking](https://example.com/pixel)"} />,
            ),
        ).not.toContain("<img"));
});
describe("native SDK commands", () => {
    it("rejects a pending question before permissions", async () => {
        const reject = vi.fn(async () => true);
        const list = vi.fn(async () => []);
        const client = {
            interactions: {
                questions: { list: async () => [{ id: "q" }], reject },
                permissions: { list },
            },
        } as unknown as NeoismClient;
        const show = vi.fn();
        expect(
            await nativeCommand("reject", "", {
                client,
                id: "s",
                directory: "",
                show,
            }),
        ).toBe(true);
        expect(reject).toHaveBeenCalledWith("q");
        expect(list).not.toHaveBeenCalled();
    });
    it("requires a session for session operations", async () => {
        await expect(
            nativeCommand("goal", "hello", {
                client: {} as NeoismClient,
                directory: "",
                show: vi.fn(),
            }),
        ).rejects.toThrow("Open a chat");
    });
    it("propagates capability failures rather than succeeding silently", async () => {
        const client = {
            plugins: {
                use: async () => {
                    throw new Error("Capability unavailable");
                },
            },
        } as unknown as NeoismClient;
        await expect(
            nativeCommand("mcp", "", { client, directory: "", show: vi.fn() }),
        ).rejects.toThrow("Capability unavailable");
    });
    it("does not consume unknown commands", async () =>
        expect(
            await nativeCommand("deploy", "prod", {
                client: {} as NeoismClient,
                directory: "",
                show: vi.fn(),
            }),
        ).toBe(false));
    it("provides auth, conflict and network recovery guidance", () => {
        expect(errorMessage({ status: 403 })).toContain("token");
        expect(errorMessage({ status: 412 })).toContain("Reload");
        expect(errorMessage(new Error("Failed to fetch"))).toContain("CORS");
    });
});
