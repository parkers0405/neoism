#!/usr/bin/env python3
"""Check wordmark geometry, hover-only close buttons and editor Space+x without building Rust."""
import argparse
import json
import os
from pathlib import Path
from rune_paths import default_binary
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', default=default_binary())
parser.add_argument('--output', default='/tmp/neoism-rune-hover-regressions')
args = parser.parse_args()
out = Path(args.output)
out.mkdir(parents=True, exist_ok=True)
env = {**os.environ, 'NEOISM_RUNE_OFFLINE': '1', 'NEOISM_REDUCED_MOTION': '0'}


def nodes(value):
    if isinstance(value, dict):
        yield value
        for child in value.values():
            yield from nodes(child)
    elif isinstance(value, list):
        for child in value:
            yield from nodes(child)


def render(name, scene, events=()):
    script = out / f'{name}-input.json'
    script.write_text(json.dumps(events))
    result = subprocess.run([args.binary, '--scene', scene, '--size', '1252x674', '--at', '2',
                             '--script', str(script), '--snap', str(out / f'{name}.png'),
                             '--tree', str(out / f'{name}.json')], env=env, capture_output=True, text=True)
    (out / f'{name}.log').write_text(result.stderr)
    if result.returncode:
        raise RuntimeError(result.stderr)
    data = json.loads((out / f'{name}.json').read_text())
    return {n['key']: n for n in nodes(data['accessibility']) if 'key' in n}


def move_to(t, node):
    x, y, w, h = node['bounds']
    return {'t': t, 'type': 'move', 'x': x + w / 2, 'y': y + h / 2}


def wordmark(scene, kind):
    idle = render(f'{scene}-{kind}-idle', scene)
    hit_key = next(key for key in idle if key.startswith(kind + '-wordmark') and key.endswith('-hit-0'))
    letter_key = hit_key.replace('-hit-0', '-letter-0')
    hover = render(f'{scene}-{kind}-hover', scene, [move_to(.4, idle[hit_key])])
    before, after = idle[letter_key]['bounds'], hover[letter_key]['bounds']
    assert after[2] > before[2] * 1.12, f'{kind}: hovering must enlarge the letter, beyond idle shimmer'
    assert after[1] < before[1] - before[3] * .12, f'{kind}: hovering must lift the letter'
    leave = render(f'{scene}-{kind}-leave', scene, [move_to(.4, idle[hit_key]), {'t': 1.2, 'type': 'move', 'x': 5, 'y': 5}])
    assert abs(leave[letter_key]['bounds'][2] - before[2]) < 1.0, 'Letter must ease back after pointer leaves'


def close_buttons():
    idle = render('tabs-idle', 'lsp')
    assert not any(k.startswith(('pane-close:', 'workspace-close:')) for k in idle), 'X must not appear automatically on focused or inactive tabs'
    for key, close in [('pane-tab:1', 'pane-close:1'), ('pane-tab:2', 'pane-close:2'), (next(k for k in idle if k.startswith('workspace-tab:')), 'workspace-close:0')]:
        over = render(key.replace(':', '-') + '-hover', 'lsp', [move_to(.4, idle[key])])
        assert close in over, f'Hover must expose {close}'
        over_key = next(k for k in over if k.startswith('workspace-tab:')) if key.startswith('workspace-tab:') else key
        assert over[over_key]['bounds'] == idle[key]['bounds'], 'Revealing X must not shift tab geometry'
        leave = render(key.replace(':', '-') + '-leave', 'lsp', [move_to(.4, idle[key]), {'t': 1.2, 'type': 'move', 'x': 5, 'y': 5}])
        assert close not in leave, 'X must disappear when hover ends'


def leader():
    tree = render('space-x-clean', 'lsp', [{'t': .3, 'type': 'type', 'text': ' x'}])
    assert 'pane-tab:2' not in tree and 'pane-tab:1' in tree, 'Space+x must close only the code buffer'
    tree = render('space-x-dirty', 'lsp', [{'t': .3, 'type': 'type', 'text': 'Iunsaved '},
                  {'t': .8, 'type': 'key', 'name': 'Escape'}, {'t': 1, 'type': 'type', 'text': ' x'}])
    assert 'modal' in tree and 'pane-tab:2' in tree, 'Space+x must protect unsaved changes'


failures = []
checks = [('Home hover', lambda: wordmark('welcome', 'splash')),
          ('Agent hover unchanged', lambda: wordmark('agent-new', 'agent')),
          ('Unfocused Home hover', lambda: wordmark('splits', 'splash')),
          ('Unfocused agent hover', lambda: wordmark('splits', 'agent')),
          ('Hover-only close buttons', close_buttons), ('Space+x', leader)]
for label, check in checks:
    try:
        check()
        print('PASS', label, flush=True)
    except (AssertionError, RuntimeError, StopIteration) as error:
        failures.append(label)
        print('FAIL', label, str(error), flush=True)
if failures:
    raise SystemExit('Failed: ' + ', '.join(failures))
