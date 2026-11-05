import type { KeyEvent } from "@gpuix/solid";
import type { TerminalBuffer } from "./buffer";
import type { SearchHost } from "../native-core";
import { readClipboard, writeClipboard } from "../clipboard";

/** Keep native key protocols and application shortcuts identical on both views. */
export function terminalInput(options: {
  buffer: TerminalBuffer; send: (bytes: Uint8Array) => unknown; report: (error: unknown) => void;
  custom?: (event: KeyEvent) => boolean; search: (kind: "/" | "?", host: SearchHost) => void;
  focus: () => void; activity: () => void; disposed: () => boolean; selection?: () => string;
}) {
  const { buffer, send, report } = options, intercepted = new Set<string>();
  return (event: KeyEvent, released = false) => {
    const key = event.key?.toLowerCase() || "";
    if (released && intercepted.delete(key)) { event.preventDefault(); event.stopPropagation(); return; }
    const mods = event.modifiers || { ctrl: false, cmd: false, shift: false, alt: false }, primary = mods.ctrl || mods.cmd;
    if (!released && options.custom?.(event)) { intercepted.add(key); event.preventDefault(); event.stopPropagation(); return; }
    if (key === "c" && mods.ctrl && !mods.shift && !mods.alt && !mods.cmd) {
      event.preventDefault(); event.stopPropagation();
      const input = { key: "c", char: "c", ctrl: true, repeat: event.isHeld, released }, wasVi = buffer.viMode;
      let bytes = buffer.key(input);
      if (!released && wasVi && !buffer.viMode) bytes = buffer.key(input);
      if (bytes.length) send(bytes);
      if (!released) { options.activity(); buffer.scrollToBottom(); }
      return;
    }
    const clipboardChord = !mods.alt && ((mods.cmd && !mods.ctrl) || (mods.ctrl && mods.shift && !mods.cmd));
    const shiftInsert = key === "insert" && mods.shift && !mods.ctrl && !mods.alt && !mods.cmd;
    if (clipboardChord && (key === "c" || key === "v") || shiftInsert) {
      event.preventDefault(); event.stopPropagation();
      if (!released) {
        intercepted.add(key);
        if (key === "c") { const selected = options.selection?.() || buffer.selectedText; if (selected) void writeClipboard(selected).catch(report); }
        else void readClipboard().then(text => {
          if (options.disposed()) return;
          const bytes = buffer.paste(text); if (bytes.length) { options.activity(); send(bytes); buffer.scrollToBottom(); }
        }).catch(report);
      }
      return;
    }
    if (mods.ctrl && (key === "tab" || mods.shift && ["r", "d", "[", "]", "left", "right", "up", "down", "w"].includes(key))) return;
    if (mods.alt && ["e", "n", "c", "h"].includes(key)) return;
    if (mods.ctrl && ["`", "p", "t"].includes(key)) return;
    const char = event.keyChar || (key.length === 1 ? key : "");
    if (buffer.viMode && !primary && ["/", "?"].includes(char)) {
      event.preventDefault(); event.stopPropagation();
      if (!released) {
        buffer.search("start", { forward: char === "/" });
        options.search(char as "/" | "?", { preview: pattern => buffer.search("preview", { pattern }), commit: () => buffer.search("commit"), cancel: () => buffer.search("cancel"), focus: options.focus });
      }
      return;
    }
    const bytes = buffer.key({ key, char, ...event.modifiers, repeat: event.isHeld, released });
    if (buffer.pendingClipboard !== undefined) { void writeClipboard(buffer.pendingClipboard).catch(report); buffer.pendingClipboard = undefined; }
    if (buffer.lastKeyHandled) { event.preventDefault(); event.stopPropagation(); if (!released) options.activity(); }
    if (bytes.length) { event.preventDefault(); event.stopPropagation(); send(bytes); if (!released) { options.activity(); buffer.scrollToBottom(); } }
  };
}