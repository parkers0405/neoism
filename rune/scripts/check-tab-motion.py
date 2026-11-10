#!/usr/bin/env python3
"""Assert GPUiX-style live tab slides and pointer-following ghosts in Rune frame traces."""
import argparse,json,os,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--binary',default='/tmp/neoism-rune-target/release/neoism-rune')
p.add_argument('--output',default='/tmp/neoism-rune-tab-motion')
a=p.parse_args(); out=Path(a.output); out.mkdir(parents=True,exist_ok=True)
def nodes(v):
    if isinstance(v,dict):
        yield v
        for c in v.values():yield from nodes(c)
    elif isinstance(v,list):
        for c in v:yield from nodes(c)
def run(name,at,events=()):
    script=out/f'{name}-input.json';script.write_text(json.dumps(events))
    result=subprocess.run([a.binary,'--scene','tab-reorder','--size','1252x674','--at',str(at),'--fps','120','--script',str(script),'--snap',str(out/f'{name}.png'),'--tree',str(out/f'{name}.json'),'--trace',str(out/f'{name}.jsonl')],env={**os.environ,'NEOISM_RUNE_OFFLINE':'1','NEOISM_REDUCED_MOTION':'0'},capture_output=True,text=True)
    (out/f'{name}.log').write_text(result.stderr);assert result.returncode==0,result.stderr
    return {n['key']:n for n in nodes(json.loads((out/f'{name}.json').read_text())['accessibility']) if 'key'in n}
def frames(name):
    meta={};state={}
    for line in (out/f'{name}.jsonl').read_text().splitlines():
        f=json.loads(line)
        for m in f.get('m',[]):meta[m['id']]=m['nm'].rsplit('/',1)[-1]
        for e in f.get('e',[]):state[e[0]]=e
        for gone in f.get('g',[]):state.pop(gone,None)
        if 't'in f:yield f['t'],{meta[k]:e for k,e in state.items() if k in meta}
idle=run('idle',.15)
for kind,prefix in [('buffer','pane-tab:'),('workspace','workspace-tab:')]:
    tabs=sorted((n for k,n in idle.items() if k.startswith(prefix)),key=lambda n:n['bounds'][0])
    x,y,w,h=tabs[0]['bounds']; target=tabs[-1]['bounds']; dx=target[0]+target[2]*.85
    events=[{'t':.2,'type':'down','x':x+w/2,'y':y+h/2},{'t':.4,'type':'move','x':dx,'y':y+h/2}]
    held=run(kind+'-held',.9,events)
    assert 'tab-drag-preview' in held,'Ghost must remain attached throughout the hold'
    assert 'tab-drop-marker' not in held,'GPUiX has no insertion line'
    ghost=held['tab-drag-preview']['bounds']; assert abs(ghost[0]-(dx-w/2))<1,(ghost,dx,w)
    # Source identities are generated per process for workspaces. Select the first two by
    # initial positions in the trace, then follow those stable keys through the live reorder.
    trace=list(frames(kind+'-held'))
    initial=next(state for t,state in trace if t>=100)
    keys=sorted((k for k in initial if k.startswith(prefix)),key=lambda k:initial[k][1])
    source,neighbor=keys[:2]; start=initial[neighbor][1]; end=initial[source][1]
    motion=[state[neighbor][1] for t,state in trace if 400<t<800 and neighbor in state]
    assert any(end+1<value<start-1 for value in motion),f'{kind}: neighbors must slide while held, not jump on release'
    assert min(motion)>=end-.6 and max(motion)<=start+.6, f'{kind}: critically damped slide must not bounce'
    assert abs(motion[-1]-end)<1,(kind,motion[-1],end)
    final=trace[-1][1]; assert final[source][5]==0, 'The source copy must disappear while its ghost follows the hand'
    # The canonical order is already changed before button-up; release settles the real tab.
    released=run(kind+'-released',1.8,events+[{'t':1.0,'type':'up','x':dx,'y':y+h/2}])
    assert 'tab-drag-preview' not in released
    print('PASS',kind,'live crossing, neighbor spring, hidden source, plain ghost and release',flush=True)
