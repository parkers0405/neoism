import { expect, test } from "bun:test";
import { inactiveTabIcon, tabChrome, workspaceLabel } from "../src/layout/tab-chrome";

test("tabs use the original Rust natural measured geometry and exact close reservation", () => {
  const open = tabChrome(1, 137, 16, true).buffer, shell = tabChrome(1, 137, 16, false).buffer;
  expect(open.width).toBeCloseTo(199.5);
  expect(open.width - shell.width).toBe(11.5);
  expect(open.titleWidth).toBe(137);
  expect(open.closeCenter).toBe(open.width - 12);
  expect(shell.closeCenter).toBeNull();
  expect(tabChrome(1, 5000, 12, true).buffer.width).toBe(520);
  expect(tabChrome(2, 80, 24, true).buffer.newWidth).toBe(60);
});

test("workspace chrome and inactive icon colors come from the original source contract", () => {
  const spec = tabChrome(1.5).workspace;
  expect(spec.height).toBe(42); expect(spec.tabPaddingX).toBe(36); expect(spec.tabRadius).toBe(9);
  expect(spec.iconGlyph).toBe("\uf07b");
  expect(workspaceLabel("/home/projects/neoism/", null)).toBe("neoism");
  expect(workspaceLabel("Workspace 3", "/project/actual-name")).toBe("actual-name");
  // A re-rooted workspace names its current folder, never a stale title.
  expect(workspaceLabel("ktblack-main-api", "/home/me", "/home/me")).toBe("~");
  expect(workspaceLabel("ktblack-main-api", "/home/me/projects/neoism/", "/home/me")).toBe("neoism");
  expect(inactiveTabIcon("#7ebae4")).toBe("#427ea8a0");
});
