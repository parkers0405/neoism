// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { SidebarSubagents } from "./ChatDetails";
import { emptySubagents } from "./subagentController";
import { readFileSync } from "node:fs";
const css = readFileSync("src/components/sidebar-native.css", "utf8");
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

it("left-aligns short, wrapped and unbroken titles in equal-width rows and retains navigation", () => {
    const style = document.createElement("style");
    style.textContent = `button { display: inline-flex; justify-content: center; padding: 8px; } .right-sidebar button { padding-left: 0; } ${css}`;
    document.head.append(style);
    const host = document.createElement("div"); document.body.append(host);
    const root = createRoot(host), open = vi.fn();
    const titles = ["Inspect", "Restructure settings page navigation with multiple sections", "x".repeat(160)];
    try {
        act(() => root.render(<aside className="right-sidebar sidebar-native"><SidebarSubagents open={open} data={{ ...emptySubagents(), rows: titles.map((title, i) => ({ id: `task-${i}`, sessionId: `child-${i}`, title, agent: "build", status: "running", nested: false, stoppable: true })) }} /></aside>));
        const buttons = host.querySelectorAll("button");
        expect(buttons).toHaveLength(3);
        buttons.forEach((button, i) => {
            expect(button.textContent).toBe(titles[i]);
            const computed = getComputedStyle(button);
            expect(computed.justifyContent).toBe("flex-start");
            expect(computed.textAlign).toBe("left");
            expect(computed.width).toBe("100%");
            expect(computed.boxSizing).toBe("border-box");
            expect(computed.padding).toBe("6px 0px");
            expect(computed.whiteSpace).toBe("normal");
            expect(computed.overflowWrap).toBe("anywhere");
            act(() => button.click());
            expect(open).toHaveBeenLastCalledWith(`child-${i}`);
        });
    } finally { act(() => root.unmount()); host.remove(); style.remove(); }
});
