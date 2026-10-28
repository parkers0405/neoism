import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { parse, type ParseError } from "jsonc-parser";
import { coreCall } from "../native-core";
export type KeyboardAction = { kind: "action"; action: string; argument?: number; program?: string; args?: string[] } | { kind: "escape"; text: string };
export interface BindingState { vi?: boolean; appcursor?: boolean; appkeypad?: boolean; alt?: boolean; search?: boolean; disambiguate?: boolean; allKeysAsEsc?: boolean }
export interface BindingKeyEvent { key?: string; keyChar?: string; modifiers?: { ctrl?: boolean; shift?: boolean; alt?: boolean; cmd?: boolean }; location?: "standard" | "numpad" }
interface Binding { key: string; location: "standard" | "numpad"; modifiers: number; required: number; excluded: number; action: KeyboardAction }
const modes = { appcursor: 1, appkeypad: 2, alt: 4, vi: 8, search: 16, disambiguate: 32, allKeysAsEsc: 64 };
const modifiers = { control: 1, shift: 2, alt: 4, option: 4, command: 8, super: 8 };
const aliases: Record<string, string> = { home: "home", space: "space", delete: "delete", esc: "escape", insert: "insert", pageup: "pageup", pagedown: "pagedown", end: "end",
  up: "up", back: "backspace", down: "down", left: "left", right: "right", colon: ":", return: "enter", tab: "tab" };
const numpad: Record<string, string> = { enter: "enter", add: "+", comma: ",", decimal: ".", divide: "/", equals: "=", subtract: "-", multiply: "*", ...Object.fromEntries(Array.from({ length: 10 }, (_, n) => [String(n), String(n)])) };
const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);
function conflicts(a: Binding, b: Binding) {
  if (a.key !== b.key || a.location !== b.location || a.modifiers !== b.modifiers) return false;
  if (!((a.required || 127) & (b.required || 127))) return false;
  return !(a.required & b.excluded) && !(b.required & a.excluded);
}
function normalizeEvent(key: string) {
  const value = key.toLowerCase();
  return ({ " ": "space", return: "enter", esc: "escape", arrowup: "up", arrowdown: "down", arrowleft: "left", arrowright: "right", back: "backspace" } as Record<string, string>)[value] || value;
}
/** Config overrides obey the desktop's modifier/mode conflict and priority rules. */
export class KeyboardBindings {
  private bindings: Binding[] = [];
  readonly warnings: string[] = [];
  constructor(input: unknown = []) {
    const rows = Array.isArray(input) ? input : record(input) && record(input.keybinds) && Array.isArray(input.keybinds.keys) ? input.keybinds.keys : [];
    for (const row of rows) {
      if (!record(row) || typeof row.key !== "string") { this.warnings.push("Ignored a key binding without a key."); continue; }
      const raw = row.key.toLowerCase();
      const location = raw.startsWith("numpad") ? "numpad" : "standard";
      const key = Array.from(raw).length === 1 ? raw : location === "numpad" ? numpad[raw.slice(6)] : aliases[raw];
      if (!key) { this.warnings.push(`Ignored unsupported key binding '${row.key}'.`); continue; }
      let mods = 0, required = 0, excluded = 0;
      for (const value of (typeof row.with === "string" ? row.with : "").split("|")) mods |= modifiers[value.trim().toLowerCase() as keyof typeof modifiers] || 0;
      // These are exactly the modes accepted by desktop config conversion.
      for (const value of (typeof row.mode === "string" ? row.mode : "").split("|")) {
        const name = value.trim().toLowerCase(), negate = name.startsWith("~"), plain = negate ? name.slice(1) : name;
        const bit = ["appcursor", "appkeypad", "alt", "vi"].includes(plain) ? modes[plain as keyof typeof modes] : 0;
        if (negate) excluded |= bit; else required |= bit;
      }
      const action: KeyboardAction = typeof row.esc === "string" && row.esc.length > 0 ? { kind: "escape", text: row.esc }
        : coreCall("commands.parse-action", { action: typeof row.action === "string" ? row.action : "" });
      const binding: Binding = { key, location, modifiers: mods, required, excluded, action };
      this.bindings = this.bindings.filter((existing) => !conflicts(existing, binding));
      this.bindings.push(binding);
    }
  }
  static fromConfig(path?: string) {
    if (!path) {
      const args = process.argv.slice(2), index = args.indexOf("--config");
      path = index >= 0 ? args[index + 1] : join(process.env.XDG_CONFIG_HOME || join(homedir(), ".config"), "neoism", "config.json");
    }
    if (!path || !existsSync(path)) return new KeyboardBindings();
    try {
      const errors: ParseError[] = [], value = parse(readFileSync(path, "utf8"), errors, { allowTrailingComma: true });
      const result = new KeyboardBindings(errors.length ? [] : value);
      if (errors.length) result.warnings.push("Key bindings were not loaded because the config contains invalid JSONC.");
      return result;
    } catch { const result = new KeyboardBindings(); result.warnings.push("Key bindings could not be read."); return result; }
  }
  /** Nonempty overrides replace matching defaults, including explicit None. */
  match(event: BindingKeyEvent, state: BindingState = {}): readonly KeyboardAction[] {
    const key = normalizeEvent(event.key || "");
    const mods = (event.modifiers?.ctrl ? 1 : 0) | (event.modifiers?.shift ? 2 : 0) | (event.modifiers?.alt ? 4 : 0) | (event.modifiers?.cmd ? 8 : 0);
    let active = 0; for (const [name, bit] of Object.entries(modes)) if (state[name as keyof BindingState]) active |= bit;
    return this.bindings.filter((binding) => binding.key === key && binding.location === (event.location || "standard") && binding.modifiers === mods
      && (active & binding.required) === binding.required && !(active & binding.excluded)).map((binding) => binding.action);
  }
  get size() { return this.bindings.length; }
}

/** Native config action spellings differ from a few palette action spellings. */
export function paletteActionForBinding(action: string): string {
  return ({ TabCreateNew: "CreateWorkspace", WorkspaceTerminalTabCreateNew: "TabCreate", TabCloseCurrent: "TabClose", TabCloseUnfocused: "TabCloseUnfocused",
    SelectPrevBufferTab: "SelectPrevTab", SelectNextBufferTab: "SelectNextTab" } as Record<string, string>)[action] || action;
}
