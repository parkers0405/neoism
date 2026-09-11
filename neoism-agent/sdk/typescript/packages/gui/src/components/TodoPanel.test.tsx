// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { afterEach, beforeEach, expect, it } from "vitest";
import { MarkdownTodoInput, TodoPanel, TodoToolPart } from "./TodoPanel";
import { parseTodos } from "../todoHelpers";
let root: Root, el: HTMLDivElement;
beforeEach(() => { (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true; el = document.createElement("div"); document.body.append(el); root = createRoot(el); });
afterEach(() => { act(() => root.unmount()); el.remove(); });
it("updates the same row/glyph across assistant check-offs and retains all completed rows", () => {
    const render = (status: string) => act(() => root.render(<TodoPanel todos={parseTodos([{ content: "Write tests", status }])!} />));
    render("pending"); const row = el.querySelector("li"), glyph = el.querySelector("svg");
    expect(el.querySelector('[role="checkbox"]')?.getAttribute("aria-checked")).toBe("false");
    render("in_progress"); expect(el.textContent).toContain("In progress");
    expect(el.querySelector('[role="checkbox"]')?.getAttribute("aria-checked")).toBe("mixed");
    render("completed"); expect(el.querySelector("li")).toBe(row); expect(el.querySelector("svg")).toBe(glyph);
    expect(el.querySelector('[role="checkbox"]')?.getAttribute("aria-checked")).toBe("true");
    expect(el.textContent).toContain("1/1"); expect(el.textContent).toContain("Write tests");
    expect(el.querySelector("button, input, details")).toBeNull();
    expect(el.querySelector('[role="checkbox"]')?.getAttribute("aria-readonly")).toBe("true");
});
it("preserves real Markdown lists with styled readonly checks, including user checklists", () => {
    act(() => root.render(<ReactMarkdown remarkPlugins={[remarkGfm]} components={{ input: MarkdownTodoInput }}>{"- [ ] user task\n- [x] done task\n- ordinary list"}</ReactMarkdown>));
    expect(el.querySelectorAll("li")).toHaveLength(3); expect(el.querySelectorAll('[role="checkbox"]')).toHaveLength(2);
    expect(el.querySelector("input")).toBeNull(); expect(el.textContent).toContain("user task"); expect(el.textContent).toContain("ordinary list");
});
it("renders the native inline left-rule variant and empty update without raw JSON", () => {
    act(() => root.render(<TodoToolPart part={{ type: "tool", tool: "todowrite", id: "p", callId: "c", sessionId: "s", messageId: "m", state: { status: "completed", input: {}, output: "[]", metadata: {}, time: { start: 0 }, title: "0 todos" } }} />));
    expect(el.textContent).toBe("Tasks updated"); expect(el.textContent).not.toContain("[]");
});
