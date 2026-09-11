#!/usr/bin/env node
// Node >=22.13, no dependencies. No font downloads or installed-font enumeration.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { stripTypeScriptTypes } from 'node:module';
import { buildFontAssets, fontSources, generateFontAssets } from './generate-font-assets.mjs';

assert(generateFontAssets(true));
const gui = new URL('../', import.meta.url);
const root = new URL('../../../../../../', import.meta.url);
const load = source => import('data:text/javascript;base64,' + Buffer.from(stripTypeScriptTypes(source)).toString('base64'));
const { fonts, systemFontOptions, localFontAccess } = await load(readFileSync(new URL('src/generated/fonts.ts', gui), 'utf8'));
const css = readFileSync(new URL('src/generated/fonts.css', gui), 'utf8');
const provenance = JSON.parse(readFileSync(new URL('public/fonts/SOURCES.json', gui), 'utf8'));
assert.equal(fonts.length, 6);
assert.equal(new Set(fonts.map(f => f.id)).size, 6);
assert.equal(fonts.flatMap(f => f.faces).length, 13);
assert.equal((css.match(/@font-face/g) ?? []).length, 13);
assert(!css.includes('local('), 'Do not substitute unpredictable local versions for bundled bytes');
for (const font of fonts) {
  assert.equal(font.availability, 'bundled');
  assert(font.cssFamily.includes(JSON.stringify(font.family)) || font.cssFamily.startsWith(`${font.family},`));
  assert(css.includes(`font-family: ${JSON.stringify(font.family)}`));
  const license = readFileSync(new URL('public' + font.licenseUrl, gui), 'utf8');
  assert.match(license, /Copyright/);
  assert.match(license, /SIL OPEN FONT LICENSE Version 1.1/);
  assert.match(license, /TERMINATION/);
  assert.match(license, /DISCLAIMER/);
  for (const face of font.faces) assert(css.includes(`url(${JSON.stringify(face.url)})`));
}
const geist = fonts.find(f => f.id === 'geist');
assert.equal(geist.family, 'Geist');
assert.equal(geist.cssFamily, 'Geist, ui-sans-serif, system-ui, sans-serif');
assert.equal(geist.faces.length, 1);
assert.equal(geist.faces[0].weight, '100 900');
assert.equal(geist.faces[0].style, 'normal');
assert.match(css, /font-family: "Geist";[^}]*font-weight: 100 900;/);
const defaultSans = systemFontOptions.find(f => f.id === 'system-sans');
assert.equal(defaultSans.name, 'Default sans');
assert.equal(defaultSans.cssFamily, geist.cssFamily);
// Read the actual SFNT variation table: a CSS range alone does not make a variable font.
const geistBytes = readFileSync(new URL('public' + geist.faces[0].url, gui));
assert.equal(createHash('sha256').update(geistBytes).digest('hex'), '73894e0448cae90a92b6c2f8732b7bb9acb7b94c418bff559dad4a18e1de9659');
let fvar;
for (let i = 0; i < geistBytes.readUInt16BE(4); i++) {
  const entry = 12 + i * 16;
  if (geistBytes.toString('ascii', entry, entry + 4) === 'fvar') fvar = geistBytes.readUInt32BE(entry + 8);
}
assert(fvar, 'Geist must contain real variable axes');
const axes = new Map();
for (let i = 0; i < geistBytes.readUInt16BE(fvar + 8); i++) {
  const offset = fvar + geistBytes.readUInt16BE(fvar + 4) + i * geistBytes.readUInt16BE(fvar + 10);
  axes.set(geistBytes.toString('ascii', offset, offset + 4), [4, 8, 12].map(n => geistBytes.readInt32BE(offset + n) / 65536));
}
assert.deepEqual(axes.get('wght'), [100, 400, 900]);
assert.equal(axes.size, 1);
for (const weight of [440, 530, 640]) assert(weight >= axes.get('wght')[0] && weight <= axes.get('wght')[2]);
assert(geistBytes.length < 200_000, 'Keep Geist to a single small variable face');
const pixel = fonts.find(f => f.id === 'press-start-2p');
assert.equal(pixel.family, 'Press Start 2P');
assert.equal(pixel.cssFamily, '"Press Start 2P", monospace');
assert.deepEqual(pixel.faces, [{ url: '/fonts/press-start-2p/PressStart2P-Regular.ttf', weight: 400, style: 'normal', format: 'truetype' }]);
assert.match(css, /font-family: "Press Start 2P";[^}]*font-weight: 400;/);
const pixelBytes = readFileSync(new URL('public' + pixel.faces[0].url, gui));
assert.equal(createHash('sha256').update(pixelBytes).digest('hex'), '034c77f1f05ec89421e4a63f0e3a4ca1ecf852cc6d2bf611f126f275728e017d');
assert(pixelBytes.equals(readFileSync(new URL('sugarloaf/src/font/resources/PressStart2P/PressStart2P-Regular.ttf', root))));
assert(readFileSync(new URL('public' + pixel.licenseUrl, gui)).equals(readFileSync(new URL('sugarloaf/src/font/resources/PressStart2P/OFL.txt', root))));
assert(!fonts.some(f => f.id === 'inter'), 'Superseded Inter must not remain a UI choice');
assert(fonts.some(f => f.id === 'geist-mono'));
assert(fonts.some(f => f.id === 'jetbrains-mono'));
assert.equal(fonts.filter(f => f.kind === 'display').length, 3);
assert.equal(systemFontOptions.length, 3);
assert(systemFontOptions.every(f => f.availability === 'system-fallback' && f.faces.length === 0));
assert.equal(localFontAccess.availability, 'runtime-only');
assert(localFontAccess.requiresUserPermission && localFontAccess.requiresSecureContext);
let total = 0;
for (const record of provenance.files) {
  const src = readFileSync(new URL(record.source, root));
  const dst = readFileSync(new URL(record.destination, gui));
  assert(src.equals(dst), `Not a byte-identical copy: ${record.destination}`);
  assert.equal(dst.length, record.bytes);
  assert.equal(createHash('sha256').update(dst).digest('hex'), record.sha256);
  if (record.mime.startsWith('font/')) total += dst.length;
}
assert(total - geistBytes.length - pixelBytes.length < 1_500_000, 'Existing curated font payload should stay below 1.5 MB');
assert(total < 1_600_000, 'Total font payload including Geist and native pixel headings should stay below 1.6 MB');
const first = buildFontAssets(), second = buildFontAssets();
for (const [path, bytes] of first) assert(bytes.equals(second.get(path)), `Non-reproducible: ${path}`);

// Negative tests live in a temporary replica, never corrupting actual output/source files.
const dir = mkdtempSync(join(tmpdir(), 'neoism-font-drift-'));
try {
  const scriptPath = 'neoism-agent/sdk/typescript/packages/gui/scripts/generate-font-assets.mjs';
  const paths = [scriptPath, ...provenance.files.map(f => f.source)];
  for (const path of paths) {
    const target = join(dir, path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, readFileSync(new URL(path, root)));
  }
  const generator = join(dir, scriptPath);
  const run = (...args) => spawnSync(process.execPath, [generator, ...args], { encoding: 'utf8' });
  let result = run(); assert.equal(result.status, 0, result.stderr);
  assert.equal(run('--check').status, 0);
  const target = join(dirname(generator), '../public/fonts/geist-mono/GeistMono-Regular.otf');
  const corrupted = readFileSync(target); corrupted[corrupted.length - 1] ^= 1;
  writeFileSync(target, corrupted);
  result = run('--check'); assert.equal(result.status, 1); assert.match(result.stderr, /Font asset drift/);
  assert.equal(run().status, 0);
  const native = join(dir, fontSources[0].faces[0].source);
  writeFileSync(native, corrupted);
  assert.equal(run('--check').status, 1, 'Native source changes must detect binary/manifest drift');
  assert.equal(run().status, 0);
  assert.equal(run('--check').status, 0);
  // Removing all generated fonts must not remove vendored inputs or require a download.
  rmSync(join(dirname(generator), '../public/fonts'), { recursive: true });
  assert.equal(run().status, 0);
  assert.equal(run('--check').status, 0);
  const notice = join(dirname(generator), '../public/fonts/licenses/GeistMono-OFL.txt');
  rmSync(notice);
  assert.equal(run('--check').status, 1, 'Missing license must fail check');
} finally { rmSync(dir, { recursive: true, force: true }); }
console.log(`Font tests passed: 6 families, 13 faces, ${total.toLocaleString('en-US')} font bytes, source SHA-256 identity and drift detection.`);
