import { describe, expect, test } from "bun:test";
import { Effect } from "effect";
import { TerminalSuggestions, suggestionPrefix } from "../src/terminal/suggestions";
import { fileURLToPath } from "node:url";
import type { DaemonConnection } from "../src/daemon/client";

function fixture(options: { history?: string[]; entries?: { name: string; is_dir: boolean }[]; fail?: boolean } = {}) {
  const requests: unknown[] = [];
  const client = { request: (_family: unknown, message: unknown) => {
    requests.push(message);
    if (options.fail) return Effect.fail(new Error("unavailable"));
    if (message === "ListBrowserLocations") return Effect.succeed({ BrowserLocations: { locations: [{ kind: "home", path: "/home/test" }] } });
    if (message && typeof message === "object" && "ReadShellHistory" in message) return Effect.succeed({ ShellHistory: { entries: options.history || [] } });
    return Effect.succeed({ DirListing: { entries: options.entries || [{ name: "my file.txt", is_dir: false }, { name: "my folder", is_dir: true }] } });
  } } as unknown as Pick<DaemonConnection, "request">;
  return { model: new TerminalSuggestions(client), requests, options };
}

describe("terminal suggestions", () => {
  test("history is cached, newest-first, unique, and matches subsequences", async () => {
    const { model, requests } = fixture({ history: ["git status", "cargo check", "git status"] });
    await Promise.all([model.loadHistory(), model.loadHistory()]);
    await model.loadHistory();
    expect(requests).toHaveLength(1);
    expect(model.history("", ["git status", "git log"]).map(row => row.value)).toEqual(["git log", "git status", "cargo check"]);
    expect(model.history("gts", []).map(row => row.value)).toEqual(["git status"]);
    expect(model.ghost("git", ["git log"])).toBe("git log");
    expect(model.ghost("", [])).toBeUndefined();
    expect(model.ghost("gts", [])).toBeUndefined();
  });
  test("failed history load rejects and can retry", async () => {
    const { model, options, requests } = fixture({ fail: true });
    await expect(model.loadHistory()).rejects.toThrow();
    options.fail = false;
    await model.loadHistory();
    expect(requests).toHaveLength(2);
  });
  test.each([
    ["cat my", "cat my\\ file.txt ", "cat my\\ folder/"],
    ['cat "my', 'cat "my file.txt" ', 'cat "my folder/"'],
    ["cat 'my'", "cat 'my file.txt' ", "cat 'my folder/'"],
    ["cat ./my\\ f", "cat ./my\\ file.txt ", "cat ./my\\ folder/"],
  ])("replaces only the last token safely: %s", async (current, file, dir) => {
    const { model, requests } = fixture();
    expect((await model.complete(current, "/actual/shell", [])).map(row => row.value)).toEqual([file, dir]);
    expect(requests).toEqual([{ BrowserListDir: { path: "/actual/shell" } }]);
  });
  test("absolute and relative parent paths use daemon paths", async () => {
    const { model, requests } = fixture();
    await model.complete("cat ../my", "/actual/shell", []);
    await model.complete("cat /other/my", "/actual/shell", []);
    expect(requests).toEqual([{ BrowserListDir: { path: "/actual" } }, { BrowserListDir: { path: "/other" } }]);
  });
  test("tilde uses authorized home, quoted tilde stays literal", async () => {
    const { model, requests } = fixture();
    expect((await model.complete("cat ~/my", "/actual", []))[0]?.value).toBe("cat ~/my\\ file.txt ");
    await model.complete("cat '~/my'", "/actual", []);
    expect(requests).toEqual(["ListBrowserLocations", { BrowserListDir: { path: "/home/test" } }, { BrowserListDir: { path: "/actual/~" } }]);
  });
  test("filenames cannot inject shell syntax, including within quotes", async () => {
    const { model } = fixture({ entries: [{ name: "a'$`;.txt", is_dir: false }] });
    expect((await model.complete("cat a", "/actual", []))[0]?.value).toBe("cat a\\'\\$\\`\\;.txt ");
    expect((await model.complete('cat "a', "/actual", []))[0]?.value).toBe('cat "a\'\\$\\`;.txt" ');
    expect((await model.complete("cat 'a", "/actual", []))[0]?.value).toBe("cat 'a'\\''$`;.txt' ");
  });
  test.each(["cat $HOME/a", "cat `pwd`/a", "cat x; ls", "cat x | ls", "cat *.ts", "cat > out", "cat foo\\", "X=1 cat a"])("unsupported context is history-only: %s", async current => {
    const { model, requests } = fixture();
    expect(await model.complete(current, "/actual", [current + "bc"])).toEqual(model.history(current, [current + "bc"]));
    expect(requests).toHaveLength(0);
  });
  test("commands are local, errors preserve history, and results are bounded", async () => {
    const { model, requests } = fixture({ fail: true });
    expect((await model.complete("car", "/actual", [])).some(row => row.value === "cargo ")).toBe(true);
    expect(requests).toHaveLength(0);
    expect(await model.complete("cat a", "/actual", ["cat abc"])).toEqual(model.history("cat a", ["cat abc"]));
    expect(model.history("", Array.from({ length: 200 }, (_, i) => `echo ${i}`))).toHaveLength(100);
  });
  test("history cannot crowd commands and verified paths out of Tab completions", async () => {
    const { model } = fixture();
    const recent = Array.from({ length: 100 }, (_, i) => `cat previous-${i}`);
    const commands = await model.complete("ca", "/actual", recent);
    expect(commands).toHaveLength(100);
    expect(commands[0]?.kind).toBe("command");
    expect(commands.some(row => row.value === "cargo ")).toBe(true);
    const paths = await model.complete("cat ", "/actual", recent);
    expect(paths).toHaveLength(100);
    expect(paths.slice(0, 2).map(row => row.kind)).toEqual(["file", "directory"]);
    expect(paths[2]?.kind).toBe("history");
  });
  test("session completion uses host candidates, UTF-8 cursor offsets and rich kinds before history", async () => {
    const requests: unknown[] = [], line = "gc café";
    const client = { request: (family: string, message: unknown) => {
      requests.push({ family, message });
      return Effect.succeed({ TerminalCompletions: { session_id: "shell-a", line, cursor: new TextEncoder().encode(line).length, provider: "bash",
        entries: [{ value: "gc café-feature ", label: "café-feature", detail: "Local branch", kind: "branch" }] } });
    } } as unknown as Pick<DaemonConnection, "request">;
    const model = new TerminalSuggestions(client, "shell-a");
    expect(await model.complete(line, "/not-the-host-directory", ["gc café-old"])).toEqual([
      { value: "gc café-feature ", label: "café-feature", detail: "Local branch", kind: "branch" },
      { value: "gc café-old", label: "gc café-old", detail: "History", kind: "history" },
    ]);
    expect(requests).toEqual([{ family: "Pty", message: { CompleteTerminal: { session_id: "shell-a", line, cursor: 8 } } }]);
    expect(suggestionPrefix(line, { value: "gc café-feature ", label: "café-feature", detail: "", kind: "branch" })).toBe("café");
    expect(suggestionPrefix("echo $PA", { value: "echo $PATH", label: "$PATH", detail: "", kind: "variable" })).toBe("$PA");
  });
  test("native empty results are respected and malformed candidates never reach the menu", async () => {
    let entries: unknown[] = [];
    const client = { request: () => Effect.succeed({ TerminalCompletions: { session_id: "shell-a", line: "ca", cursor: 2, provider: "bash", entries } }) } as unknown as Pick<DaemonConnection, "request">;
    const model = new TerminalSuggestions(client, "shell-a");
    expect(await model.complete("ca", "/actual", [])).toEqual([]);
    entries = [
      { value: "cat\nrm x", label: "bad", kind: "command" }, { value: "cat ", label: "bad\x1b[31m", kind: "command" },
      { value: "x".repeat(9000), label: "large", kind: "command" },
      { value: "cargo ", label: "cargo", detail: "Build\nRust", kind: "command" },
      { value: "cargo ", label: "duplicate", kind: "command" },
    ];
    expect(await model.complete("ca", "/actual", [])).toEqual([{ value: "cargo ", label: "cargo", detail: "Build Rust", kind: "command" }]);
  });
  test("mismatched native replies fall back without accepting another session's data", async () => {
    const requests: string[] = [];
    const client = { request: (family: string) => {
      requests.push(family);
      return Effect.succeed(family === "Pty"
        ? { TerminalCompletions: { session_id: "another-shell", line: "cat f", cursor: 5, entries: [{ value: "wrong ", label: "wrong", kind: "command" }] } }
        : { DirListing: { path: "/actual", entries: [{ name: "file.txt", is_dir: false }] } });
    } } as unknown as Pick<DaemonConnection, "request">;
    const model = new TerminalSuggestions(client, "shell-a");
    expect((await model.complete("cat f", "/actual", [])).map(row => row.value)).toEqual(["cat file.txt "]);
    expect(requests).toEqual(["Pty", "Files"]);
  });
  test("small command hints supplement a host without installed completion plugins without duplicate rows", async () => {
    let entries: unknown[] = [];
    const client = { request: () => Effect.succeed({ TerminalCompletions: { session_id: "shell-a", line: "git status --br", cursor: 15, provider: "bash", entries } }) } as unknown as Pick<DaemonConnection, "request">;
    const model = new TerminalSuggestions(client, "shell-a");
    expect((await model.complete("git status --br", "/actual", []))[0]?.value).toBe("git status --branch ");
    entries = [{ value: "git status --branch", label: "--branch", detail: "Host flag", kind: "option" }];
    expect(await model.complete("git status --br", "/actual", [])).toEqual([{ value: "git status --branch", label: "--branch", detail: "Host flag", kind: "option" }]);
  });
  test("a new query cancels the prior native request without starting stale fallback work", async () => {
    const pending = new Map<string, (reply: unknown) => void>(), requests: string[] = [];
    let cleaned = 0;
    const client = { request: (family: string, message: { CompleteTerminal: { line: string } }) => {
      requests.push(family); const line = message.CompleteTerminal.line;
      return Effect.callback<unknown, never>(resume => {
        pending.set(line, reply => resume(Effect.succeed(reply)));
        return Effect.sync(() => { cleaned++; });
      });
    } } as unknown as Pick<DaemonConnection, "request">;
    const model = new TerminalSuggestions(client, "shell-a");
    const first = model.complete("gc old", "/actual", []), second = model.complete("gc new", "/actual", []);
    pending.get("gc new")!({ TerminalCompletions: { session_id: "shell-a", line: "gc new", cursor: 6, provider: "bash", entries: [{ value: "gc new-branch ", label: "new-branch", kind: "branch" }] } });
    expect(await first).toEqual([]);
    expect((await second)[0]?.value).toBe("gc new-branch ");
    expect(requests).toEqual(["Pty", "Pty"]); expect(cleaned).toBe(1);
  });
  test("common subcommands and flags are suggested without executing or browsing flags", async () => {
    const { model, requests } = fixture();
    expect((await model.complete("git st", "/actual", [])).map(row => row.value)).toContain("git status ");
    requests.length = 0;
    expect(await model.complete("git status --sh", "/actual", [])).toEqual([{ value: "git status --short ", label: "--short", detail: "Compact status", kind: "option" }]);
    expect(requests).toHaveLength(0);
    expect((await model.complete("cd my", "/actual", [])).map(row => row.value)).toEqual(["cd my\\ folder/"]);
  });
  test("literal tilde filenames, hidden entries and malformed daemon names", async () => {
    const { model } = fixture({ entries: [
      { name: "~someone", is_dir: false }, { name: ".hidden", is_dir: false },
      { name: "../escape", is_dir: true }, { name: "bad\nname", is_dir: false },
    ] });
    expect((await model.complete("cat ", "/actual", [])).map(row => row.value)).toEqual(["cat \\~someone "]);
    expect((await model.complete("cat .", "/actual", [])).map(row => row.value)).toEqual(["cat .hidden "]);
  });
  test("concurrent listings share requests and stop at four distinct directories", async () => {
    const requests: unknown[] = [];
    let finish!: (reply: unknown) => void;
    const pending = new Promise(resolve => { finish = resolve; });
    const client = { request: (_family: unknown, message: unknown) => {
      requests.push(message);
      return Effect.promise(() => pending);
    } } as unknown as Pick<DaemonConnection, "request">;
    const model = new TerminalSuggestions(client);
    const calls = ["/a", "/a", "/b", "/c", "/d", "/e"].map(cwd => model.complete("cat x", cwd, []));
    expect(requests).toHaveLength(4);
    finish({ DirListing: { entries: [] } });
    await Promise.all(calls);
  });
  test("candidate kinds and highlighted prefixes use unescaped display text", async () => {
    const { model } = fixture();
    const paths = await model.complete('cat "./my f', "/actual", []);
    expect(paths.map(item => item.kind)).toEqual(["file", "directory"]);
    expect(paths.map(item => suggestionPrefix('cat "./my f', item))).toEqual(["my f", "my f"]);
    const command = (await model.complete("ca", "/actual", [])).find(item => item.label === "cargo")!;
    expect(command.kind).toBe("command"); expect(suggestionPrefix("ca", command)).toBe("ca");
    const history = model.history("gts", ["git status"])[0]!;
    expect(history.kind).toBe("history"); expect(suggestionPrefix("gts", history)).toBe("");
  });
  test("production completion input previews, cycles a stable base, restores and submits once", async () => {
    const child = Bun.spawn([process.execPath, "run", "scripts/completion-native-check.tsx"], {
      cwd: fileURLToPath(new URL("..", import.meta.url)), stdout: "pipe", stderr: "pipe",
    });
    const [status, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
    expect({ status, stderr }).toEqual({ status: 0, stderr: "" });
    expect(stdout).toContain("completion input checks passed");
  }, 10000);
});