import type { CommandBlock } from "./blocks";

export function terminalDirectoryLabel(directory: string, home?: string): string {
  if (!directory) return "Terminal";
  if (!home) return directory;
  const path = directory.replace(/\\/g, "/"), root = home.replace(/\\/g, "/").replace(/\/+$/, "");
  const windows = /^[a-z]:\//i.test(path), comparedPath = windows ? path.toLowerCase() : path, comparedRoot = windows ? root.toLowerCase() : root;
  if (comparedPath === comparedRoot || !root && path === "/") return "~";
  return comparedPath.startsWith(comparedRoot + "/") ? "~" + path.slice(root.length) : directory;
}

export function commandBlockHeader(block: Pick<CommandBlock, "directory" | "duration" | "background">, home?: string): string {
  if (block.background) return "Shell output";
  return terminalDirectoryLabel(block.directory, home);
}

export function terminalDurationLabel(duration: number): string {
  const milliseconds = Number.isFinite(duration) ? Math.max(0, duration) : 0;
  return milliseconds < 1000 ? `${Math.round(milliseconds)} ms` : `${(milliseconds / 1000).toFixed(1)} s`;
}