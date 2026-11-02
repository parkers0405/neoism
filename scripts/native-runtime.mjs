// `web` is a legacy runtime directory, not a browser distribution. Old
// updaters require index.html, but it must never redirect to a browser UI.
import {mkdirSync, readdirSync, rmSync, writeFileSync} from 'node:fs';
export const runtimeMarker = '<!-- Neoism native runtime resources; no browser UI. -->\n';
export function pruneBrowserAssets(directory) {
  mkdirSync(directory, {recursive: true});
  for (const name of readdirSync(directory)) {
    if (name !== 'app' && name !== 'native') rmSync(`${directory}/${name}`, {recursive: true, force: true});
  }
  writeFileSync(`${directory}/index.html`, runtimeMarker);
}