import {test} from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {pruneBrowserAssets, runtimeMarker} from './native-runtime.mjs';

test('native staging drops both stale browser UIs but keeps native runtime bytes', () => {
  const root = mkdtempSync(join(tmpdir(), 'neoism-native-runtime-'));
  try {
    for (const name of ['app', 'native', 'agent-gui', 'assets']) {
      mkdirSync(join(root, name));
      writeFileSync(join(root, name, 'payload'), name);
    }
    writeFileSync(join(root, 'index.html'), '<html>old browser UI</html>');
    writeFileSync(join(root, 'neoism_bg.wasm'), 'old workspace UI');
    pruneBrowserAssets(root);
    assert.deepEqual(readdirSync(root).sort(), ['app', 'index.html', 'native']);
    assert.equal(readFileSync(join(root, 'app/payload'), 'utf8'), 'app');
    assert.equal(readFileSync(join(root, 'native/payload'), 'utf8'), 'native');
    assert.equal(readFileSync(join(root, 'index.html'), 'utf8'), runtimeMarker);
  } finally { rmSync(root, {recursive: true, force: true}); }
});