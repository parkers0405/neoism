import { readFileSync, readdirSync, existsSync } from "node:fs";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { homedir } from "node:os";
import { parse } from "jsonc-parser";
import { coreCall } from "../native-core";
export interface BackgroundEntry { id: string; name: string; description: string; path?: string | null; opacity?: number; blur?: number; fit?: string; position_x?: number; position_y?: number }
const object = (value: unknown): Record<string, any> => typeof value === "object" && value !== null ? value as Record<string, any> : {};
/** Pack manifests are decoded in the host; the actual backend catalog file
 * owns image normalization, format checks, discovery, sorting and dedup. */
export function backgroundCatalog(values: unknown, configPath: string) {
  const raw = object(values), base = dirname(configPath), packs: Array<{ id: string; name: string; description: string; wallpaper: unknown }> = [];
  let folders: string[] = []; try { folders = readdirSync(join(base, "packs"), { withFileTypes: true }).filter((entry) => entry.isDirectory()).map((entry) => entry.name).sort(); } catch {}
  for (const id of folders) {
    const directory = join(base, "packs", id), path = ["json", "jsonc", "toml"].map((ext) => join(directory, `pack.${ext}`)).find(existsSync);
    if (!path) continue;
    try {
      const content = readFileSync(path, "utf8"), manifest = object(path.endsWith(".toml") ? Bun.TOML.parse(content) : parse(content)), pack = object(manifest.pack);
      const pathValue = typeof pack.wallpaper === "string" && pack.wallpaper.trim() ? pack.wallpaper : undefined;
      const expanded = pathValue?.replace(/^~(?=\/)/, homedir());
      packs.push({ id, name: pack.name || id, description: pack.description || "", wallpaper: expanded ? { path: isAbsolute(expanded) ? expanded : resolve(directory, expanded), opacity: Math.max(0, Math.min(1, pack["wallpaper-opacity"] ?? 1)), blur: 0, fit: "cover", "position-x": 0.5, "position-y": 0.5 } : null });
    } catch {}
  }
  const image = object(object(raw.ui).window)["background-image"], catalog = coreCall<{ entries: BackgroundEntry[] }>("menu.background-catalog", { base, image, packs });
  const inherited = packs.find((pack) => pack.id === object(raw.appearance)["mashup-pack"])?.wallpaper as Record<string, unknown> | undefined;
  const entries: BackgroundEntry[] = [{ id: "inherit", name: "Use pack/default", description: "Use the active Mash Up Pack's background", path: inherited?.path as string | undefined, opacity: Number(inherited?.opacity ?? 1), blur: 0 }, { id: "none", name: "None", description: "Disable the background image without changing the active pack" }, ...catalog.entries];
  const disabled = object(object(raw.ui).window)["background-image-disabled"], selected = disabled ? 1 : image?.path ? Math.max(0, entries.findIndex((entry) => entry.path === image.path)) : 0;
  return { entries, selected };
}
