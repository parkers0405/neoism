// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { MessageWithParts } from "@neoism/sdk";
import { Timeline } from "./Timeline";

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
let root: Root, host: HTMLDivElement;
const text = (id: string, value: string) => ({ id, type: "text", text: value });
const image = (id: string, filename: string) => ({ id, type: "file", filename, mime: "image/png", url: "data:image/png;base64,AQID" });
const documentPart = { id: "doc", type: "file", filename: "report.txt", mime: "text/plain", url: "/v2/artifacts/doc/content" };
const row = (id: string, role: string, parts: unknown[]) => ({ info: { id, sessionId: "s", role, time: { created: 1000 } }, parts }) as MessageWithParts;
const render = (messages: MessageWithParts[]) => act(async () => root.render(<Timeline messages={messages} busy={false} loading={false} loadOlder={() => {}} />));
beforeEach(() => {
    host = document.createElement("div"); document.body.append(host); root = createRoot(host);
    vi.stubGlobal("IntersectionObserver", undefined);
    vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:attachment");
    vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const order = (article: Element) => [...article.children].map(node => node.querySelector("img")?.getAttribute("alt") || node.textContent);

it("presents image DOM above text in the same user bubble without mutating text-first wire parts", async () => {
    const message = row("u", "user", [text("t", "Describe this picture"), image("i", "clipboard.png")]);
    const original = JSON.stringify(message);
    Object.freeze(message.parts);
    await render([message]);
    const article = host.querySelector("article.message.user")!;
    expect(order(article)).toEqual(["clipboard.png", "Describe this picture"]);
    expect(article.querySelectorAll(".attachment-preview-name")).toHaveLength(0);
    expect(article.querySelector("button")?.getAttribute("aria-label")).toBe("View clipboard.png");
    const img = article.querySelector("img")!;
    Object.defineProperties(img, { naturalWidth: { value: 640 }, naturalHeight: { value: 480 } });
    await act(async () => img.dispatchEvent(new Event("load")));
    expect(article.textContent).toBe("Describe this picture");
    expect(article.innerHTML).not.toContain("data:image");
    expect(JSON.stringify(message)).toBe(original);
});
it("stacks multiple images first, retaining the relative order of text and nonimage files", async () => {
    const message = row("u", "user", [text("t1", "First caption"), documentPart, image("i1", "one.png"), text("t2", "Second caption"), image("i2", "two.png")]);
    await render([message]);
    const article = host.querySelector("article")!;
    expect(order(article)).toEqual(["one.png", "two.png", "First caption", "report.txt", "Second caption"]);
    expect([...article.querySelectorAll(".attachment-preview-name")].map(node => node.textContent)).toEqual(["report.txt"]);
});
it("leaves assistant and runtime part ordering and chronological footer assignment unchanged", async () => {
    const user = row("u", "user", [text("t", "Question"), image("i", "question.png")]);
    const runtime = row("msg_background_completion_job", "user", [text("notice", "Background shell task finished."), documentPart, image("ri", "runtime.png")]);
    const assistant = {
        ...row("a", "assistant", [text("answer", "Answer"), image("ai", "answer.png")]),
        info: { id: "a", sessionId: "s", role: "assistant", agent: "build", modelId: "gpt-5.6", providerId: "openai", parentId: "u", time: { created: 3000, completed: 8000 }, finish: "stop" },
    } as MessageWithParts;
    await render([user, runtime, assistant]);
    expect([...host.querySelectorAll("article")].map(node => node.getAttribute("data-message-id"))).toEqual(["u", "msg_background_completion_job", "a"]);
    const runtimeArticle = host.querySelector(".runtime-message")!;
    expect(order(runtimeArticle).slice(1)).toEqual(["report.txt", "runtime.png"]);
    const response = host.querySelector('[data-message-id="a"]')!;
    expect(response.children[0].textContent).toBe("Answer");
    expect(response.children[1].tagName).toBe("FOOTER");
    expect(response.children[1].textContent).toContain("Build · GPT-5.6 · 7.0s");
    expect(response.children[2].querySelector("img")?.alt).toBe("answer.png");
    expect(host.querySelectorAll("footer")).toHaveLength(1);
});
