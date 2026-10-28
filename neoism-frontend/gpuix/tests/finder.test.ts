import { afterEach, expect, test } from "bun:test";
import { FinderController, type FinderOptions } from "../src/finder/FinderController";
import { variant, type SearchClientMessage } from "../src/daemon/protocol";
const finders: FinderController[] = [];
afterEach(() => finders.splice(0).forEach((finder) => finder.dispose()));
const until = async (test: () => boolean) => { const end = Date.now() + 3000; while (!test()) { if (Date.now() > end) throw new Error("Finder did not converge"); await Bun.sleep(5); } };
function fixture(options: Partial<FinderOptions> = {}) {
  const requests: Array<{ tag: string; body: Record<string, unknown> }> = [];
  const finder = new FinderController({ cwd: "/project", width: 1200, height: 800,
    search: async (message) => {
      const [tag, payload] = variant(message)!; const body = payload as Record<string, unknown>; requests.push({ tag, body });
      if (tag === "CollectFiles") return { CollectFilesResult: { req_id: 9000, paths: ["src/main.rs", "README.md"] } };
      if (tag === "SearchFiles") return { SearchFilesResult: { req_id: 9001, hits: Array.from({ length: 40 }, (_, n) => ({ score: 100 - n, path: `src/${body.query || "file"}${n}.rs` })) } };
      if (tag === "SearchGrep") return { SearchGrepResult: { req_id: 9002, hits: [{ score: 100, path: "src/main.rs", line: 12, column: 3, text: "let value = needle;" }] } };
      if (tag === "SearchGitChanges") return { SearchGitChangesResult: { req_id: 9003, hits: [{ path: "src/main.rs", status: "Modified", line: 12, text: "changed" }] } };
      throw new Error(`Unexpected ${tag}`);
    }, ...options });
  finders.push(finder); return { finder, requests };
}
test("file Finder uses the canonical compact geometry, scoped search, fuzzy/exact modes, and scroll springs", async () => {
  const { finder, requests } = fixture();
  await until(() => finder.frame.rows.length === 40 && !finder.frame.animating);
  expect(finder.frame.rect[2]).toBeCloseTo(480); expect(finder.frame.preview).toBe(false);
  expect(requests.every((r) => r.body.cwd === "")).toBe(true);
  finder.move(20); expect(finder.frame.selected).toBe(20); expect(finder.frame.top).toBeGreaterThan(0); expect(finder.frame.animating).toBe(true);
  finder.query("main"); await until(() => finder.frame.rows[0]?.path === "src/main0.rs");
  finder.cycle(); await until(() => requests.some((r) => r.tag === "SearchFiles" && r.body.mode === "Exact"));
  expect(finder.selectedTarget()?.path).toBe("src/main0.rs"); expect(finder.frame.rect[2]).toBeLessThanOrEqual(480 * 1.04);
});
test("grep keeps the native minimum-query policy, regex mode, expanded geometry, and highlighted remote preview", async () => {
  const previews: string[] = [];
  const { finder, requests } = fixture({ mode: "grep", preview: async (path) => { previews.push(path); return Array.from({ length: 100 }, (_, n) => `let line${n + 1} = 1;`).join("\n"); } });
  finder.query("x"); await Bun.sleep(260); expect(requests.filter((r) => r.tag === "SearchGrep")).toHaveLength(0);
  finder.cycle(); finder.cycle(); await until(() => finder.frame.rows.length === 1 && finder.frame.previewLoaded && !finder.frame.animating);
  expect(requests.find((r) => r.tag === "SearchGrep")?.body.mode).toBe("Regex");
  expect(finder.frame.rect[2]).toBeCloseTo(880); expect(previews).toEqual(["src/main.rs"]);
  expect(finder.frame.previewRows.some((r) => r.line === 12 && r.selected)).toBe(true);
  expect(finder.frame.previewRows.some((r) => r.tokens.some((t) => t.token === "Keyword"))).toBe(true);
  expect(finder.selectedTarget()).toMatchObject({ path: "src/main.rs", line: 12, mode: "grep", query: "x" });
});
test("buffer lines and replacements run the existing in-memory matcher and cap rows without a daemon search", () => {
  const { finder, requests } = fixture({ mode: "buffer", lines: Array.from({ length: 300 }, (_, n) => `needle ${n}`) });
  finder.query("needle"); expect(finder.frame.rows).toHaveLength(200); expect(finder.frame.matchTotal).toBe(300);
  finder.move(12); expect(finder.selectedTarget()).toMatchObject({ path: undefined, line: 13, mode: "buffer" });
  expect(requests).toHaveLength(0);
  const replace = fixture({ mode: "replace", lines: ["one needle", "two", "another needle"] }).finder;
  replace.query("needle/replaced"); expect(replace.frame.rows.map((r) => r.line)).toEqual([1, 3]);
});
test("cancelled stale search replies cannot replace the current query", async () => {
  const delayed: Array<{ query: string; reply: (message: unknown) => void; signal: AbortSignal }> = [];
  const search: FinderOptions["search"] = (message, signal) => {
    const [tag, body] = variant(message)!;
    if (tag === "CollectFiles") return Promise.resolve({ CollectFilesResult: { req_id: 0, paths: [] } });
    return new Promise((reply) => delayed.push({ query: (body as { query: string }).query, reply, signal }));
  };
  const { finder } = fixture({ search });
  await until(() => delayed.some((r) => r.query === ""));
  const old = delayed.find((r) => r.query === "")!;
  finder.query("new"); expect(old.signal.aborted).toBe(true);
  await until(() => delayed.some((r) => r.query === "new"));
  delayed.find((r) => r.query === "new")!.reply({ SearchFilesResult: { req_id: 100, hits: [{ path: "new.rs", score: 10 }] } });
  await until(() => finder.frame.rows[0]?.path === "new.rs");
  old.reply({ SearchFilesResult: { req_id: 200, hits: [{ path: "stale.rs", score: 20 }] } });
  await Bun.sleep(10); expect(finder.frame.rows[0].path).toBe("new.rs");
});
test("a late preview cannot overwrite the file selected after it", async () => {
  const previews = new Map<string, { reply: (text: string) => void; signal: AbortSignal }>();
  const { finder } = fixture({ mode: "grep", preview: (path, signal) => new Promise((reply) => previews.set(path, { reply, signal })),
    search: async () => ({ SearchGrepResult: { req_id: 0, hits: [
      { score: 10, path: "old.rs", line: 1, column: 0, text: "old" },
      { score: 9, path: "new.rs", line: 1, column: 0, text: "new" },
    ] } }) });
  finder.query("match"); await until(() => previews.has("old.rs"));
  finder.select(1); await until(() => previews.has("new.rs"));
  expect(previews.get("old.rs")!.signal.aborted).toBe(true);
  previews.get("new.rs")!.reply("fn latest() {}");
  await until(() => finder.frame.previewLoaded && finder.frame.previewRows.some((r) => r.text.includes("latest")));
  previews.get("old.rs")!.reply("fn stale() {}");
  await Bun.sleep(10); expect(finder.frame.previewPath).toBe("new.rs");
  expect(finder.frame.previewRows.some((r) => r.text.includes("stale"))).toBe(false);
});
test("Git changes and references preserve daemon-owned paths, line targets, filtering, and statuses", async () => {
  const git = fixture({ mode: "git" }).finder; await until(() => git.frame.rows.length === 1);
  expect(git.frame.rows[0].marker).toBe("M"); expect(git.selectedTarget()).toMatchObject({ path: "src/main.rs", line: 12, mode: "git" });
  const refs = fixture({ mode: "references", references: [{ path: "src/a.rs", line: 10, column: 4, text: "find_me()" }, { path: "src/b.rs", line: 20, column: 8, text: "another()" }] }).finder;
  refs.query("find_me"); expect(refs.frame.rows).toHaveLength(1); expect(refs.selectedTarget()).toMatchObject({ path: "src/a.rs", line: 10, column: 4, mode: "references" });
});
test("search failures stay visible through animation ticks and a new query clears the error", async () => {
  const { finder } = fixture({ search: async () => { throw new Error("Refine the query or retry"); } });
  await until(() => !!finder.frame.error);
  await Bun.sleep(100); expect(finder.frame.error).toBe("Refine the query or retry");
  finder.query("retry"); expect(finder.frame.error).toBeUndefined();
});
