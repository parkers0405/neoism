import { expect, test } from "bun:test";
import { blockTextPoint, blockTextRanges, blockSelectedText, type BlockTextRow } from "../src/terminal/block-selection";

test("selection crosses command/output and block boundaries in either direction", () => {
  const rows: BlockTextRow[] = [
    { id: "1:command:0", top: 20, text: "printf hello" },
    { id: "1:output:0", top: 50, text: "hello" },
    { id: "2:command:0", top: 100, text: "echo next" },
    { id: "2:output:0", top: 130, text: "next" },
  ];
  const anchor = blockTextPoint(rows, 25, 7)!, head = blockTextPoint(rows, 135, 2)!;
  expect(blockSelectedText(rows, { anchor, head })).toBe("hello\nhello\necho next\nne");
  expect(blockSelectedText(rows, { anchor: head, head: anchor })).toBe("hello\nhello\necho next\nne");
  expect(blockTextRanges(rows, { anchor, head: anchor })).toEqual([]);
  expect(blockSelectedText(rows.slice(1), { anchor, head })).toBe("");
  expect(blockSelectedText(rows, { anchor: { id: rows[0].id, column: 6 }, head: { id: rows[0].id, column: 7 } })).toBe(" ");
});

test("native cell gaps, wide graphemes and combining accents survive selection", () => {
  const row: BlockTextRow = { id: "output", top: 0, runs: [
    { text: "a界é", column: 0, width: 4, fg: "#ffffff", bold: false, italic: false, underline: false },
    { text: "👩‍💻!", column: 6, width: 3, fg: "#ffffff", bold: false, italic: false, underline: false },
  ] };
  const anchor = blockTextPoint([row], 0, 1)!, head = blockTextPoint([row], 0, 8)!;
  expect(blockSelectedText([row], { anchor, head })).toBe("界é  👩‍💻");
  expect(blockTextPoint([row], 0, 2)?.column).toBe(3);
  expect(blockTextPoint([row], 0, 999)?.column).toBe(9);
});