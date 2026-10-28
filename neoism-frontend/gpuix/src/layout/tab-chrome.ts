import { coreCall } from "../native-core";
export interface TabChrome {
  workspace: { height: number; titleFontSize: number; tabPaddingX: number; tabRadius: number; marginRight: number; iconGlyph: string; iconColor: number[] };
  buffer: { height: number; fontSize: number; iconSize: number; iconGap: number; padding: number; closeSize: number; closeHitSize: number; newWidth: number; newIcon: string; width: number; titleWidth: number; iconWidth: number; closeCenter: number | null };
}
export const tabChrome = (scale: number, titleWidth = 0, iconWidth = 12 * scale, close = true) => coreCall<TabChrome>("layout.chrome", { scale, titleWidth, iconWidth, close });
/** A workspace is its declared directory: the tab names that folder (`~` for
 * home), so re-rooting it never leaves a stale title. Title only without one. */
export function workspaceLabel(title: string, root?: string | null, home = process.env.HOME) {
  const directory = root?.replace(/\/+$/, "");
  if (directory !== undefined && directory !== null && root) {
    if (home && directory === home.replace(/\/+$/, "")) return "~";
    return directory.split("/").pop() || "/";
  }
  return title.replace(/\/+$/, "").split("/").pop() || "~";
}
export const rgba = (color: readonly number[]) => "#" + color.map((n) => Math.round(n).toString(16).padStart(2, "0")).join("");
export function inactiveTabIcon(color: string) {
  const bytes = color.replace(/^#/, "").match(/../g)?.slice(0, 3).map((n) => Math.max(0, parseInt(n, 16) - 60)) || [140, 140, 140];
  return rgba([...bytes, 160]);
}
