import { describe, expect, it } from "vitest";
import { subagentView } from "./subagent-view";

describe("session-owned subagent view", () => {
    it("composes new and known root chats only", () => {
        expect(subagentView(undefined).canCompose).toBe(true);
        expect(subagentView("root", { id: "root" }).canCompose).toBe(true);
        expect(subagentView("root").canCompose).toBe(false);
        expect(subagentView("root", { id: "other" }).metadataLoading).toBe(true);
    });
    it("uses a distinct runtime root, otherwise the actual parent", () => {
        const child = { id: "child", parentId: "parent" };
        expect(subagentView("child", child, { rootSessionId: "root" })).toMatchObject({ canCompose: false, returnId: "root", backLabel: "Back to main chat" });
        expect(subagentView("child", child, { rootSessionId: "child" })).toMatchObject({ returnId: "parent", backLabel: "Back to parent chat" });
        expect(subagentView("child", child).returnId).toBe("parent");
        expect(subagentView("root", { id: "root" }, { rootSessionId: "other" }).isChild).toBe(false);
    });
});
