#!/usr/bin/env python3
"""Check exact tab/workspace close commands in a bounded, isolated native Rune window."""
import argparse, json, os, shlex, signal, subprocess, time
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--connected',action='store_true',help='Use the local daemon for workspace close acknowledgments')
p.add_argument('--binary',default='/tmp/neoism-rune-target/release/neoism-rune')
p.add_argument('--output',default='/tmp/neoism-rune-corrections/native-close')
a=p.parse_args(); out=Path(a.output).absolute(); out.parent.mkdir(parents=True,exist_ok=True)
trace=out.with_suffix('.jsonl'); trace.unlink(missing_ok=True)
def active(): return json.loads(subprocess.check_output(['hyprctl','-j','activewindow']))
def dispatch(value): subprocess.run(['hyprctl','dispatch',value],check=True,capture_output=True)
previous=active(); window=None
command=shlex.join(['env',*(['-u','NEOISM_RUNE_OFFLINE'] if a.connected else ['NEOISM_RUNE_OFFLINE=1']),'NEOISM_REDUCED_MOTION=0','timeout','30',a.binary,'--scene','tab-reorder','--native','--trace',str(trace)])+' > '+shlex.quote(str(out.with_suffix('.log')))+' 2>&1'
dispatch(f'hl.dsp.exec_cmd({json.dumps(command)}, {{ workspace = "5" }})')
def guard():
    if active().get('pid')!=window['pid']: raise RuntimeError('Focus changed; input stopped')
def tree():
    metadata={}; state={}
    if not trace.exists(): return {}
    for line in trace.read_text().splitlines():
        try: frame=json.loads(line)
        except json.JSONDecodeError: continue
        for m in frame.get('m',[]): metadata[m['id']]=m['nm'].rsplit('/',1)[-1]
        for e in frame.get('e',[]): state[e[0]]=e[1:5]
        for gone in frame.get('g',[]): state.pop(gone,None)
    return {metadata[i]:bounds for i,bounds in state.items() if i in metadata}
def ordered(prefix): return [k for k,b in sorted(tree().items(),key=lambda pair:pair[1][0]) if k.startswith(prefix)]
def settle(): time.sleep(.35)
def chord(*mods,key):
    guard(); args=[]
    for m in mods: args+=['-M',m]
    args+=['-k',key]
    for m in reversed(mods): args+=['-m',m]
    subprocess.run(['wtype',*args],check=True); settle()
try:
    for _ in range(80):
        candidate=active()
        try: cmd=Path(f'/proc/{candidate["pid"]}/cmdline').read_bytes()
        except (KeyError,FileNotFoundError): cmd=b''
        if str(trace).encode() in cmd: window=candidate; break
        time.sleep(.1)
    assert window,'Native test window did not focus; no input sent'
    target=json.dumps('address:'+window['address'])
    dispatch(f'hl.dsp.window.float({{ action = "on", window = {target} }})')
    dispatch(f'hl.dsp.window.resize({{ x = 1252, y = 674, relative = false, window = {target} }})')
    settle(); window=active(); guard()
    time.sleep(.8)
    def alive():
        guard()
        assert Path(f'/proc/{window["pid"]}').exists(), 'Window exited unexpectedly'
    def palette(command):
        chord('alt',key='p'); guard()
        subprocess.run(['wtype',command],check=True); settle()
        subprocess.run(['wtype','-k','Return'],check=True); settle()
    def tab_count(): return len(ordered('pane-tab:'))
    # Every command is delivered through native Wayland keyboard input.
    chord('alt',key='a'); before=tab_count()
    guard(); subprocess.run(['wtype','q','-k','Return'],check=True); settle()
    alive(); assert tab_count()==before-1
    chord('alt',key='a'); before=tab_count()
    palette('q'); alive(); assert tab_count()==before-1
    print('PASS native Agent q and palette q close one tab; window stays alive',flush=True)
    # Close clean editor, Agent and Home tabs, then q on the last Home leaves a Home.
    for _ in range(4):
        palette('q'); alive()
    assert tab_count()==1
    assert len(ordered('workspace-tab:'))==3
    print('PASS native palette q on last tab preserves workspace/window',flush=True)
    for remaining in (2,1):
        palette('qall'); alive()
        assert len(ordered('workspace-tab:'))==remaining
        assert tab_count()>=1
    print('PASS native qall selects a remaining workspace',flush=True)
    palette('qall')
    time.sleep(.4)
    assert not Path(f'/proc/{window["pid"]}').exists(), 'Final qall must close the native window'
    print('PASS native final qall closes the window',flush=True)

finally:
    if window:
        owned=active().get('pid')==window['pid']
        try:
            if str(trace).encode() in Path(f'/proc/{window["pid"]}/cmdline').read_bytes(): os.kill(window['pid'],signal.SIGTERM)
        except FileNotFoundError: pass
        if (owned or not active().get('address')) and previous.get('address'): dispatch(f'hl.dsp.focus({{ window = "address:{previous["address"]}" }})')
