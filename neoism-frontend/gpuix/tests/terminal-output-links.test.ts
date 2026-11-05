import { describe, expect, test } from "bun:test";
import { Effect } from "effect";
import type { DaemonConnection } from "../src/daemon/client";
import type { CommandBlock } from "../src/terminal/blocks";
import type { CellRun } from "../src/terminal/buffer";
import { TerminalOutputLinks } from "../src/terminal/output-links";

const run = (text: string, column = 0): CellRun => ({ text, column, width: Bun.stringWidth(text), fg: "#fff", bold: false, italic: false, underline: false });
const block = (command: string, rows: (string | CellRun[])[], id = 1, directory = "/remote/work"): CommandBlock => ({ id, started: id, command, directory, duration: 1, status: 0, truncated: false, lines: rows.map(row => typeof row === "string" ? [run(row)] : row) });

/** An in-memory daemon filesystem: responses are computed from the requested
 * remote directory, not precomputed link results. No host filesystem access. */
function filesystem() {
  const directories = new Map<string, { name: string; is_dir: boolean }[]>([
    ["/remote/work", [{ name: "src", is_dir: true }, { name: "README.md", is_dir: false }, { name: "my file.txt", is_dir: false }, { name: "界面", is_dir: true }, { name: "café", is_dir: false }]],
    ["/remote/work/my folder", [{ name: "remote-only.txt", is_dir: false }]],
    ["/remote/other", [{ name: "other.txt", is_dir: false }]],
  ]);
  const requests: unknown[] = [];
  let locations: unknown = { BrowserLocations: { locations: [{ kind: "home", path: "/remote/home" }] } };
  let response: unknown;
  let fail = false;
  const client = { request: (family: string, message: unknown) => {
    requests.push(message);
    if (family !== "Files") return Effect.fail(new Error("Unexpected service"));
    if (fail) return Effect.fail(new Error("Disconnected"));
    if (message === "ListBrowserLocations") return Effect.succeed(locations);
    if (response !== undefined) return Effect.succeed(response);
    const directory = (message as { BrowserListDir?: { path: string } }).BrowserListDir?.path;
    if (!directory || !directories.has(directory)) return Effect.fail(new Error("Unauthorized or missing directory"));
    return Effect.succeed({ DirListing: { path: directory, entries: directories.get(directory)!.map(entry => ({ ...entry })) } });
  } } as unknown as Pick<DaemonConnection, "request">;
  return { links: new TerminalOutputLinks(client), directories, requests, setResponse: (value: unknown) => { response = value; }, setLocations: (value: unknown) => { locations = value; }, setFail: (value: boolean) => { fail = value; } };
}

describe("terminal verified output links", () => {
  test("column ls maps styled split runs and wide/combining cells to native spans", async () => {
    const { links } = filesystem();
    const result = await links.resolve(block("ls", [[run("sr"), run("c", 2), run("  界面  README.md", 3)], "my file.txt", "café"]));
    expect(result).toEqual([
      { row: 0, column: 0, endColumn: 3, path: "/remote/work/src", isDirectory: true },
      { row: 0, column: 5, endColumn: 9, path: "/remote/work/界面", isDirectory: true },
      { row: 0, column: 11, endColumn: 20, path: "/remote/work/README.md", isDirectory: false },
      { row: 1, column: 0, endColumn: 11, path: "/remote/work/my file.txt", isDirectory: false },
      { row: 2, column: 0, endColumn: 4, path: "/remote/work/café", isDirectory: false },
    ]);
  });
  test("ls -la metadata and file names with spaces, without linking totals or symlink targets", async () => {
    const { links } = filesystem();
    const prefix = "-rw-r--r-- 1 user group 123 Oct 10 12:30 ";
    const result = await links.resolve(block("ls -la", ["total 20", prefix + "my file.txt", "lrwxrwxrwx 1 u g 3 Oct 10 12:30 src -> README.md"]));
    expect(result).toEqual([{ row: 1, column: prefix.length, endColumn: prefix.length + 11, path: "/remote/work/my file.txt", isDirectory: false }]);
  });
  test.each(["ls -- 'my folder'", 'ls "my folder"', "ls   --   'my folder'"])("literal directory operand: %s", async command => {
    const { links } = filesystem();
    expect((await links.resolve(block(command, ["remote-only.txt"]))).map(link => link.path)).toEqual(["/remote/work/my folder/remote-only.txt"]);
  });
  test("absolute and parent-relative operands are remote daemon paths", async () => {
    const { links } = filesystem();
    expect((await links.resolve(block("ls ../other", ["other.txt"]))).map(link => link.path)).toEqual(["/remote/other/other.txt"]);
    expect((await links.resolve(block("ls /remote/other", ["other.txt"], 2))).map(link => link.path)).toEqual(["/remote/other/other.txt"]);
  });
  test.each(["cat log", "echo src", "ls; pwd", "ls | cat", "ls $(pwd)", "ls *.txt", "ls -R", "ls -d src", "ls a b", "ls 'my folder", "ls --color=always", "ls > file", "sudo ls"])("rejects unsupported syntax without browsing: %s", async command => {
    const { links, requests } = filesystem();
    expect(await links.resolve(block(command, ["src"]))).toEqual([]);
    expect(requests).toEqual([]);
  });
  test("requires complete verified rows and rejects ambiguous partitions", async () => {
    const { links, directories } = filesystem();
    directories.get("/remote/work")!.push({ name: "a", is_dir: false }, { name: "b", is_dir: false }, { name: "a  b", is_dir: false });
    expect(await links.resolve(block("ls", ["error: src failed", "missing.txt", "a  b", "src  missing.txt"]))).toEqual([]);
  });
  test("new commands refresh removed files, same command dedupes and cwd isolates", async () => {
    const { links, directories, requests } = filesystem();
    const first = block("ls", ["README.md"]);
    expect((await Promise.all([links.resolve(first), links.resolve({ ...first })]))[0]).toHaveLength(1);
    expect(requests).toHaveLength(1);
    directories.set("/remote/work", []);
    expect(await links.resolve(block("ls", ["README.md"], 2))).toEqual([]);
    expect((await links.resolve(block("ls", ["other.txt"], 3, "/remote/other")))[0]?.path).toBe("/remote/other/other.txt");
    expect(requests).toHaveLength(3);
  });
  test.each([{}, { DirListing: { entries: "src" } }, { DirListing: { entries: [{ name: "../escape", is_dir: false }] } }, { DirListing: { entries: [{ name: "src", is_dir: "yes" }] } }, { DirListing: { entries: [{ name: "src", is_dir: true }, { name: "src", is_dir: false }] } }])("malformed daemon data fails closed: %j", async response => {
    const { links, setResponse } = filesystem();
    // Preserve valid envelope metadata while varying the actual entries.
    setResponse("DirListing" in response ? { DirListing: { path: "/remote/work", ...response.DirListing } } : response);
    expect(await links.resolve(block("ls", ["src"]))).toEqual([]);
  });
  test("no links for running/failed/truncated output or inconsistent native widths", async () => {
    const { links } = filesystem();
    for (const patch of [{ running: true }, { status: 1 }, { truncated: true }, { background: true }]) expect(await links.resolve({ ...block("ls", ["src"]), ...patch })).toEqual([]);
    expect(await links.resolve(block("ls", [[{ ...run("界面"), width: 2 }]]))).toEqual([]);
  });
  test("home is an authorized descriptor, cached, not cwd or environment", async () => {
    const { links, requests } = filesystem();
    expect(await links.homeDirectory()).toBe("/remote/home");
    expect(await links.homeDirectory()).toBe("/remote/home");
    expect(requests).toHaveLength(1);
  });
  test("home failures retry; invalid or ambiguous descriptors never become home", async () => {
    const fixture = filesystem(); fixture.setFail(true);
    expect(await fixture.links.homeDirectory()).toBeUndefined();
    fixture.setFail(false);
    expect(await fixture.links.homeDirectory()).toBe("/remote/home");
    for (const path of ["relative", "/remote/\nunsafe", "//host/share"]) {
      const next = filesystem(); next.setLocations({ BrowserLocations: { locations: [{ kind: "home", path }] } });
      expect(await next.links.homeDirectory()).toBeUndefined();
    }
  });
  test("authorization errors do not create links and new commands can recover", async () => {
    const fixture = filesystem(); fixture.setFail(true);
    expect(await fixture.links.resolve(block("ls", ["src"]))).toEqual([]);
    fixture.setFail(false);
    expect((await fixture.links.resolve(block("ls", ["src"], 2)))[0]?.path).toBe("/remote/work/src");
  });
  test("quoted ls names resolve, but literal quote names cannot cause ambiguous links", async () => {
    const { links, directories } = filesystem();
    const result = await links.resolve(block("ls", ["'my file.txt'  src"]));
    expect(result.map(link => [link.column, link.endColumn, link.path])).toEqual([[0, 13, "/remote/work/my file.txt"], [15, 18, "/remote/work/src"]]);
    directories.get("/remote/work")!.push({ name: "'my file.txt'", is_dir: false });
    expect(await links.resolve(block("ls", ["'my file.txt'"], 2))).toEqual([]);
  });
  test("native gaps and classification slashes preserve cell coordinates", async () => {
    const { links } = filesystem();
    expect(await links.resolve(block("ls -F", [[run("src/", 0), run("README.md", 9)]]))).toEqual([
      { row: 0, column: 0, endColumn: 4, path: "/remote/work/src", isDirectory: true },
      { row: 0, column: 9, endColumn: 18, path: "/remote/work/README.md", isDirectory: false },
    ]);
  });
  test("out-of-order daemon replies are isolated; concurrency is bounded", async () => {
    const pending: Array<(entries: { name: string; is_dir: boolean }[]) => void> = [];
    const client = { request: () => Effect.promise(() => new Promise(resolve => { pending.push(entries => resolve({ DirListing: { path: "/remote/work", entries } })); })) } as unknown as Pick<DaemonConnection, "request">;
    const links = new TerminalOutputLinks(client);
    const oldBlock = block("ls", ["README.md"]), newBlock = block("ls", ["README.md"], 2);
    const old = links.resolve(oldBlock), fresh = links.resolve(newBlock);
    const third = links.resolve(block("ls", ["src"], 3)), fourth = links.resolve(block("ls", ["src"], 4));
    const fifthBlock = block("ls", ["src"], 5);
    let fifthSettled = false;
    const fifth = links.resolve(fifthBlock).then(result => { fifthSettled = true; return result; });
    const fifthDuplicate = links.resolve({ ...fifthBlock });
    await Promise.resolve();
    expect(pending).toHaveLength(4);
    expect(fifthSettled).toBe(false);
    pending[1]!([]);
    expect(await fresh).toEqual([]);
    expect(pending).toHaveLength(5);
    expect(fifthSettled).toBe(false);
    pending[4]!([{ name: "src", is_dir: true }]);
    expect((await fifth)[0]?.path).toBe("/remote/work/src");
    expect((await fifthDuplicate)[0]?.isDirectory).toBe(true);
    pending[0]!([{ name: "README.md", is_dir: false }]);
    expect((await old)[0]?.path).toBe("/remote/work/README.md");
    expect(await links.resolve({ ...newBlock })).toEqual([]);
    pending[2]!([{ name: "src", is_dir: true }]); pending[3]!([]);
    expect((await third)[0]?.isDirectory).toBe(true); expect(await fourth).toEqual([]);
  });
  test("queue retention is bounded, overflow can retry, and queued work is deduplicated", async () => {
    const releases: Array<() => void> = [];
    let active = 0, maximum = 0;
    const client = { request: () => Effect.promise(() => new Promise(resolve => {
      maximum = Math.max(maximum, ++active);
      releases.push(() => { active--; resolve({ DirListing: { path: "/remote/work", entries: [{ name: "src", is_dir: true }] } }); });
    })) } as unknown as Pick<DaemonConnection, "request">;
    const links = new TerminalOutputLinks(client);
    const blocks = Array.from({ length: 128 }, (_, index) => block("ls", ["src"], index + 1));
    const results = blocks.map(value => links.resolve(value));
    // This pending identity is older than the completed-cache limit, but must
    // still dedupe rather than consume a second queue slot or refresh early.
    const duplicate = links.resolve({ ...blocks[4]! });
    const overflow = block("ls", ["src"], 129);
    expect(await links.resolve(overflow)).toEqual([]);
    expect(releases).toHaveLength(4);
    for (let index = 0; index < results.length; index++) {
      releases[index]!();
      expect((await results[index]!)[0]?.path).toBe("/remote/work/src");
    }
    expect((await duplicate)[0]?.isDirectory).toBe(true);
    expect(releases).toHaveLength(128);
    const retry = links.resolve(overflow);
    expect(releases).toHaveLength(129);
    releases[128]!();
    expect((await retry)[0]?.path).toBe("/remote/work/src");
    expect(maximum).toBe(4);
    expect(active).toBe(0);
  });
  test("command cache has a fixed bound rather than retaining every cwd forever", async () => {
    const { links, requests, directories } = filesystem();
    for (let id = 1; id <= 66; id++) expect((await links.resolve(block("ls", ["src"], id)))[0]?.path).toBe("/remote/work/src");
    directories.set("/remote/work", []);
    expect(await links.resolve(block("ls", ["src"], 1))).toEqual([]);
    expect(requests).toHaveLength(67);
  });
  test("a response for another directory cannot verify output", async () => {
    const { links, setResponse } = filesystem();
    setResponse({ DirListing: { path: "/remote/other", entries: [{ name: "src", is_dir: true }] } });
    expect(await links.resolve(block("ls", ["src"]))).toEqual([]);
  });
});