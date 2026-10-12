#!/usr/bin/env python3
"""Real CEF OSR protocol/pixel regression. No mocked browser or testing JS bridge.
NEOISM_CEF_NO_SANDBOX=1 is explicit trusted-fixture opt-in, not set by this test.
"""
import json
import os
from pathlib import Path
import queue
import struct
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT/'target/debug/neoism-cef-runtime'
requests = []
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        requests.append(self.path)
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'denied')
    def log_message(self, format, *args):
        pass
server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
url = f'http://127.0.0.1:{server.server_port}/denied'
html = '''<style>
html,body{margin:0;background:var(--background)}
button,input{position:absolute;left:20px;width:160px;height:40px}
button{top:20px}input{top:80px}
.flag{position:absolute;left:200px;width:40px;height:40px;background:red}
#counter{top:20px}#text{top:80px}#network{top:140px}#motion{top:200px}
#grid{position:absolute;top:260px;display:grid;grid-template-columns:40px 40px;gap:10px}
#grid div{height:30px;background:lime}#grid div+div{background:blue}
svg{position:absolute;left:150px;top:260px}
#scroller{position:absolute;left:260px;top:20px;width:40px;height:100px;overflow:auto}
#scroller div{height:100px;background:red}#scroller div+div{background:cyan}
#alpha{position:absolute;top:310px;left:0;width:30px;height:30px;background:rgba(255,0,0,.5)}
</style><button id="button">Count: 0</button><input id="field">
<div class="flag" id="counter"></div><div class="flag" id="text"></div>
<div class="flag" id="network"></div><div class="flag" id="motion"></div>
<div id="grid"><div></div><div></div></div>
<svg width="40" height="30"><rect width="40" height="30" fill="#ff00ff"/></svg>
<div id="scroller"><div></div><div></div></div><div id="alpha"></div>
<script>
let count=0,ticks=0;
button.onclick=()=>{button.textContent=String(++count);counter.style.background=count===1?'lime':'yellow'};
field.oninput=()=>text.style.background=field.value==='x'?'cyan':'red';
fetch('URL').then(()=>network.style.background='yellow').catch(()=>network.style.background='blue');
const img=new Image();img.src='URL/image';
fetch('file:///etc/passwd').catch(()=>{});
window.open('URL/popup');
function tick(){motion.style.background=(++ticks%20<10)?'magenta':'yellow';requestAnimationFrame(tick)}requestAnimationFrame(tick);
</script>'''.replace('URL', url)
colors = {name: 0 for name in ('background foreground card card_foreground popover popover_foreground muted muted_foreground border input primary primary_foreground accent accent_foreground destructive destructive_foreground success warning info chart_1 chart_2 chart_3 chart_4 chart_5 chart_6').split()}
colors.update(background=0x123456, foreground=0xffffff)
document = dict(key='live',html=html,revision=1,viewport=dict(width=320,height=350,scale=1.0),visible=True,theme='Dark',styles=dict(colors=colors,font_sans='sans-serif',font_mono='monospace',radius=8.0))
messages=queue.Queue()
arrivals=[]
metrics=[]
log=ROOT/'target/live-worker.stderr.log'
with log.open('wb') as stderr:
    child=subprocess.Popen([str(BINARY),'--stdio','--experimental'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=stderr)
    assert child.stdin is not None and child.stdout is not None
    input_pipe,output_pipe=child.stdin,child.stdout
    def exact(n):
        data=bytearray()
        while len(data)<n:
            c=output_pipe.read(n-len(data))
            if not c: raise RuntimeError(f'worker EOF exit={child.poll()}; see {log}')
            data.extend(c)
        return bytes(data)
    def packet():
        kind,n=struct.unpack('<II',exact(8))
        assert n<=64*1024*1024
        return kind,exact(n)
    def read():
        try:
            last=0
            while True:
                kind,data=packet()
                assert kind==1, f'IPC stdout corruption {kind}'
                message=json.loads(data)
                if 'Frame' in message:
                    meta=message['Frame'];kind,pixels=packet()
                    assert kind==2 and meta['bytes']==len(pixels)==meta['stride']*meta['height']
                    assert meta['stride']==meta['width']*4 and meta['sequence']>last
                    last=meta['sequence']
                    frame=(meta,pixels)
                    # EVERY publication must match its transaction; not just eventual pixels.
                    expected={1:(18,52,86,255),2:(101,67,33,255),3:(101,67,33,255),5:(101,67,33,255),6:(255,255,255,255),10:(0,255,0,255),11:(0,0,255,255)}.get(meta['generation'])
                    if expected: assert pixel(frame,5,5)==expected, f'blank/stale publication {meta}, {pixel(frame,5,5)}'
                    if meta['generation'] in (42,43):
                        assert pixel(frame,100,90)==(18,52,86,255), f'stale native popup {meta}'
                    arrivals.append((time.monotonic(),meta))
                    messages.put(frame)
                elif 'ContentHeight' in message:
                    metrics.append((time.monotonic(),message['ContentHeight']))
                elif 'Fatal' in message: raise RuntimeError(message['Fatal'])
                else: messages.put(message)
        except BaseException as e: messages.put(e)
    def pixel(frame,x,y):
        meta,p=frame;i=y*meta['stride']+x*4
        return tuple(p[i:i+4])
    threading.Thread(target=read,daemon=True).start()
    def send(m):
        data=json.dumps(m).encode();input_pipe.write(struct.pack('<II',1,len(data))+data);input_pipe.flush()
    def wait(label,predicate,seconds=15):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            try: m=messages.get(timeout=.1)
            except queue.Empty: continue
            if isinstance(m,BaseException): raise m
            if isinstance(m,tuple) and predicate(m):
                print(f'PASS {label}: sequence={m[0]["sequence"]}',flush=True);return m
        raise AssertionError(f'{label}: timeout; see {log}')
    def click(x,y):
        send({'Input':dict(key='live',input={'PointerMove':dict(x=x,y=y)})})
        for state in ('Down','Up'):
            send({'Input':dict(key='live',input={'PointerButton':dict(x=x,y=y,button='Left',state=state)})})
    def key(key,code,modifiers='',named=False,state=None):
        for phase in ([state] if state else ['Down','Up']):
            send({'Input':dict(key='live',input={'Key':dict(state=phase,key={'Named' if named else 'Character':key},code=code,location='Standard',modifiers=modifiers,repeat=False,is_composing=False)})})
    def move(x,y): send({'Input':dict(key='live',input={'PointerMove':dict(x=x,y=y)})})
    def button(x,y,state,which='Left'): send({'Input':dict(key='live',input={'PointerButton':dict(x=x,y=y,button=which,state=state)})})
    def reconcile(g): send({'Reconcile':dict(document=document,generation=g)})
    try:
        hello=messages.get(timeout=10)
        assert hello=={'Hello':{'version':2}},hello
        reconcile(1)
        wait('first loaded grid/SVG (no white flash)',lambda f:pixel(f,5,5)==(18,52,86,255) and pixel(f,20,275)==(0,255,0,255) and pixel(f,60,275)==(0,0,255,255) and pixel(f,170,275)==(255,0,255,255))
        if sys.platform=='linux':
            descendants={child.pid}
            renderers=[]
            for _ in range(4):
                for p in Path('/proc').glob('[0-9]*'):
                    try:
                        status=dict(line.split(':',1) for line in (p/'status').read_text().splitlines() if ':' in line)
                        if int(status['PPid']) in descendants:
                            descendants.add(int(p.name))
                            try: command=(p/'cmdline').read_bytes()
                            except PermissionError:
                                command=b''
                            names=[]
                            try: names=[t.read_text().strip() for t in (p/'task').glob('*/comm')]
                            except OSError: pass
                            # Linux zygote fork keeps the zygote argv in /proc;
                            # renderer has the Blink compositor thread, unlike Viz GPU.
                            is_renderer=b'--type=renderer' in command or ('Compositor' in names and 'Chrome_ChildIOT' in names)
                            if is_renderer and int(p.name) not in [r[0] for r in renderers]: renderers.append((int(p.name),status,command))
                    except (OSError,ValueError,KeyError): pass
            assert renderers, 'real renderer subprocess missing'
            if os.environ.get('NEOISM_CEF_NO_SANDBOX')!='1':
                for pid,status,command in renderers:
                    assert b'--no-sandbox' not in command, 'unexpected no-sandbox child'
                    assert status['Seccomp'].strip()=='2' and status['NoNewPrivs'].strip()=='1', f'renderer {pid} sandbox not active: {status}'
                print('PASS renderer sandbox: Seccomp=2 NoNewPrivs=1, no --no-sandbox',flush=True)
            else: print('EXPLICIT UNSANDBOXED trusted-fixture test',flush=True)
        wait('private-network fetch denied',lambda f:pixel(f,220,160)==(0,0,255,255))
        click(80,40)
        wait('real click',lambda f:pixel(f,220,40)==(0,255,0,255))
        click(80,100)
        for state in ('Down','Up'):
            send({'Input':dict(key='live',input={'Key':dict(state=state,key={'Character':'x'},code='KeyX',location='Standard',modifiers='',repeat=False,is_composing=False)})})
        wait('real keyboard',lambda f:pixel(f,220,100)==(0,255,255,255))
        wait('rAF phase 1 without parent pump',lambda f:pixel(f,220,220)==(255,0,255,255))
        wait('rAF phase 2 without parent pump',lambda f:pixel(f,220,220)==(255,255,0,255))
        send({'Input':dict(key='live',input={'Wheel':dict(x=275,y=60,delta_x=0,delta_y=110,unit='Pixel')})})
        wait('real scroll',lambda f:pixel(f,270,30)==(0,255,255,255))
        preserved=lambda f:pixel(f,220,40)==(0,255,0,255) and pixel(f,220,100)==(0,255,255,255)
        document['theme']='Light';colors['background']=0x654321;reconcile(2)
        wait('live theme preserves DOM',lambda f:f[0]['generation']==2 and preserved(f))
        send({'Resize':dict(key='live',viewport=dict(width=400,height=380,scale=1.0),generation=3)})
        wait('resize preserves DOM',lambda f:f[0]['generation']==3 and f[0]['width']==400 and f[0]['height']==380 and preserved(f))
        document['viewport']=dict(width=400,height=380,scale=1.0)
        document['visible']=False;reconcile(4)
        time.sleep(.4)
        start=time.monotonic();time.sleep(.5)
        assert not [m for t,m in arrivals if t>=start], 'hidden frames published'
        document['visible']=True;reconcile(5)
        wait('visibility resume preserves DOM',lambda f:f[0]['generation']==5 and preserved(f))
        colors['background']=0xffffff;reconcile(6)
        wait('legitimate white is not filtered',lambda f:f[0]['generation']==6 and preserved(f))
        # Replace incarnation while the old renderer is still animating.
        document.update(revision=2,html='<style>html,body{margin:0;background:lime}</style>')
        reconcile(10)
        wait('revision replacement never publishes prior pixels',lambda f:f[0]['generation']==10 and f[0]['revision']==2)
        document.update(revision=3,html='<style>html,body{margin:0;background:blue}</style>')
        reconcile(11)
        wait('second replacement',lambda f:f[0]['generation']==11 and f[0]['revision']==3)
        # Fast live-theme transactions: queued callbacks may not relabel old pixels.
        document.update(revision=4,html='<style>html,body{margin:0;background:var(--background)}</style>',visible=False)
        colors['background']=0x123456;reconcile(20)
        colors['background']=0x654321;document['visible']=True;reconcile(2)
        wait('theme replaced during initial hidden load',lambda f:f[0]['revision']==4 and f[0]['generation']==2)
        for cycle in range(10):
            colors['background']=0x123456;reconcile(1)
            colors['background']=0x654321;reconcile(2)
            wait(f'rapid theme pair {cycle}',lambda f:f[0]['revision']==4 and f[0]['generation']==2)
        send({'Resize':dict(key='live',viewport=dict(width=401,height=381,scale=1.25),generation=3)})
        wait('fractional scale exact physical bounds',lambda f:f[0]['generation']==3 and f[0]['width']==401 and f[0]['height']==381)
        document.update(revision=5,html='<style>html,body{margin:0;background:transparent}div{width:40px;height:40px;background:rgba(255,0,0,.5)}</style><div></div>',viewport=dict(width=320,height=350,scale=1.0))
        reconcile(30)
        wait('real premultiplied alpha',lambda f:f[0]['generation']==30 and pixel(f,10,10) in [(128,0,0,128),(127,0,0,127)] and pixel(f,60,60)==(0,0,0,0))
        document.update(revision=6,html='''<style>
html,body{margin:0;background:#123456}select{position:absolute;left:20px;top:20px;width:170px;height:32px;font:20px sans-serif;color:black;background:white}
option{font:20px sans-serif;color:black}option:nth-child(1){background:orange}option:nth-child(2){background:lime}option:nth-child(3){background:magenta}
input[type=range]{position:absolute;left:20px;top:130px;width:170px;height:30px;margin:0}
textarea{position:absolute;left:20px;top:190px;width:160px;height:80px}
.flag{position:absolute;left:220px;width:30px;height:30px;background:red}
#selection{top:20px}#drag{top:130px}#mods{top:165px}#typing{top:200px}#newline{top:240px}#shortcut{top:280px}
</style><select id="choices"><option value="a">Alpha</option><option value="b">Bravo</option><option value="c">Charlie</option></select>
<input type="range" id="slider" min="0" max="100" value="0"><textarea id="editor"></textarea>
<div class="flag" id="selection"></div><div class="flag" id="drag"></div><div class="flag" id="mods"></div><div class="flag" id="typing"></div><div class="flag" id="newline"></div><div class="flag" id="shortcut"></div>
<script>
choices.onchange=()=>selection.style.background=choices.value==='b'?'lime':'red';
slider.oninput=()=>drag.style.background=+slider.value>80?'cyan':'red';
slider.onpointermove=e=>{if(e.buttons===1&&e.ctrlKey)mods.style.background='yellow'};
editor.onkeydown=e=>{if(e.metaKey&&e.key==='a')requestAnimationFrame(()=>requestAnimationFrame(()=>shortcut.style.background=editor.value==='z'?'lime':'red'))};
editor.oninput=()=>{typing.style.background=editor.value==='z'?'lime':'red';newline.style.background=editor.value==='z\\n!'?'cyan':'red'};
</script>''')
        reconcile(40)
        base=wait('interactive controls load',lambda f:f[0]['generation']==40 and pixel(f,5,5)==(18,52,86,255))
        popup_green=lambda f:[y for y in range(35,130) if pixel(f,100,y)==(0,255,0,255)]
        popup_open=lambda f:any(pixel(f,100,y)==(255,0,255,255) for y in range(35,130))
        click(100,35)
        popup=wait('native SELECT POPUP composited',lambda f:f[0]['generation']==40 and len(popup_green(f))>5)
        ys=popup_green(popup);click(100,ys[len(ys)//2])
        wait('native SELECT mouse chooses and restores base',lambda f:pixel(f,235,35)==(0,255,0,255) and pixel(f,100,90)==(18,52,86,255))
        click(100,35)
        wait('native SELECT reopen',lambda f:popup_open(f))
        key('Escape','Escape',named=True)
        wait('native SELECT Escape closes without changing choice',lambda f:pixel(f,100,90)==(18,52,86,255) and pixel(f,235,35)==(0,255,0,255))
        # Maintain Control and down-button masks over separate mouse move controls.
        key('Control','ControlLeft','CONTROL',named=True,state='Down')
        move(28,145);button(28,145,'Down')
        for x in (50,90,130,180): move(x,145);time.sleep(.03)
        button(180,145,'Up')
        key('Control','ControlLeft',named=True,state='Up')
        wait('native range real drag with held mouse and Control masks',lambda f:pixel(f,235,145)==(0,255,255,255) and pixel(f,235,180)==(255,255,0,255))
        # Ignored browser-history buttons must not terminate the worker or navigate.
        for which in ('Back','Forward'):button(50,145,'Down',which);button(50,145,'Up',which)
        click(80,220)
        key('hello','KeyH')
        key('a','KeyA','CONTROL')
        key('Control','ControlLeft',named=True,state='Up')
        key('z','KeyZ')
        wait('Ctrl+A replacement (shortcut emits no CHAR)',lambda f:pixel(f,235,215)==(0,255,0,255))
        key('a','KeyA','META')
        key('Meta','MetaLeft',named=True,state='Up')
        wait('Meta shortcut emits no CHAR',lambda f:pixel(f,235,295)==(0,255,0,255))
        key('End','End',named=True)
        key('Enter','Enter',named=True)
        key('!','Digit1','SHIFT')
        key('Shift','ShiftLeft',named=True,state='Up')
        wait('textarea Enter newline and physical punctuation',lambda f:pixel(f,235,255)==(0,255,255,255))
        click(100,35)
        wait('native popup before resize',lambda f:popup_open(f))
        send({'Resize':dict(key='live',viewport=dict(width=330,height=350,scale=1.0),generation=42)})
        wait('resize discards native popup without stale overlay',lambda f:f[0]['generation']==42 and pixel(f,100,90)==(18,52,86,255))
        send({'Resize':dict(key='live',viewport=dict(width=330,height=350,scale=1.0),generation=44)})
        wait('fresh popup interaction generation',lambda f:f[0]['generation']==44)
        click(100,35)
        wait('native popup before revision replacement',lambda f:popup_open(f))
        document.update(revision=7,html='<style>html,body{margin:0;background:#123456}</style>')
        reconcile(43)
        wait('revision replaces popup incarnation',lambda f:f[0]['generation']==43 and pixel(f,100,90)==(18,52,86,255))
        if os.environ.get('NEOISM_CEF_NETWORK_SMOKE') == '1':
            document.update(revision=8,html='''<body style="margin:0;background:red"><script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.8/dist/chart.umd.min.js" onload="document.body.style.background=typeof Chart==='function'?'lime':'blue'" onerror="document.body.style.background='magenta'"></script></body>''')
            reconcile(45)
            wait('allowed pinned CDN script loaded and executed',lambda f:f[0]['generation']==45 and pixel(f,5,5)==(0,255,0,255),seconds=45)
        def metric(g, predicate, seconds=15):
            end=time.monotonic()+seconds
            while time.monotonic()<end:
                found=[m for _,m in metrics if m['generation']==g and predicate(m['height'])]
                if found:
                    print(f'PASS intrinsic metric generation={g}: {found[-1]}',flush=True)
                    return found[-1]['height']
                time.sleep(.02)
            raise AssertionError(f'missing metric {g}: {metrics[-10:]}')
        def source(g, content, width=320, height=320, scale=1.):
            document.update(revision=g,html=content,visible=True,viewport=dict(width=width,height=height,scale=scale))
            reconcile(g)
        source(100,'<div style="height:42px;background:red"></div>')
        metric(100,lambda h:h==42)
        f=wait('short content transparent surround',lambda f:f[0]['generation']==100)
        assert pixel(f,10,100)==(0,0,0,0)
        source(101,'<div data-neoism-artifact-root style="height:900px">tall</div>')
        metric(101,lambda h:h==900)
        wait('tall content initial frame',lambda f:f[0]['generation']==101)
        start=len(metrics)
        send({'Input':dict(key='live',input={'Wheel':dict(x=20,y=20,delta_x=0.,delta_y=150.,unit='Pixel')})})
        time.sleep(.4)
        assert len(metrics)==start,'scroll changed intrinsic metric'
        source(102,'<div style="font:20px/24px sans-serif">'+('wrapping text '*30)+'</div>',width=400)
        wide=metric(102,lambda h:h>0)
        send({'Resize':dict(key='live',viewport=dict(width=180,height=320,scale=1.),generation=103)})
        metric(103,lambda h:h>wide)
        source(104,'<div id="grow" style="height:40px"></div><script>setTimeout(()=>grow.style.height="170px",600)</script>')
        metric(104,lambda h:h==40)
        metric(104,lambda h:h==170)
        source(105,'<div style="height:25px"></div>')
        metric(105,lambda h:h==25)
        source(106,'<style>body{min-height:100vh;padding-bottom:10px}section{height:100vh}</style><section></section>')
        metric(106,lambda h:h==640)
        for g,h in [(107,640),(108,200),(109,640)]:
            send({'Resize':dict(key='live',viewport=dict(width=320,height=h,scale=1.),generation=g)})
            metric(g,lambda h:h==640)
        source(110,'<div style="height:37.5px"></div>',width=401,height=320,scale=1.25)
        metric(110,lambda h:abs(h-37.5)<.1)
        document.update(visible=False);reconcile(111)
        time.sleep(.3)
        assert not [m for _,m in metrics if m['generation']==111],'hidden metric published'
        document.update(visible=True);reconcile(112)
        metric(112,lambda h:abs(h-37.5)<.1)
        source(113,'<style>body{height:48px;padding:4px;margin:3px}</style>')
        metric(113,lambda h:h==62)
        source(114,'<div style="height:24px"></div><div hidden style="height:900px"></div><style>script{display:block;height:999px}</style><script>/* ignored box */</script>')
        metric(114,lambda h:h==24)
        source(115,'<style>.component{height:80px;position:relative}.fill{height:100%;top:50%}.unused{height:100vh}@media(max-width:100px){.component{height:100vh}}</style><div class="component"><div class="fill"></div></div>')
        metric(115,lambda h:h==80)
        source(116,'<style>.component{position:relative;height:70px}.fill{position:absolute;bottom:0;height:100%;max-height:100%}</style><div class="component"><div class="fill"></div></div>')
        metric(116,lambda h:h==70)
        source(117,'<style>body{min-height:100vh;background:#111;padding-bottom:8px}</style><div style="height:42px"></div>')
        metric(117,lambda h:h==50)
        send({'Resize':dict(key='live',viewport=dict(width=320,height=50,scale=1.),generation=118)})
        metric(118,lambda h:h==50)
        # Static pages publish neither duplicate metrics nor continuous paint.
        wait('static geometry final frame',lambda f:f[0]['generation']==118)
        time.sleep(.2);start=time.monotonic();time.sleep(.4)
        assert not [m for t,m in metrics if t>=start], 'static page repeated metrics'
        send({'Destroy':dict(key='live')})
        time.sleep(.2);start=time.monotonic();time.sleep(.3)
        assert not [m for t,m in arrivals if t>=start], 'destroyed frames published'
        assert not requests, f'private-network requests escaped: {requests}'
        send('Shutdown');input_pipe.close();child.wait(timeout=15)
        assert child.returncode==0, f'shutdown exit {child.returncode}; {log}'
        print('PASS clean shutdown / bounded IPC / no private-network or popup requests',flush=True)
    finally:
        if child.poll() is None:
            child.kill();child.wait()
        server.shutdown()
