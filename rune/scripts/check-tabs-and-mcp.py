#!/usr/bin/env python3
"""Exercise actual Rune pointer drags, reorder chords, and supplied MCP artwork."""
import argparse
import json
import os
from pathlib import Path
import subprocess
from PIL import Image

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', default='/tmp/neoism-rune-target/release/neoism-rune')
parser.add_argument('--output', default='/tmp/neoism-rune-tabs-mcp')
args = parser.parse_args()
out = Path(args.output); out.mkdir(parents=True, exist_ok=True)

def nodes(value):
    if isinstance(value, dict):
        yield value
        for child in value.values(): yield from nodes(child)
    elif isinstance(value, list):
        for child in value: yield from nodes(child)

def render(name, scene, events=()):
    script = out / f'{name}-input.json'; script.write_text(json.dumps(events))
    p = subprocess.run([args.binary, '--scene', scene, '--size', '1252x674', '--at', '2', '--script', str(script), '--snap', str(out/f'{name}.png'), '--tree', str(out/f'{name}.json')], env={**os.environ, 'NEOISM_RUNE_OFFLINE':'1'}, capture_output=True, text=True)
    (out/f'{name}.log').write_text(p.stderr)
    assert p.returncode == 0, p.stderr
    return {n['key']:n for n in nodes(json.loads((out/f'{name}.json').read_text())['accessibility']) if 'key' in n}

def slots(tree, prefix):
    return sorted((n for k,n in tree.items() if k.startswith(prefix)), key=lambda n:n['bounds'][0])
def names(tree, prefix):
    return [next((x['name'] for x in n.get('children',[]) if x.get('role')=='label' and x.get('name') in ('First','Second','Third')), n.get('name','')) for n in slots(tree,prefix)]
def drag(source, target, after=True, leave=False):
    x,y,w,h=source['bounds']; tx,ty,tw,th=target['bounds']; sx=x+w/2; sy=y+h/2
    dx=tx+tw*(.85 if after else .15)
    return [{'t':.3,'type':'down','x':sx,'y':sy}, {'t':.6,'type':'move','x':dx,'y':sy if not leave else sy+100}, {'t':1,'type':'up','x':dx,'y':sy if not leave else sy+100}]
def key(t,name,**mods):return {'t':t,'type':'key','name':name,**mods}

idle=render('tabs-idle','tab-reorder')
panes=slots(idle,'pane-tab:'); workspaces=slots(idle,'workspace-tab:')
assert [n['key'] for n in panes]==['pane-tab:1','pane-tab:2','pane-tab:3']
x,y,w,h=panes[0]['bounds']
closed=render('inactive-close','tab-reorder',[{'t':.3,'type':'move','x':x+w/2,'y':y+h/2},{'t':.6,'type':'click','key':'pane-close:1'}])
assert any(k.startswith('agent-wordmark:3') for k in closed) and 'pane-tab:1' not in closed, 'Closing an inactive tab must preserve the active content'
moved=render('buffer-drag','tab-reorder',drag(panes[0],panes[-1]))
assert [n['key'] for n in slots(moved,'pane-tab:')]==['pane-tab:2','pane-tab:3','pane-tab:1'], 'Pointer drag must reorder buffer tabs'
assert any(k.startswith('splash-wordmark:1') for k in moved), 'Pressing a tab to drag must activate it, matching the reference'
left=render('buffer-key-left','tab-reorder',[key(.3,'Left',shift=True,alt=True)])
assert [n['key'] for n in slots(left,'pane-tab:')]==['pane-tab:1','pane-tab:3','pane-tab:2']
right=render('buffer-key-right','tab-reorder',[key(.3,'Left',shift=True,alt=True),key(.6,'Right',shift=True,alt=True)])
assert [n['key'] for n in slots(right,'pane-tab:')]==['pane-tab:1','pane-tab:2','pane-tab:3']
cancel=render('buffer-outside-drop','tab-reorder',drag(panes[0],panes[-1],leave=True))
assert [n['key'] for n in slots(cancel,'pane-tab:')]==['pane-tab:1','pane-tab:2','pane-tab:3']
moved=render('workspace-drag','tab-reorder',drag(workspaces[0],workspaces[-1]))
assert names(moved,'workspace-tab:')==['Second','Third','First'], names(moved,'workspace-tab:')
assert 'pane-tab:3' in moved, 'Workspace reorder must retain its active content'
keyboard=render('workspace-key','tab-reorder',[key(.3,'Right',ctrl=True,shift=True,alt=True)])
assert names(keyboard,'workspace-tab:')==['Second','First','Third']
# Alt+Up walks through buffer and workspace focus. Shift+Alt then targets that strip.
keyboard=render('workspace-focused-key','tab-reorder',[key(.3,'Up',alt=True),key(.6,'Up',alt=True),key(.9,'Right',shift=True,alt=True)])
assert names(keyboard,'workspace-tab:')==['Second','First','Third']
overflow=render('tab-overflow', 'tab-overflow')
# The last selected buffer must be fully revealed; Alt+9 must reveal its workspace.
last=overflow['pane-tab:17']['bounds']; assert last[0]>=55 and last[0]+last[2]<=1253,last
shifted=render('workspace-overflow', 'tab-overflow', [key(.5,'9',alt=True)])
active=next(n for n in slots(shifted,'workspace-tab:') if 'Workspace 7' in n.get('name',''))
x,y,w,h=active['bounds']; assert x>=55 and x+w<=1253,active
first=render('buffer-overflow-wrap', 'tab-overflow', [key(.5,'Right',ctrl=True,shift=True)])
x,y,w,h=first['pane-tab:1']['bounds']; assert x>=55 and x+w<=1253,(x,y,w,h)
print('PASS pointer reorder, outside drop, keyboard reorder, overflow reveal and workspace identity',flush=True)

picker=render('mcp-picker','mcp-icons')
actions=render('mcp-actions','mcp-icons',[key(.4,'Enter')])
timeline=render('mcp-timeline','mcp-timeline')
assert len([k for k in picker if k.startswith('picker-row-icon:')])==5
assert 'mcp-header-icon' in actions
assert 'mcp-tool-icon:custom-mcp-call' in timeline
# Verify custom server pixels, not just an empty image slot or a brand fallback.
for name,tree,keyname in [('mcp-picker',picker,'picker-row-icon:0'),('mcp-actions',actions,'mcp-header-icon'),('mcp-timeline',timeline,'mcp-tool-icon:custom-mcp-call')]:
    x,y,w,h=tree[keyname]['bounds']; im=Image.open(out/f'{name}.png').convert('RGB')
    sx=im.width/1252; sy=im.height/674
    crop=im.crop((int(x*sx),int(y*sy),int((x+w)*sx),int((y+h)*sy)))
    colors=list(crop.get_flattened_data())
    assert any(g>150 and b>100 and r<80 for r,g,b in colors), f'{name}: missing supplied teal pixels'
    assert any(r>180 and g<130 and b<100 for r,g,b in colors), f'{name}: missing supplied orange pixels'
print('PASS supplied MCP artwork in picker, actions header and timeline',flush=True)
