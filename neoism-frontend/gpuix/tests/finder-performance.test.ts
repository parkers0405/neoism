import { afterEach, expect, test } from "bun:test";
import { FinderController, type FinderFrameUpdate, type FinderOptions } from "../src/finder/FinderController";
import { coreCall } from "../src/native-core";
import { variant } from "../src/daemon/protocol";

// These integration tests require the native library rebuilt from this source.
const finders: FinderController[] = [];
afterEach(() => finders.splice(0).forEach(finder => finder.dispose()));
const until = async (predicate: () => boolean) => {
  const deadline = performance.now() + 4000;
  while (!predicate()) {
    if (performance.now() > deadline) throw new Error("Finder did not settle");
    await Bun.sleep(5);
  }
};
const symbols = Array.from({ length: 500 }, (_, n) => ({ kind: "Function", name: `symbol_${n}`, line: n + 1, column: 0 }));
function fixture(options: Partial<FinderOptions> = {}) {
  const finder = new FinderController({ cwd: "/project", mode: "symbols", symbols, width: 1200, height: 800,
    search: async () => { throw new Error("Unexpected daemon search"); }, ...options });
  finders.push(finder);
  return finder;
}

test("500 rows are retained by identity across motion and omitted from native updates", async () => {
  const finder = fixture(), rows = finder.frame.rows;
  expect(rows).toHaveLength(500);
  for (const operation of [() => finder.move(20), () => {finder.select(30);expect(finder.selectedTarget()?.line).toBe(31);}, () => finder.scroll(100), () => finder.layout(1100, 750)]) {
    operation(); expect(finder.frame.rows).toBe(rows);
  }
  const compact = coreCall<FinderFrameUpdate>("finder.frame", { id: finder.id });
  expect(Object.hasOwn(compact, "rows")).toBe(false);
  expect(compact.inputCaret?.kind).toBe("rect");
  expect(compact.commands).not.toContainEqual(compact.inputCaret);
  expect(compact.nextTickMs).toBeNull();
  await until(() => !finder.frame.animating);
  expect(finder.frame.rows).toBe(rows);
  // Wheel scrolling clamps selection to its visible rows; compact targets must
  // follow that selection without replacing the retained results array.
  expect(finder.frame.selected).toBeGreaterThanOrEqual(finder.frame.top);
  expect(finder.frame.selected).toBeLessThan(finder.frame.top+finder.frame.visibleRows);
  expect(finder.selectedTarget()?.line).toBe(rows[finder.frame.selected]!.line);
  finder.symbols([], true);
  expect(finder.frame.rows).toEqual([]);
  expect(finder.frame.rows).not.toBe(rows);
});

test("settled Finder does not publish full frames for idle caret ticks", async () => {
  const finder = fixture();
  await until(() => !finder.frame.animating);
  let frames = 0;
  const unsubscribe = finder.subscribe(() => frames++);
  const commands = finder.frame.commands;
  await Bun.sleep(1150);
  expect(frames).toBe(0);
  expect(finder.frame.commands).toBe(commands);
  expect(finder.frame.inputCaret).toBeTruthy();
  unsubscribe();
});

test("compact scheduling still runs debounced searches and installs changed results", async () => {
  const queries: string[] = [];
  const finder = fixture({ mode: "files", search: async message => {
    const [tag, body] = variant(message)!;
    if (tag === "CollectFiles") return { CollectFilesResult: { paths: [] } };
    if (tag !== "SearchFiles") throw new Error(`Unexpected ${tag}`);
    const query = (body as { query: string }).query;
    queries.push(query);
    return { SearchFilesResult: { hits: Array.from({ length: 500 }, (_, n) => ({ path: `${query || "file"}_${n}.rs`, score: 500 - n })) } };
  } });
  await until(() => finder.frame.rows.length === 500 && !finder.frame.loading && !finder.frame.animating);
  const old = finder.frame.rows;
  finder.query("new");
  expect(finder.frame.rows).toBe(old);
  await until(() => finder.frame.rows[0]?.path === "new_0.rs" && !finder.frame.loading);
  expect(queries).toContain("new");
  expect(finder.frame.rows).not.toBe(old);
  expect(finder.frame.rows).toHaveLength(500);
  const changed = finder.frame.rows;
  finder.move(4);
  expect(finder.frame.rows).toBe(changed);
  expect(finder.selectedTarget()?.path).toBe("new_4.rs");
});