import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export function ghosttyLibraryPath() {
  const name = process.platform === "win32" ? "ghostty-vt.dll" : process.platform === "darwin" ? "libghostty-vt.dylib" : "libghostty-vt.so";
  const candidates = [process.env.NEOISM_GPUIX_GHOSTTY, join(dirname(process.execPath), "web/native", name), join(dirname(process.execPath), "../Resources/web/native", name), join(dirname(process.execPath), name),
    fileURLToPath(new URL(`../artifacts/${name}`, import.meta.url))];
  const path = candidates.find((path): path is string => !!path && existsSync(path));
  if (!path) throw new Error("Ghostty's terminal engine is not built. Run npm run build:ghostty.");
  return path;
}
