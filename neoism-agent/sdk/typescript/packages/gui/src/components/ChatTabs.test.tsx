import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { Session } from "@neoism/sdk";
import { ChatTabs, chatTabTitle, tabWheelDelta } from "./ChatTabs";
import type { ChatTab } from "../tabs";

const tab = (key: string, draft = ""): ChatTab => ({key, draft, explicit:{}});
describe("chat tab presentation", () => {
    it("maps vertical wheels and horizontal trackpads into horizontal tab scrolling", () => {
        expect(tabWheelDelta({deltaX:0,deltaY:40,deltaMode:0},300)).toBe(40);
        expect(tabWheelDelta({deltaX:-60,deltaY:5,deltaMode:0},300)).toBe(-60);
        expect(tabWheelDelta({deltaX:0,deltaY:3,deltaMode:1},300)).toBe(72);
        expect(tabWheelDelta({deltaX:0,deltaY:-1,deltaMode:2},300)).toBe(-300);
    });
    it("keeps New tab until the session has a real title, ignoring draft and seed timestamps", () => {
        expect(chatTabTitle(tab("a"))).toBe("New tab");
        expect(chatTabTitle({...tab("a", "Review the parser\nMore details"), metadata:{title:"New session - 1789016003938"} as Session})).toBe("New tab");
        expect(chatTabTitle({...tab("a"), metadata:{title:"Parser fixes"} as Session})).toBe("Parser fixes");
    });
    it("exposes the selected tab and its panel without making every tab a tab stop", () => {
        const html = renderToStaticMarkup(<ChatTabs tabs={[tab("a"),tab("b")]} active="b" activate={vi.fn()} close={vi.fn()} add={vi.fn()} />);
        expect(html).toContain('role="tablist"');
        expect(html).toContain('id="chat-tab-b" aria-controls="chat-panel-b" aria-selected="true" tabindex="0"');
        expect(html).toContain('id="chat-tab-a" aria-selected="false" tabindex="-1"');
        expect(html).toContain("Close tab (keeps chat)");
        expect(html).toContain('<span class="chat-tab-label">New tab</span></button>');
    });
});
