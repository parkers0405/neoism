export type ShellEvent = { kind: "prompt" | "input" | "start" | "end"; status?: number } | { kind: "directory"; path: string } | { kind: "clear"; mode: "screen" | "history" | "reset" };

/** Observe shell metadata without interpreting or removing terminal bytes. */
export class ShellEvents {
  private state: "text" | "escape" | "csi" | "osc" | "osc-escape" | "string" | "string-escape" = "text";
  private payload: number[] = [];
  private overflow = false;
  reset() { this.state = "text"; this.payload = []; this.overflow = false; }
  feed(bytes: Uint8Array, emit: (end: number, event: ShellEvent) => void) {
    for (let i = 0; i < bytes.length; i++) {
      const byte = bytes[i];
      if (this.state === "text") { if (byte === 27) this.state = "escape"; continue; }
      if (this.state === "escape") {
        if (byte === 99) emit(i + 1, { kind: "clear", mode: "reset" });
        this.state = byte === 91 ? "csi" : byte === 93 ? "osc" : [80, 88, 94, 95].includes(byte) ? "string" : byte === 27 ? "escape" : "text";
        this.payload = []; this.overflow = false; continue;
      }
      // Observe only ED, not cursor movement or screen state. Native VT still
      // consumes every byte, including cancelled and oversized sequences.
      if (this.state === "csi") {
        if (byte === 24 || byte === 26) { this.reset(); continue; }
        if (byte === 27) { this.reset(); this.state = "escape"; continue; }
        if (byte >= 64 && byte <= 126) {
          if (!this.overflow && byte === 74 && this.payload.length === 1 && (this.payload[0] === 50 || this.payload[0] === 51))
            emit(i + 1, { kind: "clear", mode: this.payload[0] === 50 ? "screen" : "history" });
          this.reset(); continue;
        }
        if (this.payload.length < 32) this.payload.push(byte); else this.overflow = true;
        continue;
      }
      if (this.state === "string" || this.state === "string-escape") {
        this.state = this.state === "string-escape" && byte === 92 ? "text" : byte === 27 ? "string-escape" : "string"; continue;
      }
      if (byte === 24 || byte === 26) { this.reset(); continue; }
      if (byte === 7 || this.state === "osc-escape" && byte === 92) {
        if (!this.overflow) {
          const value = new TextDecoder().decode(Uint8Array.from(this.payload));
          const match = /^(?:133|633);([ABCD])(?:;(\d+))?(?:;.*)?$/.exec(value);
          if (match) emit(i + 1, { kind: ({ A: "prompt", B: "input", C: "start", D: "end" } as const)[match[1] as "A" | "B" | "C" | "D"], ...(match[2] === undefined ? {} : { status: Number(match[2]) }) });
          else if (value.startsWith("7;file://")) {
            try { const url = new URL(value.slice(2)); if (url.protocol === "file:") emit(i + 1, { kind: "directory", path: decodeURIComponent(url.pathname) }); } catch { /* Ignore malformed shell metadata. */ }
          }
        }
        this.reset(); continue;
      }
      if (this.state === "osc-escape") {
        this.reset(); this.state = byte === 27 ? "escape" : "text"; continue;
      }
      if (byte === 27) { this.state = "osc-escape"; continue; }
      if (this.payload.length < 8192) this.payload.push(byte); else this.overflow = true;
    }
  }
}