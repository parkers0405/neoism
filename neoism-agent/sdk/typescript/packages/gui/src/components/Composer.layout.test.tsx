// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { Composer, type ComposerProps } from "./Composer";

it("preserves the live input/caret across focus and picker interaction, including native cancel", () => {
    (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
    const host = document.createElement("div"); document.body.append(host);
    const outside = document.createElement("button"); document.body.append(outside);
    const root = createRoot(host);
    const props: ComposerProps = { busy: false, commands: [], send: vi.fn(async () => {}), abort: vi.fn(),
        model: "model", agent: "Build", thinking: "High", openPicker: vi.fn(), showFooter: false };
    const render = () => act(() => root.render(<Composer {...props} />));
    const layout = () => host.querySelector<HTMLElement>(".native-composer")!.dataset.layout;
    try {
        render(); expect(layout()).toBe("compact");
        const input = host.querySelector("textarea")!;
        act(() => input.focus()); expect(layout()).toBe("expanded");
        props.draft = "existing draft"; render();
        input.setSelectionRange(3, 3);
        act(() => outside.focus()); expect(layout()).toBe("expanded");
        act(() => input.focus()); expect(input.selectionStart).toBe(3);
        expect(host.querySelector("textarea")).toBe(input);
        props.draft = ""; render(); act(() => outside.focus()); expect(layout()).toBe("compact");
        act(() => host.querySelector<HTMLButtonElement>('[aria-label="Add attachments"]')!.click());
        expect(layout()).toBe("expanded");
        act(() => host.querySelector('input[type="file"]')!.dispatchEvent(new Event("cancel")));
        expect(layout()).toBe("compact");
        act(() => input.focus());
        props.pickerOpen = true; render(); act(() => outside.focus()); expect(layout()).toBe("expanded");
        props.pickerOpen = false; render(); expect(layout()).toBe("compact");
        act(() => host.querySelector<HTMLElement>(".native-composer-surface")!.click());
        expect(document.activeElement).toBe(input); expect(layout()).toBe("expanded");
    } finally {
        act(() => root.unmount()); host.remove(); outside.remove();
    }
});
