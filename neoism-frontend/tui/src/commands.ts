export type CommandId = "files" | "home" | "refresh" | "reconnect" | "close" | "quit" | "search" | "grep" | "themes" | "save" | "save-all" | "next-buffer" | "prev-buffer" | "next-workspace" | "prev-workspace" | "hover" | "completion" | "signature" | "definition" | "references" | "symbols" | "diagnostics" | "actions" | "rename" | "format" | "vim" | "new-file" | "directory" | "new-workspace";
export const commands: readonly { id: CommandId; title: string; key: string }[] = [
  { id: "files", title: "Open file tree", key: "Alt + E" },
  { id: "new-file", title: "Create a new file", key: "" },
  { id: "directory", title: "Change workspace directory", key: "Alt + D" },
  { id: "new-workspace", title: "Create a workspace", key: "Ctrl + Shift + W" },
  { id: "search", title: "Search files", key: "Alt + S" },
  { id: "grep", title: "Search project contents", key: "Alt + Shift + S" },
  { id: "themes", title: "Change theme", key: "" },
  { id: "save", title: "Save buffer", key: "Ctrl + S" },
  { id: "save-all", title: "Save all buffers", key: "" },
  { id: "next-buffer", title: "Next buffer tab", key: "Ctrl + Tab" },
  { id: "prev-buffer", title: "Previous buffer tab", key: "Ctrl + Shift + Tab" },
  { id: "next-workspace", title: "Next workspace", key: "Alt + ]" },
  { id: "prev-workspace", title: "Previous workspace", key: "Alt + [" },
  { id: "hover", title: "LSP hover documentation", key: "K / Shift + F1" },
  { id: "completion", title: "LSP completion", key: "Ctrl + Space" },
  { id: "signature", title: "LSP signature help", key: "Ctrl + Shift + Space" },
  { id: "definition", title: "Go to definition", key: "gd / F12" },
  { id: "references", title: "Find references", key: "gr / Shift + F12" },
  { id: "symbols", title: "Document symbols", key: "" },
  { id: "diagnostics", title: "Document diagnostics", key: "" },
  { id: "actions", title: "LSP code actions", key: "Ctrl + ." },
  { id: "rename", title: "Rename symbol (updates project files)", key: "F2" },
  { id: "format", title: "Format document", key: "Alt + Shift + F" },
  { id: "vim", title: "Toggle Vim mode", key: "Alt + Shift + Space" },
  { id: "home", title: "Welcome", key: "Alt + 0" },
  { id: "refresh", title: "Refresh workspace", key: "F5" },
  { id: "reconnect", title: "Reconnect to daemon", key: "" },
  { id: "close", title: "Close buffer", key: "Ctrl + W" },
  { id: "quit", title: "Quit terminal frontend", key: "Ctrl + Q" },
];

export function matchingCommands(query: string) {
  const words = query.toLowerCase().trim().split(/\s+/);
  return commands.filter(command => words.every(word => command.title.toLowerCase().includes(word)));
}

/** Match the GUI proportions while preserving a usable center on small TTYs. */
export function terminalLayout(width: number, height: number, sidebar: boolean) {
  const showSidebar = sidebar && width >= 76 && height >= 12;
  const sidebarWidth = showSidebar ? Math.min(30, Math.max(22, Math.floor(width * 0.23))) : 0;
  return { showSidebar, sidebarWidth, contentWidth: Math.max(1, width - sidebarWidth), compact: width < 76 || height < 22 };
}