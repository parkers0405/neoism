#!/usr/bin/env python3
"""Assert a file keeps its scroll position while another tab shows: scroll a long file, show
Home, come back, and the same lines are on screen."""
import argparse,json,os,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--binary',default='/tmp/neoism-rune-target/release/neoism-rune')
p.add_argument('--output',default='/tmp/neoism-rune-scroll-kept')
a=p.parse_args(); out=Path(a.output); out.mkdir(parents=True,exist_ok=True)
def nodes(v):
    if isinstance(v,dict):
        yield v
        for c in v.values():yield from nodes(c)
    elif isinstance(v,list):
        for c in v:yield from nodes(c)
def run(name,at,events=()):
    script=out/f'{name}-input.json';script.write_text(json.dumps(events))
    result=subprocess.run([a.binary,'--scene','editor-long','--size','1252x674','--at',str(at),'--fps','120','--script',str(script),'--snap',str(out/f'{name}.png'),'--tree',str(out/f'{name}.json')],
        env={**os.environ,'NEOISM_RUNE_OFFLINE':'1','NEOISM_REDUCED_MOTION':'0'},capture_output=True,text=True)
    assert result.returncode==0,result.stderr
    return [n for n in nodes(json.loads((out/f'{name}.json').read_text())['accessibility']) if 'key' in n]
def top_row(tree):
    view=next(n['bounds'] for n in tree if n['key'].startswith('editor-scroll') or n['key'].startswith('code-scroll') or 'code-rows' == n['key'])
    rows=sorted((n for n in tree if n['key'].startswith('visual:') and n['bounds'][1]>=view[1]-1),key=lambda n:n['bounds'][1])
    return int(rows[0]['key'].split(':')[1]) if rows else None
idle=run('idle',.3)
tabs=sorted((n for n in idle if n['key'].startswith('pane-tab:')),key=lambda n:n['bounds'][0])
home,editor=tabs[0]['bounds'],tabs[-1]['bounds']
centre=lambda b:{'x':b[0]+b[2]/2,'y':b[1]+b[3]/2}
wheel=[{'t':.4+i*.05,'type':'wheel','x':700,'y':380,'dy':-240} for i in range(8)]
scrolled=run('scrolled',3.0,wheel)  # long enough for the wheel's glide to settle
before=top_row(scrolled); assert before and before>10,('the file scrolled',before)
away=wheel+[{'t':1.7,'type':'click',**centre(home)}]
back=away+[{'t':2.3,'type':'click',**centre(editor)}]
returned=run('returned',3.2,back)
after=top_row(returned)
assert after is not None and abs(after-before)<=1,(f'back on the file at row {after}, left it at row {before}')
print(f'PASS the file is back at row {after}, where it was left (row {before})')
