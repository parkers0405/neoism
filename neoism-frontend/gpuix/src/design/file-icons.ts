import { extensionIcons, namedIcons } from "./generated";

/** Both frontends use the file tree's generated Rust icon/color tables. */
export function fileIcon(name: string) {
  const lower = name.toLowerCase();
  const extension = lower.split(".").pop() || "";
  return (Object.hasOwn(namedIcons, lower) ? namedIcons[lower] : undefined)
    || (Object.hasOwn(extensionIcons, extension) ? extensionIcons[extension] : undefined)
    || { glyph: "\uf15b", color: "#c8c8c8" };
}