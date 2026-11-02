// `web` is a legacy runtime directory, not a browser distribution. Old
// updaters require both HTML paths, but neither may load or redirect to a UI.
import {mkdirSync, readdirSync, rmSync, writeFileSync} from 'node:fs';
export const runtimeMarker = '<!-- Neoism native runtime resources; no browser UI. -->\n';
export function pruneBrowserAssets(directory) {
  mkdirSync(directory, {recursive: true});
  for (const name of readdirSync(directory)) {
    if (name !== 'app' && name !== 'native') rmSync(`${directory}/${name}`, {recursive: true, force: true});
  }
  writeFileSync(`${directory}/index.html`, runtimeMarker);
  // Recreate only the immutable legacy updater's marker AFTER deleting old GUI assets.
  mkdirSync(`${directory}/agent-gui`, {recursive: true});
  writeFileSync(`${directory}/agent-gui/index.html`, runtimeMarker);
}