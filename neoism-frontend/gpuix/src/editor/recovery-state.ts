import { isRecord } from "../daemon/protocol";

const states = new WeakMap<object, { text: string; dirty: boolean }>();
/** Read the shared CodeBuffer recovery schema without allocating/painting a
 * native editor for a hidden tab. Full validation remains native on first use. */
export function recoveredEditorState(snapshot: unknown) {
  if (!isRecord(snapshot) || snapshot.version !== 1 || typeof snapshot.text !== "string") return;
  const cached = states.get(snapshot); if (cached) return cached;
  const recovery = snapshot.buffer;
  if (!isRecord(recovery) || recovery.version !== 1 || !isRecord(recovery.buffer)) return;
  const buffer = recovery.buffer, lines = buffer.lines, saved = buffer.saved_baseline;
  if (!Array.isArray(lines) || !Array.isArray(saved)) return;
  const dirty = lines.length !== saved.length || lines.some((line, index) => line !== saved[index])
    || (buffer.saved_line_ending != null && buffer.saved_line_ending !== buffer.line_ending)
    || (buffer.saved_trailing_newline != null && buffer.saved_trailing_newline !== buffer.trailing_newline);
  const state = { text: snapshot.text, dirty }; states.set(snapshot, state); return state;
}
