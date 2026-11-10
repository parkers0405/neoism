#!/usr/bin/env python3
"""Check tab drag/chords and wordmark hover in a bounded, isolated native Rune window."""
import argparse, fcntl, json, os, shlex, signal, struct, subprocess, time
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--binary',default='/tmp/neoism-rune-target/release/neoism-rune')
p.add_argument('--output',default='/tmp/neoism-rune-corrections/native-tabs')
a=p.parse_args(); out=Path(a.output).absolute(); out.parent.mkdir(parents=True,exist_ok=True)
trace=out.with_suffix('.jsonl'); trace.unlink(missing_ok=True)
def active(): return json.loads(subprocess.check_output(['hyprctl','-j','activewindow']))
def dispatch(value): subprocess.run(['hyprctl','dispatch',value],check=True,capture_output=True)
previous=active(); window=None; fd=None
command=shlex.join(['env','NEOISM_RUNE_OFFLINE=1','NEOISM_REDUCED_MOTION=0','timeout','30',a.binary,'--scene','tab-reorder','--native','--trace',str(trace)])+' > '+shlex.quote(str(out.with_suffix('.log')))+' 2>&1'
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
def move(x,y):
    guard(); window.update(active()); dispatch(f'hl.dsp.cursor.move({{ x = {window["at"][0]+x}, y = {window["at"][1]+y} }})')
    # Hyprland cursor warps alone do not deliver physical pointer motion.
    if fd is not None:
        for value in (1,-1):
            for kind,code,v in [(2,0,value),(0,0,0)]: os.write(fd,struct.pack('llHHi',0,0,kind,code,v))
def button(value):
    guard()
    for kind,code,v in [(1,0x110,value),(0,0,0)]: os.write(fd,struct.pack('llHHi',0,0,kind,code,v))
def drag(source,target):
    x,y,w,h=tree()[source]; tx,ty,tw,th=tree()[target]
    # Wait for native hover delivery before pressing. A compositor warp by itself
    # does not guarantee that the freshly registered uinput device has reached Rune.
    hovered = 'pane-close:'+source.split(':',1)[1] if source.startswith('pane-tab:') else 'workspace-close:'
    for _ in range(10):
        move(x+w/2,y+h/2); time.sleep(.1)
        if any(k == hovered or (hovered == 'workspace-close:' and k.startswith(hovered)) for k in tree()): break
    else: raise AssertionError(f'Native pointer did not hover {source}; no drag sent')
    button(1); time.sleep(.1)
    for i in range(1,9): move(x+w/2+(tx+tw*.85-x-w/2)*i/8,y+h/2); time.sleep(.035)
    button(0); settle()
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
    fd=os.open('/dev/uinput',os.O_WRONLY|os.O_NONBLOCK)
    fcntl.ioctl(fd,0x40045564,1); fcntl.ioctl(fd,0x40045565,0x110)
    fcntl.ioctl(fd,0x40045564,2)
    for rel in (0,1): fcntl.ioctl(fd,0x40045566,rel)
    fcntl.ioctl(fd,0x405c5503,struct.pack('HHHH80sI',3,0x1234,0x5679,1,b'neoism-rune-tab-test',0)); fcntl.ioctl(fd,0x5501)
    settle(); move(5,5); settle()
    tabs=ordered('pane-tab:'); spaces=ordered('workspace-tab:')
    assert tabs==['pane-tab:1','pane-tab:2','pane-tab:3'],tabs
    drag(tabs[0],tabs[-1]); assert ordered('pane-tab:')==[tabs[1],tabs[2],tabs[0]],ordered('pane-tab:')
    chord('alt','shift',key='Left'); assert ordered('pane-tab:')==[tabs[1],tabs[0],tabs[2]]
    drag(spaces[0],spaces[-1]); assert ordered('workspace-tab:')==[spaces[1],spaces[2],spaces[0]]
    chord('ctrl','alt','shift',key='Left'); assert ordered('workspace-tab:')==[spaces[1],spaces[0],spaces[2]]
    move(20,100); settle(); assert not any(k.startswith(('pane-close:','workspace-close:')) for k in tree()), (subprocess.check_output(['hyprctl','cursorpos']).decode(), active()['at'])
    for kind,tab in [('agent','pane-tab:3'),('splash','pane-tab:1')]:
        if tab:
            x,y,w,h=tree()[tab]; move(x+w/2,y+h/2); button(1); button(0); settle(); move(5,5); settle()
        t=tree(); hit=next(k for k in t if k.startswith(kind+'-wordmark:') and k.endswith('-hit-0')); letter=hit.replace('-hit-0','-letter-0')
        before=t[letter]; x,y,w,h=t[hit]; move(x+w/2,y+h/2); time.sleep(.6)
        assert tree()[letter][2]>before[2]*1.12,(kind,before,tree()[letter])
    guard(); x,y=window['at']; w,h=window['size']
    subprocess.run(['grim','-g',f'{x},{y} {w}x{h}',str(out.with_suffix('.png'))],check=True,capture_output=True)
    print('PASS native pointer reorder, reorder chords, hover-only close buttons, agent and Home hover',flush=True)
finally:
    if fd is not None:
        fcntl.ioctl(fd,0x5502); os.close(fd)
    if window:
        owned=active().get('pid')==window['pid']
        try:
            if str(trace).encode() in Path(f'/proc/{window["pid"]}/cmdline').read_bytes(): os.kill(window['pid'],signal.SIGTERM)
        except FileNotFoundError: pass
        if owned and previous.get('address'): dispatch(f'hl.dsp.focus({{ window = "address:{previous["address"]}" }})')
