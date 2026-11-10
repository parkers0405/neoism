#!/usr/bin/env python3
"""Render reference surfaces and exercise editor input without touching live user files."""
import argparse
import json
import os
import re
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', default='/tmp/neoism-rune-target/release/neoism-rune')
parser.add_argument('--output', default='/tmp/neoism-rune-verify/headless')
args = parser.parse_args()
out = Path(args.output)
out.mkdir(parents=True, exist_ok=True)
env = {**os.environ, 'NEOISM_RUNE_OFFLINE': '1'}

def render(name, scene, events=(), size='1252x674', realtime=False):
    script = out / f'{name}-input.json'
    script.write_text(json.dumps(events))
    run = subprocess.run([args.binary, *(['--realtime'] if realtime else []), '--scene', scene, '--size', size, '--scale', '2', '--at', '2', '--script', str(script), '--snap', str(out / f'{name}.png'), '--tree', str(out / f'{name}.json')], env=env, capture_output=True, text=True)
    (out / f'{name}.log').write_text(run.stderr)
    if run.returncode:
        raise RuntimeError(f'{name}: {run.stderr}')
    tree = json.loads((out / f'{name}.json').read_text())
    names = []
    def walk(node):
        if isinstance(node, dict):
            if isinstance(node.get('name'), str):
                names.append(node['name'])
            for child in node.values():
                walk(child)
        elif isinstance(node, list):
            for child in node:
                walk(child)
    walk(tree.get('accessibility', tree))
    print(f'PASS render {name}')
    return '\n'.join(names)

for scene in ['welcome', 'agent-new', 'slash', 'palette', 'themes', 'backgrounds', 'app-menu', 'settings', 'keybinds', 'sidebar-long', 'editor-long', 'editor-stress', 'editor-splits', 'editor-md', 'code', 'tool-cards-open', 'timeline-report', 'splits', 'lsp', 'lsp-complete', 'database']:
    render(scene, scene)

def key(t, name):
    return {'t': t, 'type': 'key', 'name': name}

def type_text(t, text):
    return {'t': t, 'type': 'type', 'text': text}

text = render('completion-accept', 'lsp-complete', [key(.3, 'Down'), key(.5, 'Enter')])
assert 'items.push' in text, 'Completion acceptance must replace the prefix'
text = render('completion-dismiss', 'lsp-complete', [key(.3, 'Escape')])
assert 'INSERT' in text, 'Escape first dismisses completion without leaving insert mode'
text = render('word-select-delete', 'lsp', [{'t': .3, 'type': 'double-click', 'x': 135, 'y': 96}, type_text(.8, 'd')])
assert 'fn () {' in text, 'Double click must select the whole identifier'
text = render('line-select-delete', 'lsp', [{'t': .3, 'type': 'triple-click', 'x': 135, 'y': 96}, type_text(.95, 'd')])
assert 'fn main() {' not in text, 'Triple click must select the whole line'
text = render('clipboard-paste', 'lsp', [type_text(.3, 'ggi'), {'t': .5, 'type': 'paste', 'text': 'from clipboard\n  exact indent\n'}])
assert 'from clipboard' in text and '  exact indent' in text, 'Paste must preserve multiline indentation'
render('completion-narrow', 'lsp-complete', size='260x420')
render('welcome-narrow', 'welcome', size='260x420')
render('agent-narrow', 'agent-new', size='260x420')
text = render('split-click-edit', 'editor-splits', [{'t': .3, 'type': 'click', 'x': 140, 'y': 95}, type_text(.5, 'Ileft_only '), key(1.0, 'Escape')])
assert 'left_only let left_value = 1;' in text and 'let right_value = 2;' in text, 'Clicking an unfocused editor must edit only that pane'
text = render('rebind-conflict', 'keybinds', [{'t': .3, 'type': 'click', 'key': 'keybind:opencommandpalette'}, {'t': .5, 'type': 'key', 'name': 's', 'alt': True}])
assert 'Already used by Search files' in text, 'Rebinding must preserve fixed shortcuts'
text = render('rebind-palette', 'keybinds', [{'t': .3, 'type': 'click', 'key': 'keybind:opencommandpalette'}, {'t': .5, 'type': 'key', 'name': 'o', 'ctrl': True, 'cmd': True, 'shift': True}, key(.8, 'Escape'), {'t': 1, 'type': 'key', 'name': 'o', 'ctrl': True, 'cmd': True, 'shift': True}])
assert 'neoism: New Tab' in text and 'Ctrl+Shift+O' in text, 'A captured shortcut must immediately dispatch its action'
text = render('drag-edge-scroll', 'editor-long', [{'t': .3, 'type': 'down', 'x': 440, 'y': 110}, {'t': .4, 'type': 'move', 'x': 440, 'y': 750}, {'t': 1.8, 'type': 'up', 'x': 440, 'y': 750}], realtime=True)
positions = re.findall(r'(?m)^\s*(\d+)/(\d+)\s*$', text)
assert 'VISUAL' in text and positions and int(positions[-1][0]) > 25, 'Dragging past the editor edge must extend selection beyond the initial viewport'
print('All headless checks passed.')
