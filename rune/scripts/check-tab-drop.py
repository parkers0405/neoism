#!/usr/bin/env python3
"""Assert GPUiX's drag-a-tab-to-split in Rune: a tab held over a pane edge previews the split,
release commits it, the centre and another pane's tab strip adopt the tab, a lone tab cannot
split its own pane, and leaving the zone or cancelling leaves the layout as it was."""
import argparse,json,os,subprocess
from pathlib import Path
from rune_paths import default_binary
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--binary',default=default_binary())
p.add_argument('--output',default='/tmp/neoism-rune-tab-drop')
a=p.parse_args(); out=Path(a.output); out.mkdir(parents=True,exist_ok=True)
W,H=1252,674
def nodes(v):
    if isinstance(v,dict):
        yield v
        for c in v.values():yield from nodes(c)
    elif isinstance(v,list):
        for c in v:yield from nodes(c)
def run(name,at,events=(),scene='tab-reorder'):
    script=out/f'{name}-input.json';script.write_text(json.dumps(events))
    result=subprocess.run([a.binary,'--scene',scene,'--size',f'{W}x{H}','--at',str(at),'--fps','120','--script',str(script),'--snap',str(out/f'{name}.png'),'--tree',str(out/f'{name}.json')],
        env={**os.environ,'NEOISM_RUNE_OFFLINE':'1','NEOISM_REDUCED_MOTION':'0'},capture_output=True,text=True)
    (out/f'{name}.log').write_text(result.stderr);assert result.returncode==0,result.stderr
    return {n['key']:n for n in nodes(json.loads((out/f'{name}.json').read_text())['accessibility']) if 'key'in n}
def panes(tree): return sorted((n['bounds'] for k,n in tree.items() if k.startswith('pane:')),key=lambda b:(b[0],b[1]))
def tabs(tree): return sorted((n for k,n in tree.items() if k.startswith('pane-tab:')),key=lambda n:n['bounds'][0])
def drag(tab,to,up=True,via=()):
    x,y,w,h=tab['bounds']; events=[{'t':.2,'type':'down','x':x+w/2,'y':y+h/2}]
    for i,point in enumerate([*via,to]): events.append({'t':.4+.3*i,'type':'move','x':point[0],'y':point[1]})
    end=.4+.3*len(via)
    if up: events.append({'t':end+.6,'type':'up','x':to[0],'y':to[1]})
    return events,end

idle=run('idle',.15); area=idle['panes']['bounds']; first=tabs(idle)[0]
assert not panes(idle),'one pane draws unframed'
left,top,width,height=area; mid=(left+width/2,top+height/2)
zones={'right':(left+width-40,mid[1]),'left':(left+40,mid[1]),'bottom':(mid[0],top+height-40),'top':(mid[0],top+height*.2)}
for side,point in zones.items():
    events,end=drag(first,point,up=False)
    held=panes(run(f'{side}-held',end+.5,events))
    assert len(held)==2,(side,held)
    a_,b_=held
    if side in('left','right'): assert abs(a_[2]-b_[2])<1 and a_[1]==b_[1] and b_[0]>a_[0]+a_[2],(side,held)
    else: assert abs(a_[3]-b_[3])<1 and a_[0]==b_[0] and b_[1]>a_[1]+a_[3],(side,held)
    events,end=drag(first,point)
    done=run(f'{side}-dropped',end+1.2,events)
    assert panes(done)==held,(side,'release keeps the previewed layout',panes(done),held)
    assert 'tab-drag-preview' not in done
    moved=done[first['key']]
    target=held[0] if side in('left','top') else held[1]
    mx,my=moved['bounds'][0],moved['bounds'][1]
    assert target[0]-1<=mx<target[0]+target[2] and target[1]-1<=my<target[1]+target[3],(side,'the dragged tab sits in the new pane',moved['bounds'],target)
    print('PASS',side,'edge previews then splits',flush=True)

# The centre of its own pane is no drop; leaving a zone restores the layout; Escape cancels.
events,end=drag(first,mid)
assert not panes(run('own-centre',end+1.2,events)),'own centre must not split'
events,end=drag(first,mid,up=False,via=[zones['right']])
assert not panes(run('zone-left',end+.5,events)),'the preview ends when the pointer leaves the zone'
events,end=drag(first,zones['right'],up=False)
cancelled=run('cancelled',end+1.0,events+[{'t':end+.3,'type':'key','name':'Escape'}])
assert not panes(cancelled),'Escape restores the layout'
print('PASS own centre, leaving the zone and Escape leave the layout alone',flush=True)

# Two panes: another pane's centre and its tab strip adopt; a lone tab cannot split its own pane.
split=run('split-idle',.15,scene='splits'); frames=panes(split); assert len(frames)>=2,frames
by_pane={tuple(f):[n for n in tabs(split) if f[0]<=n['bounds'][0]<f[0]+f[2] and f[1]<=n['bounds'][1]<f[1]+f[3]] for f in frames}
source_frame,source_tabs=min(by_pane.items(),key=lambda kv:len(kv[1])); other=next(f for f in frames if tuple(f)!=source_frame)
tab=source_tabs[0]
if len(source_tabs)==1:
    events,end=drag(tab,(source_frame[0]+source_frame[2]/2,source_frame[1]+source_frame[3]-30))
    assert len(panes(run('lone',end+1.2,events,scene='splits')))==len(frames),'a lone tab cannot split its own pane'
for name,point in [('centre',(other[0]+other[2]/2,other[1]+other[3]/2)),('strip',(other[0]+other[2]-60,other[1]+15))]:
    events,end=drag(tab,point)
    done=run(f'adopt-{name}',end+1.2,events,scene='splits')
    assert len(panes(done))==len(frames)-(len(source_tabs)==1),(name,panes(done))
    landed=done[tab['key']]['bounds']; host=[n['bounds'] for n in by_pane[tuple(other)]]; mates=[done[n['key']]['bounds'] for n in by_pane[tuple(other)]]
    assert all(abs(m[1]-landed[1])<1 for m in mates) and landed[0]>min(m[0] for m in mates),(name,'the tab joins the other pane\'s strip',landed,mates)
print('PASS another pane adopts at its centre and tab strip; a lone tab stays put',flush=True)
