import type { CommandId } from "./commands";
export { fileIcon } from "../../gpuix/src/design/file-icons";

// Same defaults as shared/{panels/chrome_topbar,panels/status_line,
// panels/file_tree,primitives/icons}.rs; no ASCII substitutes or separate set.
export const icons = {
  folder: "\uf07b", folderOpen: "\uf07c", file: "\uf15b",
  chevronRight: "\uf054", chevronDown: "\uf078",
  home: "\uf015", workspace: "\uf0e8", search: "\uf002", commands: "\uf0c9",
  themes: "\uf1fc", mode: "\ue7c5", branch: "\ue725", lsp: "\uf0e7",
  error: "\uea87", warning: "\uf071", code: "\uf121", panel: "\ueb56",
} as const;

export function commandIcon(id: string) {
  if (["hover", "completion", "signature", "definition", "references", "symbols", "diagnostics", "actions", "rename", "format"].includes(id)) return icons.lsp;
  if (id === "themes") return icons.themes;
  if (id === "home") return icons.home;
  if (id === "search" || id === "grep") return icons.search;
  if (["save", "save-all", "vim"].includes(id as CommandId)) return icons.code;
  if (id === "new-file") return icons.file;
  if (["files", "directory", "new-workspace", "next-workspace", "prev-workspace"].includes(id)) return icons.folder;
  return icons.commands;
}