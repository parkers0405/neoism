import type { Palette } from "../../gpuix/src/design/generated";
// Bun embeds this as text (also in compiled builds); no runtime filesystem IO.
// @ts-expect-error TypeScript has no module declaration for Bun's Rust text import.
import source from "../../shared/src/panels/command_palette/render.rs" with { type: "text" };

export interface PreviewRun { text: string; token: keyof Palette }

/** Extract the native painter's literal runs, not a second hand-maintained sample.
 * Deliberately reject source-shape changes rather than silently inventing content. */
export function extractThemePreview(source: string): PreviewRun[][] {
  const painter = source.split("fn draw_theme_preview<")[1]?.split("/// Paint a rounded-rect outline")[0];
  if (!painter) throw new Error("Native theme preview painter not found");
  const rows: PreviewRun[][] = [];
  for (const call of painter.matchAll(/draw_line\(\s*sugarloaf,\s*(\d+),\s*&\[([\s\S]*?)\],?\s*\);/g)) {
    const runs = [...call[2]!.matchAll(/\("((?:\\.|[^"\\])*)", preview\.(\w+)\)/g)].map(part => ({
      text: JSON.parse(`"${part[1]}"`) as string, token: part[2] as keyof Palette,
    }));
    if (!runs.length || rows[Number(call[1])]) throw new Error("Native theme preview run shape changed");
    rows[Number(call[1])] = runs;
  }
  if (rows.length !== 8 || Array.from(rows).some(row => !row?.length)) throw new Error("Native theme preview rows changed");
  return rows;
}

export const themePreview = extractThemePreview(source);

// render.rs::theme_display_name
export const themeDisplayName = (name: string) => name === "nvchad_one" ? "One Dark"
  : name.split(/[_-]/).filter(Boolean).map(part => part[0]!.toUpperCase() + part.slice(1)).join(" ");