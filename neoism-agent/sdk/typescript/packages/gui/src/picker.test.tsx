import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { Picker } from "./components/Picker";
import { ComposerPanel } from "./components/ComposerPanel";

describe("above-composer follow-up panels", () => {
    it("renders model choices as an anchored panel, not a centered modal", () => {
        const html = renderToStaticMarkup(<Picker title="Models" choices={[
            {id:"provider/model",label:"Model",description:"Provider"},
        ]} choose={vi.fn()} close={vi.fn()} />);
        expect(html).toContain('class="composer-panel"');
        expect(html).toContain('aria-label="Models"');
        expect(html).toContain('role="listbox"');
        expect(html).toContain('aria-selected="true"');
        expect(html).not.toContain("<dialog");
    });
    it("uses the same nonmodal panel for multistep provider and MCP content", () => {
        const html = renderToStaticMarkup(<ComposerPanel title="MCP servers" close={vi.fn()}>
            <button>Connect server</button>
        </ComposerPanel>);
        expect(html).toContain('aria-label="Close MCP servers"');
        expect(html).toContain("Connect server");
        expect(html).not.toContain("<dialog");
    });
});
