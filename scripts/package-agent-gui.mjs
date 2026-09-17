#!/usr/bin/env node
// Stage the standalone agent GUI inside the resource tree already carried by
// every installer/updater. Never merge hashed assets from different builds.
import { cpSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, renameSync, rmSync } from 'node:fs';
import { dirname, join, relative, resolve, isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';

export function validateGui(root) {
  root = resolve(root);
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name);
      const stat = lstatSync(path);
      if (stat.isSymbolicLink()) throw new Error(`GUI assets must not contain symlinks: ${path}`);
      if (stat.isDirectory()) walk(path);
      else if (!stat.isFile()) throw new Error(`Not a regular GUI asset: ${path}`);
    }
  };
  walk(root);
  const html = readFileSync(join(root, 'index.html'), 'utf8');
  const refs = [...html.matchAll(/(?:src|href)="([^"]+)"/g)].map(m => m[1]);
  if (!refs.some(ref => /(^|\/)assets\/[^/]+\.js$/.test(ref.split(/[?#]/)[0]) && !ref.startsWith('http'))) {
    throw new Error('GUI index is not a production Vite build');
  }
  for (const ref of refs) {
    if (/^(?:https?:|data:|#)/.test(ref)) continue;
    if (ref.startsWith('/')) throw new Error(`Root-absolute GUI asset: ${ref}`);
    const path = resolve(root, ref.replace(/^\.\//, '').split(/[?#]/)[0]);
    const rel = relative(root, path);
    if (rel.startsWith('..') || isAbsolute(rel)) throw new Error(`Unsafe asset: ${ref}`);
    if (!lstatSync(path).isFile()) throw new Error(`Missing GUI asset: ${ref}`);
  }
  for (const dir of ['assets', 'fonts', 'syntax']) {
    if (!readdirSync(join(root, dir)).length) throw new Error(`Empty GUI ${dir}`);
  }
  for (const name of ['index.html', ...readdirSync(join(root, 'assets')).map(n => join('assets', n))]) {
    const path = join(root, name);
    if (!existsSync(path) || !lstatSync(path).isFile()) continue;
    if (!/\.(html|js|css|mjs)$/.test(path)) continue;
    const text = readFileSync(path, 'utf8');
    if (/(?:src|href|url\()["']?\/(?:assets|fonts|syntax)\//.test(text)) {
      throw new Error(`Root-absolute public URL in ${name}`);
    }
  }
}

export function stageGui(source, destination) {
  validateGui(source);
  const staged = `${destination}.new`;
  const backup = `${destination}.old`;
  if (existsSync(staged) || existsSync(backup)) throw new Error(`Unfinished GUI staging at ${destination}`);
  mkdirSync(dirname(destination), { recursive: true });
  cpSync(source, staged, { recursive: true });
  validateGui(staged);
  const existed = existsSync(destination);
  if (existed) renameSync(destination, backup);
  try { renameSync(staged, destination); }
  catch (error) { if (existed) renameSync(backup, destination); throw error; }
  rmSync(backup, { recursive: true, force: true });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [source, destination] = process.argv.slice(2);
  if (!source) throw new Error('Usage: node scripts/package-agent-gui.mjs DIST [DESTINATION]');
  if (destination) stageGui(source, destination);
  else validateGui(source);
  console.log(`Validated agent GUI: ${destination || source}`);
}
