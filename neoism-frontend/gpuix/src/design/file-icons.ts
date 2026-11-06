import { extensionIcons, namedIcons } from "./generated";
import { isDatabasePath, isDatabaseSidecarPath } from "../database/file-path";

/** Both frontends use the file tree's generated Rust icon/color tables. */
export function fileIcon(name: string) {
  if (isDatabasePath(name)) return { glyph: "\uf1c0", color: "#c8c8c8" };
  if (isDatabaseSidecarPath(name)) return { glyph: "\uf1c0", color: "#888888" };
  const lower = name.toLowerCase();
  const extension = lower.split(".").pop() || "";
  return (Object.hasOwn(namedIcons, lower) ? namedIcons[lower] : undefined)
    || (Object.hasOwn(extensionIcons, extension) ? extensionIcons[extension] : undefined)
    || { glyph: "\uf15b", color: "#c8c8c8" };
}