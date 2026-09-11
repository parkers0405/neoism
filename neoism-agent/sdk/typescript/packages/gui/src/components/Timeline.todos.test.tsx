// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it } from "vitest";
import type { MessageWithParts } from "@neoism/sdk";
import { Timeline } from "./Timeline";

(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
let host: HTMLDivElement, root: Root;
beforeEach(() => { host = document.createElement("div"); document.body.append(host); root = createRoot(host); });
afterEach(() => { act(() => root.unmount()); host.remove(); });
const plan = (id: string, created: number, status = "pending"): MessageWithParts => ({
    info: {id,role:"assistant",sessionId:"s",time:{created}},
    parts: [{id:`p-${id}`,type:"tool",sessionId:"s",messageId:id,callId:`c-${id}`,tool:"todowrite",state:{
        status:"completed",input:{},output:JSON.stringify([{id:"item",content:"Check the selected directory",status,priority:"high"}]),title:"Tasks",metadata:{},time:{start:created,end:created+1},
    }}],
});
const response: MessageWithParts = {info:{id:"answer",role:"assistant",sessionId:"s",time:{created:2}},parts:[{id:"text",type:"text",sessionId:"s",messageId:"answer",text:"Later response"}]};
const render = (messages: MessageWithParts[], liveParts: Map<string, Set<string>>) => act(() => root.render(
    <Timeline messages={messages} sessionId="s" busy={false} loading={false} loadOlder={() => {}} liveParts={liveParts} showActivity={false} />,
));
it("keeps the checklist at its original message while later messages and todo updates arrive", () => {
    const first = plan("plan", 1), live = new Map([["plan",new Set(["p-plan"])]]);
    render([first], live);
    const panel = host.querySelector(".todo-panel")!;
    const checkbox = panel.querySelector('[role="checkbox"]')!;
    expect(panel.closest("[data-message-id]")?.getAttribute("data-message-id")).toBe("plan");
    render([first,response], live);
    expect(host.querySelector(".todo-panel")).toBe(panel);
    expect(panel.compareDocumentPosition(host.querySelector('[data-message-id="answer"]')!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const update = plan("update", 3, "completed");
    render([first,response,update], new Map([...live,["update",new Set(["p-update"])]]));
    expect(host.querySelector(".todo-panel")).toBe(panel);
    expect(panel.closest("[data-message-id]")?.getAttribute("data-message-id")).toBe("plan");
    expect(panel.querySelector('[role="checkbox"]')).toBe(checkbox);
    expect(checkbox.getAttribute("aria-checked")).toBe("true");
    expect(host.querySelectorAll(".todo-panel")).toHaveLength(1);
});
it("still hides historical inline tool state after returning to the conversation", () => {
    const first = plan("plan", 1);
    render([first], new Map([["plan",new Set(["p-plan"])]]));
    expect(host.querySelector(".todo-panel")).not.toBeNull();
    render([first,response], new Map());
    expect(host.querySelector(".todo-panel")).toBeNull();
    expect(host.textContent).toContain("Later response");
});
