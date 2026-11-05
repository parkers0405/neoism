#!/usr/bin/env node
// Build the pinned shared documentation, or explicitly verify/reuse its site artifact.
import {cpSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync, rmSync} from 'node:fs';
import {dirname, isAbsolute, join, relative, resolve, sep} from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';
import {spawnSync} from 'node:child_process';
import {createHash} from 'node:crypto';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const docsSource = join(root, 'vendor/neoism-docs');
export const docsDestination = join(root, 'neoism-frontend/gpuix/dist/resources/docs');

export function verifyDocsArtifact(directory, {source, strict = false} = {}) {
  directory = resolve(directory);
  if (!existsSync(directory) || !lstatSync(directory).isDirectory()) throw new Error(`Missing docs site artifact: ${directory}`);
  const files = new Set();
  function walk(folder) {
    for (const name of readdirSync(folder)) {
      const path = join(folder, name), stat = lstatSync(path);
      if (stat.isSymbolicLink() || (!stat.isDirectory() && !stat.isFile())) throw new Error(`Docs artifact must contain only regular files: ${path}`);
      if (/^(?:\.git|node_modules|notes)$/i.test(name) || /\.(?:md|mdx|markdown)$/i.test(name)) throw new Error(`Source/private content is not a docs site artifact: ${path}`);
      if (stat.isDirectory()) walk(path);
      else files.add(relative(directory, path).split(sep).join('/'));
    }
  }
  walk(directory);
  const required = ['api/docs/index.html', 'api/docs/reference/index.html', 'content.json'];
  if (strict) required.push('manifest.json', 'artifact.json', 'reference/openapi.json');
  for (const name of required) {
    const path = join(directory, name);
    if (!existsSync(path) || !lstatSync(path).isFile() || lstatSync(path).size === 0) throw new Error(`Missing docs site resource: ${path}`);
  }
  const contentBytes = readFileSync(join(directory, 'content.json'));
  const content = JSON.parse(contentBytes);
  const hash = bytes => createHash('sha256').update(bytes).digest('hex');
  if (files.has('manifest.json')) {
    const manifest = JSON.parse(readFileSync(join(directory, 'manifest.json'), 'utf8'));
    if (typeof content?.version !== 'string' || !content.version || manifest.version !== content.version) throw new Error('Docs content.json/manifest.json version mismatch');
  }
  if (files.has('artifact.json')) {
    const inventory = JSON.parse(readFileSync(join(directory, 'artifact.json'), 'utf8'));
    if (inventory.schemaVersion !== 1 || !inventory.files || typeof inventory.files !== 'object' || Array.isArray(inventory.files)) throw new Error('Invalid docs artifact.json inventory');
    if (typeof content?.version !== 'string' || inventory.version !== content.version) throw new Error('Docs artifact.json/content.json version mismatch');
    const listed = Object.keys(inventory.files);
    for (const name of listed) {
      if (name === 'artifact.json' || name.includes('\\') || name.includes(':') || name.split('/').some(part => !part || part === '.' || part === '..') || !files.has(name)) throw new Error(`Invalid/missing docs inventory file: ${name}`);
      const bytes = readFileSync(join(directory, name)), entry = inventory.files[name];
      if (!entry || entry.bytes !== bytes.length || entry.sha256 !== hash(bytes)) throw new Error(`Docs inventory size/hash mismatch: ${name}`);
    }
    if (listed.length !== files.size - 1) throw new Error('Docs artifact contains unlisted inventory files');
  }
  if (source !== undefined) {
    const bundle = join(resolve(source), 'bundle/content.json');
    if (!existsSync(bundle)) throw new Error(`Selected docs source is missing ${bundle}; initialize the pinned submodule or explicitly use --standalone for an external artifact`);
    if (hash(contentBytes) !== hash(readFileSync(bundle))) throw new Error('Docs artifact does not match selected source bundle/content.json (another docs revision)');
  }
  return directory;
}

export function stageDocsArtifact(artifact, destination = docsDestination, verification = {}) {
  artifact = verifyDocsArtifact(artifact, verification);
  destination = resolve(destination);
  const nested = (a, b) => { const path = relative(a, b); return path === '' || (!path.startsWith('..' + sep) && path !== '..' && !isAbsolute(path)); };
  // Neither tree may contain the other (including an accidental --out vendor/...).
  if (nested(artifact, destination) || nested(destination, artifact)) throw new Error('Docs artifact and destination must be separate trees');
  mkdirSync(dirname(destination), {recursive: true});
  const temporary = mkdtempSync(join(dirname(destination), '.docs-stage-'));
  const staged = join(temporary, 'site'), backup = join(temporary, 'previous');
  let moved = false;
  try {
    cpSync(artifact, staged, {recursive: true});
    verifyDocsArtifact(staged, verification);
    if (existsSync(destination)) { renameSync(destination, backup); moved = true; }
    try { renameSync(staged, destination); }
    catch (error) { if (moved) renameSync(backup, destination); throw error; }
  } finally { rmSync(temporary, {recursive: true, force: true}); }
  return destination;
}

export function buildDocs({source = docsSource, artifact, destination = docsDestination, standalone = false} = {}) {
  if (standalone && !artifact) throw new Error('--standalone requires explicit --artifact reuse; normal builds always validate the selected source');
  if (!artifact) {
    source = resolve(source);
    if (!existsSync(join(source, 'package.json')) || !existsSync(join(source, 'package-lock.json'))) {
      throw new Error('Shared docs build is missing. Run git submodule update --init --recursive vendor/neoism-docs (never --remote), then retry. The pinned docs revision must provide package.json and package-lock.json.');
    }
    for (const args of [['ci'], ['run', 'build']]) {
      const result = spawnSync('npm', args, {cwd: source, stdio: 'inherit', shell: process.platform === 'win32'});
      if (result.error) throw result.error;
      if (result.status !== 0) throw new Error(`Shared docs npm ${args.join(' ')} failed (${result.status})`);
    }
    artifact = join(source, 'dist');
  }
  return stageDocsArtifact(artifact, destination, standalone ? {} : {source, strict: true});
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const options = {};
    let verify;
    for (let i = 2; i < process.argv.length; i++) {
      const flag = process.argv[i];
      if (flag === '--standalone') { options.standalone = true; continue; }
      if (!['--source', '--artifact', '--out', '--verify'].includes(flag) || !process.argv[i + 1] || process.argv[i + 1].startsWith('--')) throw new Error('Usage: node scripts/build-docs.mjs [--source directory] [--artifact built-dist [--standalone]] [--out resources/docs] | --verify built-dist [--source directory]');
      const value = process.argv[++i];
      if (flag === '--verify') verify = value;
      else options[{'--source': 'source', '--artifact': 'artifact', '--out': 'destination'}[flag]] = value;
    }
    if (verify && (options.artifact || options.destination || options.standalone)) throw new Error('--verify cannot be combined with build/stage options');
    console.log(verify ? `Verified docs: ${verifyDocsArtifact(verify, {source: options.source})}` : `Staged docs: ${buildDocs(options)}`);
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}