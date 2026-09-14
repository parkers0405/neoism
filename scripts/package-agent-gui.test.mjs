import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, existsSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { stageGui, validateGui } from './package-agent-gui.mjs';

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'neoism-package-gui-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const src = join(root, 'dist');
  for (const dir of ['assets', 'fonts', 'syntax']) {
    mkdirSync(join(src, dir), { recursive: true });
    writeFileSync(join(src, dir, 'fixture.js'), 'fixture');
  }
  writeFileSync(join(src, 'index.html'), '<script type="module" src="/assets/fixture.js"></script>');
  return { root, src };
}

test('stages all assets in existing updater resource tree and removes stale hashes', t => {
  const { root, src } = fixture(t);
  const dest = join(root, 'web/agent-gui');
  stageGui(src, dest);
  writeFileSync(join(dest, 'assets/stale.js'), 'old');
  writeFileSync(join(src, 'assets/fixture.js'), 'new');
  stageGui(src, dest);
  assert.equal(readFileSync(join(dest, 'assets/fixture.js'), 'utf8'), 'new');
  assert(!existsSync(join(dest, 'assets/stale.js')));
  assert(existsSync(join(dest, 'fonts/fixture.js')));
  assert(existsSync(join(dest, 'syntax/fixture.js')));
});

test('missing build assets fail before modifying destination', t => {
  const { root, src } = fixture(t);
  const dest = join(root, 'web/agent-gui');
  stageGui(src, dest);
  rmSync(join(src, 'assets/fixture.js'));
  assert.throws(() => stageGui(src, dest));
  assert.equal(readFileSync(join(dest, 'assets/fixture.js'), 'utf8'), 'fixture');
});

test('rejects unbuilt index and traversal', t => {
  const { src } = fixture(t);
  writeFileSync(join(src, 'index.html'), '<script src="/src/main.tsx"></script>');
  assert.throws(() => validateGui(src), /production/);
  writeFileSync(join(src, 'index.html'), '<script src="/assets/fixture.js"></script><link href="/../secret">');
  assert.throws(() => validateGui(src), /Unsafe/);
});

test('rejects symlinks', { skip: process.platform === 'win32' }, t => {
  const { src } = fixture(t);
  symlinkSync('/etc/passwd', join(src, 'assets/escape.txt'));
  assert.throws(() => validateGui(src), /symlinks/);
});
