#!/usr/bin/env python3
"""Assert multiplayer presence in Rune's headless `presence` scene (six staged people): the
rail lists this user and three others then "+3", with the hovered person's name beside it; a
file's tree row carries the people in it (three orbs then "+2"); the editor draws each
person's caret and name flag on their line, and a selection across the lines it covers."""
import argparse,json,os,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--binary',default='/tmp/neoism-rune-target/release/neoism-rune')
p.add_argument('--output',default='/tmp/neoism-rune-presence')
a=p.parse_args(); out=Path(a.output); out.mkdir(parents=True,exist_ok=True)
def nodes(v):
    if isinstance(v,dict):
        yield v
        for c in v.values():yield from nodes(c)
    elif isinstance(v,list):
        for c in v:yield from nodes(c)
script=out/'input.json';script.write_text('[]')
result=subprocess.run([a.binary,'--scene','presence','--size','1252x674','--at','0.6','--fps','120','--script',str(script),'--snap',str(out/'presence.png'),'--tree',str(out/'presence.json')],
    env={**os.environ,'NEOISM_RUNE_OFFLINE':'1','NEOISM_REDUCED_MOTION':'0'},capture_output=True,text=True)
assert result.returncode==0,result.stderr
found=[n for n in nodes(json.loads((out/'presence.json').read_text())['accessibility']) if 'key' in n]
def keyed(prefix): return [n for n in found if n['key'].startswith(prefix)]
rail=sorted(keyed('presence:'),key=lambda n:n['bounds'][1])
assert len(rail)==4,[n['key'] for n in rail]
assert rail[0]['key'].split(':',1)[1].split('@')[0] not in ('ada','bob','cy','dee','eli','grace'),'this user comes first'
assert all(b['bounds'][1]>a['bounds'][1] and abs(b['bounds'][0]-a['bounds'][0])<1 for a,b in zip(rail,rail[1:])),'one slot each, down the rail'
servers=keyed('open-servers')[0]['bounds']; assert rail[0]['bounds'][1]>=servers[1]+servers[3]-1,'under the rail buttons'
assert keyed('presence-more'),'more than four people shows +N'
tip=keyed('presence-tooltip'); assert len(tip)==1 and tip[0]['bounds'][0]>rail[0]['bounds'][0]+rail[0]['bounds'][2],'the hovered name sits beside the rail'
rows=keyed('tree-presence'); assert len(rows)==2,'two files have people in them'
tree={n['key']:n['bounds'] for n in keyed('tree:')}
for cluster in rows:
    row=next(b for b in tree.values() if b[1]-1<=cluster['bounds'][1]<=b[1]+b[3])
    assert cluster['bounds'][0]+cluster['bounds'][2]<=row[0]+row[2]+1 and cluster['bounds'][0]>row[0]+row[2]/2,'right end of the row'
carets=keyed('peer-caret:'); flags=keyed('peer-flag:')
assert len(carets)==5 and len(flags)==5,([n['key'] for n in carets],[n['key'] for n in flags])
for caret in carets:
    flag=next(f for f in flags if f['key'].split(':')[1]==caret['key'].split(':')[1])
    assert abs(flag['bounds'][0]-caret['bounds'][0])<1 and flag['bounds'][1]<=caret['bounds'][1],('the flag sits over its caret',caret,flag)
beam=next(n for n in carets if n['key']=='peer-caret:Bob')['bounds']; block=next(n for n in carets if n['key']=='peer-caret:Cy')['bounds']
assert beam[2]<=2.01 and block[2]>beam[2]*3,'insert is a beam, normal a block'
bands=keyed('peer-selection:Ada'); assert len(bands)==2 and bands[0]['bounds'][1]!=bands[1]['bounds'][1],'a selection bands each line it crosses'
print('PASS rail, tree rows, carets, flags and selections for six staged people')
