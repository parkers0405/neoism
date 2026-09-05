export interface TerminalDirectoryPlan {
  payload: Uint8Array;
  /** Deterministic lexical cwd; null means wait for authoritative SessionCwd. */
  optimisticCwd: string | null;
}

export interface TerminalDirectoryTargetOptions {
  associated: string | null;
  active: string | null;
  recent: string | null;
  tabSessions: readonly (string | null | undefined)[];
  isLive: (sessionId: string) => boolean;
}

/** Select once, in focus/MRU/tab order. The caller captures the returned id;
 * later focus changes therefore cannot redirect the submitted command. */
export function resolveTerminalDirectoryTarget(options: TerminalDirectoryTargetOptions): string | null {
  const ordered = [options.associated, options.active, options.recent, ...options.tabSessions];
  const seen = new Set<string>();
  for (const candidate of ordered) {
    if (candidate && !seen.has(candidate) && options.isLive(candidate)) return candidate;
    if (candidate) seen.add(candidate);
  }
  return null;
}

export class ExactSyntheticEchoFilter {
  private expected: Uint8Array | null = null;
  private held: number[] = [];

  expect(input: Uint8Array): void {
    const command = input[input.length - 1] === 10 ? input.slice(0, -1) : input;
    this.expected = new Uint8Array(command.length);
    this.expected.set(command);
    this.held = [];
  }

  get active(): boolean {
    return this.expected !== null;
  }

  filter(chunk: Uint8Array): Uint8Array {
    const expected = this.expected;
    if (!expected) return chunk;
    const out: number[] = [];
    for (const byte of chunk) {
      if (this.expected === null) {
        out.push(byte);
        continue;
      }
      const index = this.held.length;
      if (index < expected.length && expected[index] === byte) {
        this.held.push(byte);
      } else if (index === expected.length && byte === 13) {
        this.held.push(byte);
      } else if (
        (index === expected.length && byte === 10) ||
        (index === expected.length + 1 && this.held[index - 1] === 13 && byte === 10)
      ) {
        this.expected = null;
        this.held = [];
      } else {
        out.push(...this.held, byte);
        this.expected = null;
        this.held = [];
      }
    }
    return Uint8Array.from(out);
  }
}

export function normalizeTerminalDirectory(cwd: string, target: string): string {
  const absolute = /^(?:\/|[A-Za-z]:[\\/])/.test(target)
    ? target
    : `${cwd.replace(/[\\/]+$/, "")}/${target}`;
  if (/^[A-Za-z]:[\\/]/.test(absolute)) return absolute;
  const parts: string[] = [];
  for (const part of absolute.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  return `/${parts.join("/")}`;
}

/** Build one safe, terminal-local cd submission. Rust has already parsed the
 * manually typed shell word; this layer never evaluates it as shell syntax. */
export function terminalDirectoryPlan(
  cwd: string,
  path: string,
  selected: boolean,
  shellKind: string,
  literalPayload: (path: string, shell: string) => Uint8Array,
): TerminalDirectoryPlan {
  const symbolicHome = path === "~" || (!selected && path === "");
  const oldPwd = !selected && path === "-";
  const homeChild = !selected && /^~[\\/]/.test(path);
  if (symbolicHome || oldPwd || homeChild) {
    const command = symbolicHome
      ? "cd"
      : oldPwd
        ? "cd -"
        : `cd -- "$HOME"/'${path.slice(2).replace(/'/g, `'\\''`)}'`;
    return { payload: new TextEncoder().encode(`${command}\n`), optimisticCwd: null };
  }
  const destination = normalizeTerminalDirectory(cwd, path);
  return {
    payload: literalPayload(destination, shellKind),
    optimisticCwd: destination,
  };
}