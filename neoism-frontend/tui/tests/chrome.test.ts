import { describe, expect, test } from "bun:test";
import { stringWidth } from "bun";
import { breadcrumbSegments, compactPath, fitText, statusModel, tabLabels, tabWindow } from "../src/chrome";

describe("host-path breadcrumbs", () => {
  test("workspace-relative labels keep navigable full paths", () => {
    expect(breadcrumbSegments("/work/src/main.rs", "/work")).toEqual([
      { label: "src", path: "/work/src", kind: "directory" },
      { label: "main.rs", path: "/work/src/main.rs", kind: "file" },
    ]);
    expect(breadcrumbSegments("src/main.rs", "/work")).toEqual(breadcrumbSegments("/work/src/main.rs", "/work"));
    expect(breadcrumbSegments(undefined, "/work")).toEqual([]);
    expect(breadcrumbSegments("/work", "/work")).toEqual([]);
  });
  test("ancestor boundaries, POSIX backslashes and filesystem roots", () => {
    expect(breadcrumbSegments("/workspace/main.rs", "/work").map(s => s.label)).toEqual(["/", "workspace", "main.rs"]);
    expect(breadcrumbSegments("/work/a\\b.rs", "/work").map(s => s.label)).toEqual(["a\\b.rs"]);
    expect(breadcrumbSegments("/")).toEqual([{ label: "/", path: "/", kind: "root" }]);
    expect(breadcrumbSegments("/work/a.rs", "/").map(s => s.label)).toEqual(["work", "a.rs"]);
  });
  test("Windows drive, UNC and verbatim paths use the daemon's syntax", () => {
    expect(breadcrumbSegments("c:/WORK/src/main.rs", "C:\\work").map(s => s.label)).toEqual(["src", "main.rs"]);
    expect(breadcrumbSegments("src\\main.rs", "C:\\work").at(-1)?.path).toBe("C:\\work\\src\\main.rs");
    for (const root of ["\\\\server\\share\\work", "\\\\?\\C:\\work", "\\\\?\\UNC\\server\\share\\work"]) {
      expect(breadcrumbSegments(`${root}\\src\\a.rs`, root).map(s => s.label)).toEqual(["src", "a.rs"]);
    }
    expect(breadcrumbSegments("D:\\a.rs", "C:\\work").map(s => s.label)).toEqual(["D:\\", "a.rs"]);
  });
});

describe("compact labels", () => {
  test("shortest suffix disambiguation preserves host separators", () => {
    expect(tabLabels(["/work/a/main.rs", "/work/b/main.rs", "/work/lib.rs"])).toEqual(["a/main.rs", "b/main.rs", "lib.rs"]);
    expect(tabLabels(["C:\\a\\x.rs", "C:\\b\\x.rs"])).toEqual(["a\\x.rs", "b\\x.rs"]);
    expect(tabLabels(["/a/x.rs", "/a/x.rs"])).toEqual(["x.rs", "x.rs"]);
    expect(tabLabels(["/a/x.rs", "a/x.rs"])).toEqual(["/a/x.rs", "a/x.rs"]);
    expect(tabLabels([])).toEqual([]);
  });
  test("compaction prefers the file leaf and respects cells/graphemes", () => {
    expect(compactPath("/work/src/main.rs", 30, "/work")).toBe("src/main.rs");
    expect(compactPath("/work/src/main.rs", 8, "/work")).toBe("…main.rs");
    expect(compactPath("C:\\work\\src\\a.rs", 30, "C:\\work")).toBe("src\\a.rs");
    expect(fitText("é👩‍💻界", 4)).toBe("é👩‍💻…");
    expect(fitText("界abc", 4, "end")).toBe("…abc");
    for (const width of [-1, 0, 1, 2, 3, 8, NaN, Infinity]) {
      const fitted = fitText("界👩‍💻élong.rs", width);
      expect(stringWidth(fitted)).toBeLessThanOrEqual(Number.isFinite(width) ? Math.max(0, width) : 0);
    }
  });
});

describe("tab strip window", () => {
  const tabs = Array.from({ length: 7 }, (_, index) => ({ id: String(index), label: `tab${index}`, extra: index }));
  test("fits all, preserves identity and reports actual cell width", () => {
    const result = tabWindow(tabs, "3", 100);
    expect(result.tabs.map(tab => tab.tab)).toEqual(tabs);
    expect(result.tabs[3]?.tab).toBe(tabs[3]);
    expect(result.tabs.filter(tab => tab.active).map(tab => tab.index)).toEqual([3]);
    expect(result.width).toBe(stringWidth(result.tabs.map(tab => tab.label).join(" | ")));
    expect(result.hiddenBefore + result.hiddenAfter).toBe(0);
  });
  test("active stays visible at the beginning, middle and end at every positive width", () => {
    for (const active of ["0", "3", "6"]) for (let width = 1; width <= 55; width++) {
      const result = tabWindow(tabs, active, width);
      expect(result.tabs.some(tab => tab.active)).toBe(true);
      expect(result.width).toBeLessThanOrEqual(width);
      expect(result.width).toBe(stringWidth(result.tabs.map(tab => tab.label).join(" | ")));
      expect(result.hiddenBefore + result.tabs.length + result.hiddenAfter).toBe(tabs.length);
      expect(result.tabs.map(tab => tab.index)).toEqual(Array.from({ length: result.tabs.length }, (_, i) => result.hiddenBefore + i));
    }
  });
  test("empty, missing active, Unicode and zero-width terminals", () => {
    expect(tabWindow([], undefined, 20).tabs).toEqual([]);
    expect(tabWindow(tabs, "3", 0)).toEqual({ tabs: [], hiddenBefore: 0, hiddenAfter: 7, width: 0 });
    expect(tabWindow(tabs, "missing", 4).tabs[0]?.index).toBe(0);
    expect(tabWindow(tabs, "3", 1).tabs[0]?.label).toBe("…");
    const unicode = [{ id: "a", label: "界👩‍💻é" }];
    expect(tabWindow(unicode, "a", 4).tabs[0]?.label).toBe("界…");
    expect(tabWindow(unicode, "a", 4).width).toBe(3);
    expect(tabWindow(tabs, "3", 12, " ").width).toBeLessThanOrEqual(12);
  });
});

describe("truthful status projection", () => {
  test("absent facts do not synthesize mode, cursor, language, Git or LSP", () => {
    expect(statusModel({})).toEqual({ segments: [] });
    expect(statusModel({ dirty: false, git: {}, lsp: { status: "" } })).toEqual({ segments: [] });
    expect(statusModel({ connection: { phase: "reconnecting" } }).segments).toEqual([{ kind: "connection", text: "reconnecting" }]);
  });
  test("projects real editor/repository facts with one-based display coordinates", () => {
    expect(statusModel({ mode: "insert", dirty: true, cursor: { line: 2, column: 4 }, totalLines: 20,
      language: "Rust", connection: "connected", git: { branch: "main", added: 0, deleted: 2 },
      lsp: { status: "Active", label: "rust-analyzer" },
    }).segments).toEqual([
      { kind: "mode", text: "INSERT" }, { kind: "dirty", text: "Modified" },
      { kind: "cursor", text: "Ln 3, Col 5" }, { kind: "lines", text: "20 lines" },
      { kind: "language", text: "Rust" }, { kind: "connection", text: "connected" },
      { kind: "git", text: "main +0 −2" }, { kind: "lsp", text: "rust-analyzer: Active" },
    ]);
    expect(statusModel({ cursor: { line: 0, column: 0 }, lsp: { status: "Missing" } }).segments).toEqual([
      { kind: "cursor", text: "Ln 1, Col 1" }, { kind: "lsp", text: "LSP: Missing" },
    ]);
  });
  test("invalid numeric facts are omitted rather than corrected into fake values", () => {
    expect(statusModel({ cursor: { line: -1, column: NaN }, totalLines: Infinity, git: { added: -1, deleted: 1.5 } }).segments).toEqual([]);
  });
});