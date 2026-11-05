import {test} from 'node:test';
import assert from 'node:assert/strict';
import {existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {dirname, join} from 'node:path';
import {spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {createHash} from 'node:crypto';
import {buildDocs, docsSource, stageDocsArtifact, verifyDocsArtifact} from './build-docs.mjs';

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'neoism docs packaging '));
  t.after(() => rmSync(root, {recursive: true, force: true}));
  const source = join(root, 'shared'), artifact = join(source, 'dist'), destination = join(root, 'bundle/resources/docs');
  function write(path, text) { mkdirSync(dirname(path), {recursive: true}); writeFileSync(path, text); }
  for (const name of ['api/docs/index.html', 'api/docs/reference/index.html', 'content.json', 'assets/site.css']) {
    write(join(artifact, name), name.endsWith('.json') ? '{"schemaVersion":1,"version":"1.0.0","pages":[]}' : 'site fixture');
  }
  write(join(artifact, 'manifest.json'), '{"schemaVersion":1,"version":"1.0.0","pages":[]}');
  write(join(artifact, 'reference/openapi.json'), '{"openapi":"3.1.0"}');
  write(join(source, 'bundle/content.json'), readFileSync(join(artifact, 'content.json')));
  function inventory() {
    const files = {};
    function walk(dir, prefix = '') {
      for (const entry of readdirSync(dir, {withFileTypes: true})) {
        const name = prefix + entry.name;
        if (entry.isDirectory()) walk(join(dir, entry.name), name + '/');
        else if (name !== 'artifact.json') {
          const bytes = readFileSync(join(dir, entry.name));
          files[name] = {bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex')};
        }
      }
    }
    walk(artifact);
    write(join(artifact, 'artifact.json'), JSON.stringify({schemaVersion: 1, version: '1.0.0', files}));
  }
  inventory();
  write(join(source, 'content/private.md'), 'must not ship');
  return {root, source, artifact, destination, write, inventory};
}

test('explicit artifact reuse replaces only docs, excluding raw source and stale files', t => {
  const f = fixture(t);
  f.write(join(f.destination, 'stale.md'), 'old source');
  f.write(join(f.root, 'bundle/resources/other.txt'), 'keep');
  buildDocs({source: f.source, artifact: f.artifact, destination: f.destination});
  assert.equal(verifyDocsArtifact(f.destination), f.destination);
  assert.equal(existsSync(join(f.destination, 'stale.md')), false);
  assert.equal(existsSync(join(f.destination, 'content/private.md')), false);
  assert.equal(readFileSync(join(f.root, 'bundle/resources/other.txt'), 'utf8'), 'keep');
  const result = spawnSync(process.execPath, [fileURLToPath(new URL('./build-docs.mjs', import.meta.url)), '--verify', f.destination], {encoding: 'utf8'});
  assert.equal(result.status, 0, result.stderr);
});

test('incomplete/invalid/source artifacts fail before touching the prior staged site', t => {
  const f = fixture(t);
  stageDocsArtifact(f.artifact, f.destination);
  for (const name of ['raw.md', 'Notes/secret.txt', 'node_modules/pkg.js', '.git/config']) {
    f.write(join(f.artifact, name), 'not a site resource');
    assert.throws(() => stageDocsArtifact(f.artifact, f.destination), /not a docs site artifact/);
    rmSync(join(f.artifact, name.split('/')[0]), {recursive: true});
  }
  f.write(join(f.artifact, 'content.json'), 'invalid json');
  assert.throws(() => stageDocsArtifact(f.artifact, f.destination));
  assert.equal(readFileSync(join(f.destination, 'content.json'), 'utf8'), '{"schemaVersion":1,"version":"1.0.0","pages":[]}');
  rmSync(join(f.artifact, 'api/docs/reference/index.html'));
  assert.throws(() => verifyDocsArtifact(f.artifact), /Missing docs site resource/);
});

test('reject overlapping source/destination trees and symlink payloads', t => {
  const f = fixture(t);
  for (const destination of [f.artifact, join(f.artifact, 'nested'), f.source]) {
    assert.throws(() => stageDocsArtifact(f.artifact, destination), /separate trees/);
  }
  if (process.platform !== 'win32') {
    symlinkSync(join(f.source, 'content/private.md'), join(f.artifact, 'leak.txt'));
    assert.throws(() => verifyDocsArtifact(f.artifact), /regular files/);
  }
});

test('normal build runs npm ci and npm build in the supplied pinned source', t => {
  const f = fixture(t);
  const pkg = {name: 'docs-packaging-fixture', version: '1.0.0', private: true, scripts: {build: 'node build.cjs'}};
  f.write(join(f.source, 'package.json'), JSON.stringify(pkg));
  f.write(join(f.source, 'package-lock.json'), JSON.stringify({name: pkg.name, version: pkg.version, lockfileVersion: 3, packages: {'': {name: pkg.name, version: pkg.version}}}));
  f.write(join(f.source, 'build.cjs'), 'require("node:fs").writeFileSync("build-ran.txt", "rebuilt")');
  buildDocs({source: f.source, destination: f.destination});
  assert.equal(readFileSync(join(f.source, 'build-ran.txt'), 'utf8'), 'rebuilt');
});

test('inventory catches size/hash tampering, missing entries, extras and unsafe paths', t => {
  const f = fixture(t);
  const path = join(f.artifact, 'artifact.json');
  const original = JSON.parse(readFileSync(path, 'utf8'));
  for (const field of ['bytes', 'sha256']) {
    const inventory = structuredClone(original);
    inventory.files['assets/site.css'][field] = field === 'bytes' ? 0 : '0'.repeat(64);
    f.write(path, JSON.stringify(inventory));
    assert.throws(() => verifyDocsArtifact(f.artifact), /size\/hash mismatch/);
  }
  f.write(path, JSON.stringify(original));
  f.write(join(f.artifact, 'unlisted.css'), 'extra');
  assert.throws(() => verifyDocsArtifact(f.artifact), /unlisted inventory files/);
  rmSync(join(f.artifact, 'unlisted.css'));
  for (const name of ['../outside', '/absolute', 'assets\\escape', 'C:/escape', 'artifact.json', 'missing.html']) {
    f.write(path, JSON.stringify({...original, files: {...original.files, [name]: {bytes: 0, sha256: ''}}}));
    assert.throws(() => verifyDocsArtifact(f.artifact), /Invalid\/missing docs inventory file/);
  }
  f.write(path, JSON.stringify(original));
  f.write(join(f.artifact, 'assets/site.css'), 'tampered');
  assert.throws(() => verifyDocsArtifact(f.artifact), /size\/hash mismatch/);
});

test('manifest/content/inventory versions must agree even for standalone verification', t => {
  const f = fixture(t);
  f.write(join(f.artifact, 'manifest.json'), '{"version":"another"}');
  f.inventory();
  assert.throws(() => verifyDocsArtifact(f.artifact), /content.json\/manifest.json version mismatch/);
  f.write(join(f.artifact, 'manifest.json'), '{"version":"1.0.0"}');
  f.inventory();
  const inventory = JSON.parse(readFileSync(join(f.artifact, 'artifact.json'), 'utf8'));
  inventory.version = 'another';
  f.write(join(f.artifact, 'artifact.json'), JSON.stringify(inventory));
  assert.throws(() => verifyDocsArtifact(f.artifact), /artifact.json\/content.json version mismatch/);
});

test('explicit reuse matches selected source and requires an explicit standalone opt-out', t => {
  const f = fixture(t);
  if (existsSync(join(docsSource, 'bundle/content.json'))) {
    assert.throws(() => buildDocs({artifact: f.artifact, destination: f.destination}), /another docs revision/);
    assert.equal(existsSync(f.destination), false);
  }
  buildDocs({source: f.source, artifact: f.artifact, destination: f.destination});
  f.write(join(f.source, 'bundle/content.json'), '{"version":"another","sourcesSha256":"different"}');
  assert.throws(() => buildDocs({source: f.source, artifact: f.artifact, destination: f.destination}), /another docs revision/);
  assert.equal(verifyDocsArtifact(f.destination), f.destination); // rejection kept staged site intact
  rmSync(join(f.source, 'bundle/content.json'));
  assert.throws(() => buildDocs({source: f.source, artifact: f.artifact, destination: f.destination}), /Selected docs source is missing/);
  buildDocs({source: f.source, artifact: f.artifact, destination: f.destination, standalone: true});
  assert.throws(() => buildDocs({source: f.source, destination: f.destination, standalone: true}), /requires explicit --artifact/);
});

test('missing submodule fails with a pinned initialization recipe, never fetches', t => {
  const f = fixture(t);
  assert.throws(() => buildDocs({source: join(f.root, 'absent'), destination: f.destination}), /git submodule update --init --recursive/);
  assert.equal(existsSync(f.destination), false);
});