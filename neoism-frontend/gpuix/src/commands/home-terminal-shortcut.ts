import type { BindingKeyEvent } from "./KeyboardBindings";

/** Home has no text input: Space arms a short, cancellable leader chord. */
export class HomeTerminalShortcut {
  private armedAt: number | undefined;
  reset() { this.armedAt = undefined; }
  key(event: BindingKeyEvent, now = Date.now()): "pending" | "split-terminal" | undefined {
    const mods = event.modifiers, key = event.keyChar || event.key || "";
    const armed = this.armedAt !== undefined && now - this.armedAt <= 800;
    this.reset();
    if (mods?.ctrl || mods?.alt || mods?.cmd || mods?.shift) return;
    if (key === " " || key === "space") { this.armedAt = now; return "pending"; }
    if (armed && key === "h") return "split-terminal";
  }
}
