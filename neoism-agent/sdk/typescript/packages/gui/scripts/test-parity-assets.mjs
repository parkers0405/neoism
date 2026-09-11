#!/usr/bin/env node
// Node >=22.13; no npm dependencies. --native additionally needs rustc (no Cargo build).
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { stripTypeScriptTypes } from 'node:module';

function run(command, args) {
  const r = spawnSync(command, args, { encoding: 'utf8' });
  assert.equal(r.status, 0, `${command}: ${r.error ?? ''}\n${r.stdout}\n${r.stderr}`);
  return r.stdout;
}
const load = async name => import('data:text/javascript;base64,' + Buffer.from(stripTypeScriptTypes(
  readFileSync(new URL(`../src/generated/${name}.ts`, import.meta.url), 'utf8'))).toString('base64'));
run(process.execPath, [fileURLToPath(new URL('generate-parity-assets.mjs', import.meta.url)), '--check']);
const { themes } = await load('themes');
const { commands } = await load('commands');
const { neoismLogoPath, neoismLogoViewBox } = await load('logo');
const { avatarCells, avatarGridSize } = await load('avatar');
assert.equal(themes.length, 101);
assert.equal(new Set(themes.map(t => t.id)).size, 101);
for (const theme of themes) {
  assert.equal(Object.keys(theme.colors).length, 29);
  assert.equal(theme.colors.background, theme.colors.bg);
  assert.equal(theme.colors.foreground, theme.colors.fg);
  for (const color of Object.values(theme.colors)) assert.match(color, /^#[0-9a-f]{6}$/);
}
assert.equal(themes.find(t => t.id === 'pastel_dark').colors.background, '#000000');
assert.equal(themes.find(t => t.id === 'ayu_light').colors.background, '#fafafa');
assert.equal(commands.length, 32);
const spellings = commands.flatMap(c => [c.name, ...c.aliases]);
assert.equal(spellings.length, 52);
assert.equal(new Set(spellings).size, 52);
assert.deepEqual(commands.find(c => c.name === '/model').aliases, ['/models']);
assert(commands.find(c => c.name === '/skills').aliases.includes('/skill'));
assert(commands.find(c => c.name === '/compact').aliases.includes('/comapction'));
assert.equal(neoismLogoViewBox, '0 -187.5 218.75 156.25');
assert(neoismLogoPath.startsWith('M 0 -31.25 L 0 -187.5'));
const seeds = ['', ' ', 'piss-desktop', 'other-host', 'é', '用户', '😀', 'a😀z'];
const phases = [0, 0.6, 12.5, -1];
for (const seed of seeds) {
  const grid = avatarGridSize(seed);
  assert(grid >= 11 && grid <= 14);
  for (const phase of phases) {
    const cells = avatarCells(seed, phase);
    assert.deepEqual(cells, avatarCells(seed, phase));
    assert(cells.length > grid * grid / 2 && cells.length < grid * grid);
    assert.equal(new Set(cells.map(c => `${c.x},${c.y}`)).size, cells.length);
    for (const c of cells) {
      assert(Number.isInteger(c.x) && Number.isInteger(c.y));
      assert(c.x >= 0 && c.x < grid && c.y >= 0 && c.y < grid);
      assert(Math.hypot((c.x + 0.5) / grid * 2 - 1, (c.y + 0.5) / grid * 2 - 1) <= 1.020001);
      assert.match(c.color, /^#[0-9a-f]{6}$/);
    }
  }
}
assert.deepEqual(avatarCells(''), avatarCells(' '));
assert.deepEqual(avatarCells('piss-desktop'), avatarCells('piss-desktop', 0.6));
assert.notDeepEqual(avatarCells('piss-desktop'), avatarCells('other-host'));
assert.notDeepEqual(avatarCells('piss-desktop', 0), avatarCells('piss-desktop', 1));
assert.throws(() => avatarCells('host', NaN), RangeError);

// Exercise drift detection in an isolated source tree, without touching real GUI assets.
const sandbox = mkdtempSync(join(tmpdir(), 'neoism-parity-drift-'));
try {
  const repo = new URL('../../../../../../', import.meta.url);
  const scriptDir = 'neoism-agent/sdk/typescript/packages/gui/scripts/';
  const paths = [
    'neoism-frontend/shared/src/primitives/ide_theme.rs',
    'neoism-frontend/shared/src/primitives/nvchad_themes.rs',
    'neoism-frontend/shared/src/panels/agent_pane/command_controller.rs',
    'neoism-frontend/shared/src/editor/crdt/presence_avatar.rs',
    'neoism-frontend/web/public/favicon.svg', 'THIRD_PARTY_LICENSES/NvChad-base46.txt',
    scriptDir + 'generate-parity-assets.mjs', scriptDir + 'avatar.template.ts',
  ];
  for (const path of paths) {
    const destination = join(sandbox, path);
    mkdirSync(destination.slice(0, destination.lastIndexOf('/')), { recursive: true });
    writeFileSync(destination, readFileSync(new URL(path, repo)));
  }
  const generator = join(sandbox, scriptDir, 'generate-parity-assets.mjs');
  run(process.execPath, [generator]);
  run(process.execPath, [generator, '--check']);
  const generated = join(sandbox, scriptDir, '../src/generated/commands.ts');
  writeFileSync(generated, readFileSync(generated, 'utf8') + '// stale\n');
  let drift = spawnSync(process.execPath, [generator, '--check'], { encoding: 'utf8' });
  assert.equal(drift.status, 1); assert.match(drift.stderr, /Parity asset drift/);
  run(process.execPath, [generator]);
  const catalog = join(sandbox, paths[2]);
  writeFileSync(catalog, readFileSync(catalog, 'utf8').replace('Show available commands', 'Updated native description'));
  drift = spawnSync(process.execPath, [generator, '--check'], { encoding: 'utf8' });
  assert.equal(drift.status, 1); assert.match(drift.stderr, /commands.ts/);
  run(process.execPath, [generator]);
  run(process.execPath, [generator, '--check']);
} finally { rmSync(sandbox, { recursive: true, force: true }); }

if (process.argv.includes('--native')) {
  const root = new URL('../../../../../../', import.meta.url);
  const source = fileURLToPath(new URL('neoism-frontend/shared/src/editor/crdt/presence_avatar.rs', root));
  const controller = fileURLToPath(new URL('neoism-frontend/shared/src/panels/agent_pane/command_controller.rs', root));
  const dir = mkdtempSync(join(tmpdir(), 'neoism-parity-'));
  try {
    const harness = join(dir, 'parity.rs');
    // Include the actual native files, not an independently duplicated reference implementation.
    writeFileSync(harness, `#![allow(dead_code)]
mod cursor_style { pub fn rainbow_now_seconds() -> f32 { 0.6 } }
#[path = ${JSON.stringify(source)}] mod avatar;
mod state { pub mod picker {
  pub struct NeoismAgentPickerOption { pub title: String, pub description: String, pub footer: String, pub value: String }
  impl NeoismAgentPickerOption {
    pub fn new(t: &str, d: &str, f: &str, v: &str) -> Self {
      Self { title: t.into(), description: d.into(), footer: f.into(), value: v.into() }
    }
  }
} }
#[path = ${JSON.stringify(controller)}] mod commands;
fn main() {
  for (canonical, alias) in [${commands.flatMap(c => c.aliases.map(a => `(${JSON.stringify(c.name)}, ${JSON.stringify(a)})`)).join(',')}] {
    for suffix in ["", " test-argument"] {
      assert_eq!(commands::plan_slash_command(&format!("{canonical}{suffix}")),
        commands::plan_slash_command(&format!("{alias}{suffix}")));
    }
  }
  for seed in [${seeds.map(s => JSON.stringify(s)).join(',')}] {
    for phase in [0.0_f32, 0.6, 12.5, -1.0] {
      let p = avatar::AvatarProfile::from_seed(seed);
      println!("grid {}", p.grid());
      p.cells(0.0, 0.0, p.grid() as f32, phase, |c| {
        println!("{} {} {:02x}{:02x}{:02x}", c.rect[0], c.rect[1],
          (c.color[0]*255.0).round() as u8, (c.color[1]*255.0).round() as u8, (c.color[2]*255.0).round() as u8);
      });
    }
  }
}
`);
    const binary = join(dir, 'parity');
    run('rustc', ['--edition=2021', harness, '-o', binary]);
    const lines = run(binary, []).trim().split('\n');
    let index = 0, checked = 0;
    for (const seed of seeds) for (const phase of phases) {
      assert.equal(lines[index++], `grid ${avatarGridSize(seed)}`);
      for (const cell of avatarCells(seed, phase)) {
        const [x, y, rgb] = lines[index++].split(' ');
        assert.equal(+x, cell.x); assert.equal(+y, cell.y);
        // libm sine implementations can differ at the last bit; permit one 8-bit color step.
        for (let channel = 0; channel < 3; channel++) {
          const native = parseInt(rgb.slice(channel * 2, channel * 2 + 2), 16);
          const js = parseInt(cell.color.slice(1 + channel * 2, 3 + channel * 2), 16);
          assert(Math.abs(native - js) <= 1, `${seed} ${phase} (${x},${y}): ${rgb} != ${cell.color}`);
        }
        checked++;
      }
    }
    assert.equal(index, lines.length);
    run('rustc', ['--edition=2021', '--test', harness, '-o', binary]);
    console.log(run(binary, []));
    console.log(`Compared ${checked} avatar cells against native Rust, including UTF-16 surrogate pairs.`);
  } finally { rmSync(dir, { recursive: true, force: true }); }
}
console.log('Parity asset tests passed.');
