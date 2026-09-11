import { describe, it, expect, vi } from "vitest";
import { commands } from "./generated/commands";
import {
    executeCommand,
    filterCommands,
    moveSelection,
    parseCommand,
} from "./commands";
describe("native command picker", () => {
    it("offers every native entry", () =>
        expect(filterCommands(commands, "/")).toHaveLength(32));
    it("finds aliases and canonicalizes /models", () => {
        expect(
            filterCommands(commands, "/models").some(
                (c) => c.name === "/model",
            ),
        ).toBe(true);
        expect(parseCommand("/models provider/model", commands)).toEqual({
            name: "model",
            args: "provider/model",
            original: "/models provider/model",
        });
    });
    it("wraps arrows and handles empty filters", () => {
        expect(moveSelection(0, -1, 3)).toBe(2);
        expect(moveSelection(2, 1, 3)).toBe(0);
        expect(moveSelection(0, 1, 0)).toBe(0);
    });
    it("forwards unknown server commands without dropping arguments", async () => {
        const forward = vi.fn();
        await executeCommand("/deploy staging --safe", commands, {
            local: async () => false,
            forward,
            confirm: () => true,
        });
        expect(forward).toHaveBeenCalledWith("/deploy staging --safe");
    });
    it("never forwards handled commands", async () => {
        const forward = vi.fn();
        await executeCommand("/models", commands, {
            local: async (name) => name === "model",
            forward,
            confirm: () => true,
        });
        expect(forward).not.toHaveBeenCalled();
    });
    it("requires confirmation for every dangerous alias", async () => {
        for (const command of [
            "/yolo",
            "/dangerously-skip-permissions",
            "/skip-permissions",
        ]) {
            const local = vi.fn(),
                forward = vi.fn();
            await executeCommand(command, commands, {
                local,
                forward,
                confirm: () => false,
            });
            expect(local).not.toHaveBeenCalled();
            expect(forward).not.toHaveBeenCalled();
        }
    });
    it("propagates operation errors for visible UX", async () => {
        await expect(
            executeCommand("/test", commands, {
                local: async () => false,
                forward: async () => {
                    throw new Error("403");
                },
                confirm: () => true,
            }),
        ).rejects.toThrow("403");
    });
});
