import { describe, it, expect } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { DefinitionEditor } from "./components/DefinitionEditor";
import { changeFrequency, type Editor } from "./management";
describe("definition forms", () => {
    function flow(frequency: string) {
        return renderToStaticMarkup(<DefinitionEditor editor={{ kind: "workflows", id: "flow", existing: true, revision: "r1", definition: { id: "flow", name: "Flow", prompt: "Do work", active: false, schedule: changeFrequency({ frequency: "daily", interval: 1, timezone: "UTC" }, frequency) } }} change={() => {}} />);
    }
    it("hourly uses minute rather than time", () => {
        expect(flow("hourly")).toContain("Minute of the hour");
        expect(flow("hourly")).not.toContain('type="time"');
    });
    it("weekly exposes all weekdays", () => {
        expect(flow("weekly").match(/aria-pressed=/g)).toHaveLength(7);
        expect(flow("weekly")).not.toContain("Day of month");
    });
    it("monthly and once expose their required fields", () => {
        expect(flow("monthly")).toContain("Day of month");
        expect(flow("once")).toContain('type="date"');
        expect(flow("once")).not.toContain("Interval");
    });
    it("exposes real runtime controls", () => {
        for (const label of ["Timezone", "Maximum attempts", "Backoff", "Permissions", "Model ID", "Concurrency", "Execution directory"]) expect(flow("daily")).toContain(label);
    });
    it("locks skill scope while retaining support files", () => {
        const editor: Editor = { kind: "skills", existing: true, id: "skill", scope: "installation", definition: { scope: "installation", content: "Instructions", files: { "scripts/helper.py": "print('retained')" } } };
        const html = renderToStaticMarkup(<DefinitionEditor editor={editor} change={() => {}} />);
        expect(html).toContain("Global skill · scope cannot be changed");
        expect(html).toContain("scripts/helper.py");
        expect(html).toContain("retained");
        expect(html).not.toContain("Advanced JSON");
    });
});
