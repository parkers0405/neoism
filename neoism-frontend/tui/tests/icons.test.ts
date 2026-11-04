import { expect, test } from "bun:test";
import { extensionIcons, namedIcons } from "../../gpuix/src/design/generated";
import { fileIcon, icons, commandIcon } from "../src/icons";
import { fileIcon as guiFileIcon } from "../../gpuix/src/design/file-icons";

test("GUI and TUI share exact Nerd Font file glyphs and colors, including named overrides", () => {
  expect(fileIcon).toBe(guiFileIcon);
  for (const [extension, icon] of Object.entries(extensionIcons)) expect(fileIcon(`file.${extension}`)).toEqual(icon);
  for (const [name, icon] of Object.entries(namedIcons)) expect(fileIcon(name.toUpperCase())).toEqual(icon);
  expect(fileIcon("constructor")).toEqual({ glyph: "\uf15b", color: "#c8c8c8" });
  expect(fileIcon("file.unknown-extension").glyph).toBe(icons.file);
});

test("chrome and action glyphs match the GUI's existing Nerd Font defaults", () => {
  expect(icons.folder).toBe("\uf07b"); expect(icons.folderOpen).toBe("\uf07c");
  expect(icons.search).toBe("\uf002"); expect(icons.mode).toBe("\ue7c5");
  expect(icons.branch).toBe("\ue725"); expect(icons.lsp).toBe("\uf0e7");
  expect(commandIcon("themes")).toBe("\uf1fc");
  expect(commandIcon("search")).toBe(icons.search);
});