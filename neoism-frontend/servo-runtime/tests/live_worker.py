#!/usr/bin/env python3
"""Real, offscreen Servo stdio regression test. No desktop window or engine mock.
Run after cargo build: python3 neoism-frontend/servo-runtime/tests/live_worker.py
Trusted fixture only: the experimental worker does NOT have an OS sandbox.
"""
import json
import os
import math
from pathlib import Path
import queue
import struct
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / 'target/debug/neoism-servo-runtime'
requests = []
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        requests.append(self.path)
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'network must never reach this server')
    def log_message(self, format, *args):
        pass
server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
url = f'http://127.0.0.1:{server.server_port}/denied'
html = '''<!doctype html><style>
html,body{margin:0;width:100%;height:100%;background:var(--background)}
button,input{position:absolute;left:20px;width:160px;height:40px}
button{top:20px}input{top:80px}
.flag{position:absolute;left:200px;width:40px;height:40px;background:rgb(255,0,0)}
#counter{top:20px}#text{top:80px}#network{top:140px}#animation{top:200px}
</style><button id="button">Count: 0</button><input id="field">
<div class="flag" id="counter"></div><div class="flag" id="text"></div>
<div class="flag" id="network"></div><div class="flag" id="animation"></div>
<script>
let count=0, ticks=0, running=true;
button.onclick=()=>{button.textContent='Count: '+(++count);counter.style.background=count===1?'rgb(0,255,0)':count===2?'rgb(255,255,0)':'rgb(255,0,255)';if(count===2)running=false};
field.oninput=()=>{text.style.background=field.value==='x'?'rgb(0,255,255)':'rgb(255,0,0)'};
fetch('URL').then(()=>network.style.background='rgb(255,255,0)').catch(()=>network.style.background='rgb(0,0,255)');
const image=new Image();image.hidden=true;image.src='URL/image';document.body.appendChild(image);
function tick(){if(!running)return;ticks++;animation.style.background=ticks%20<10?'rgb(255,0,255)':'rgb(255,255,0)';requestAnimationFrame(tick)}requestAnimationFrame(tick);
</script>'''.replace('URL', url)
colors = {name: 0 for name in ('background foreground card card_foreground popover popover_foreground muted muted_foreground border input primary primary_foreground accent accent_foreground destructive destructive_foreground success warning info chart_1 chart_2 chart_3 chart_4 chart_5 chart_6').split()}
colors['background'] = 0x123456
colors['foreground'] = 0xffffff
document = dict(key='live', html=html, revision=1, viewport=dict(width=320, height=280, scale=1.0), visible=True, theme='Dark', styles=dict(colors=colors, font_sans='sans-serif', font_mono='monospace', radius=8.0))
failures = []
messages = queue.Queue()
arrivals = []
baseline = os.environ.get('NEOISM_PERF_BASELINE') == '1'
log = ROOT / 'target/live-worker.stderr.log'
with log.open('wb') as stderr:
    child = subprocess.Popen([str(BINARY), '--stdio', '--experimental'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr)
    assert child.stdin is not None and child.stdout is not None
    input_pipe, output_pipe = child.stdin, child.stdout
    def exact(n):
        data = bytearray()
        while len(data) < n:
            chunk = output_pipe.read(n-len(data))
            if not chunk:
                raise RuntimeError(f'worker EOF (exit={child.poll()}); see {log}')
            data.extend(chunk)
        return bytes(data)
    def packet():
        kind, size = struct.unpack('<II', exact(8))
        assert size <= 32*1024*1024
        return kind, exact(size)
    def read():
        try:
            while True:
                kind, data = packet()
                assert kind == 1, f'protocol stdout corruption: {kind}'
                message = json.loads(data)
                if 'Frame' in message:
                    kind, pixels = packet()
                    meta = message['Frame']
                    assert kind == 2 and len(pixels) == meta['width']*meta['height']*4 == meta['bytes']
                    # Validate every initial/resume publication, not just the
                    # eventual good frame accepted by wait(). Legitimate white
                    # remains separately covered by generation 40.
                    expected = {1:(18,52,86,255), 51:(101,67,33,255), 54:(35,69,103,255)}.get(meta['generation'])
                    if expected is not None:
                        offset = 5*meta['stride']+5*4
                        actual = tuple(pixels[offset:offset+4])
                        if baseline and actual != expected:
                            print(f'BASELINE blank/stale initial/resume publication: generation={meta["generation"]} pixels={actual}', flush=True)
                        else:
                            assert actual == expected, f'blank/stale first or resume frame: {meta}'
                    arrivals.append((time.monotonic(), meta.copy()))
                    messages.put((meta, pixels))
                elif 'Fatal' in message:
                    raise RuntimeError(message['Fatal'])
                else:
                    messages.put(message)
        except BaseException as error:
            messages.put(error)
    threading.Thread(target=read, daemon=True).start()
    def send(message):
        data = json.dumps(message).encode()
        input_pipe.write(struct.pack('<II', 1, len(data))+data)
        input_pipe.flush()
    def pixel(frame, x, y):
        meta, pixels = frame
        start = y*meta['stride']+x*4
        return tuple(pixels[start:start+4])
    def wait(label, predicate, seconds=30):
        deadline = time.monotonic()+seconds
        last = None
        while time.monotonic() < deadline:
            try:
                message = messages.get(timeout=min(0.2, max(0.001, deadline-time.monotonic())))
            except queue.Empty:
                continue
            if isinstance(message, BaseException):
                raise message
            if isinstance(message, tuple):
                last = message
                if predicate(message):
                    print(f'PASS {label}: background={pixel(message,5,5)} sequence={message[0]["sequence"]} {message[0]["width"]}x{message[0]["height"]} RGBA bytes={len(message[1])}', flush=True)
                    return message
        samples = [pixel(last, x, y) for x,y in [(5,5),(220,40),(220,100),(220,160),(220,220)]] if last else None
        raise AssertionError(f'{label}: deadline, last pixels={samples}; stderr={log}')
    def verify(label, predicate):
        try:
            return wait(label, predicate, seconds=5)
        except AssertionError as error:
            failures.append(str(error))
            print(f'FAIL {error}', flush=True)
    def click(x,y):
        send({'Input':dict(key='live', input={'PointerMove':dict(x=x,y=y)})})
        for state in ('Down','Up'):
            send({'Input':dict(key='live', input={'PointerButton':dict(x=x,y=y,button='Left',state=state)})})
    try:
        hello = messages.get(timeout=10)
        assert hello == {'Hello': {'version': 1}}, hello
        send({'Reconcile':dict(document=document,generation=1)})
        wait('actual page pixels', lambda f: pixel(f,5,5)==(18,52,86,255))
        wait('network fetch rejected', lambda f: pixel(f,220,160)==(0,0,255,255))
        click(80,40)
        wait('counter button clicked once', lambda f: pixel(f,220,40)==(0,255,0,255))
        click(80,100)
        for state in ('Down','Up'):
            send({'Input':dict(key='live',input={'Key':dict(state=state,key={'Character':'x'},code='KeyX',location='Standard',modifiers='',repeat=False,is_composing=False)})})
        wait('text input value x', lambda f: pixel(f,220,100)==(0,255,255,255))
        a = wait('rAF yellow phase without parent Pump', lambda f: pixel(f,220,220)==(255,255,0,255))
        b = wait('rAF magenta phase without parent Pump', lambda f: pixel(f,220,220)==(255,0,255,255))
        assert b[0]['sequence'] > a[0]['sequence']
        document['theme'] = 'Light'
        colors['background'] = 0x654321
        send({'Reconcile':dict(document=document,generation=2)})
        preserved = lambda f: pixel(f,220,40)==(0,255,0,255) and pixel(f,220,100)==(0,255,255,255)
        wait('live theme reconciliation preserves counter/text state', lambda f: f[0]['generation']==2 and preserved(f))
        verify('live theme background pixels', lambda f: f[0]['generation']==2 and pixel(f,5,5)==(101,67,33,255) and preserved(f))
        send({'Resize':dict(key='live',viewport=dict(width=400,height=300,scale=1.0),generation=3)})
        wait('resize preserves counter/text state', lambda f: f[0]['generation']==3 and f[0]['width']==400 and f[0]['height']==300 and preserved(f))
        verify('resized live theme background pixels', lambda f: f[0]['generation']==3 and pixel(f,390,290)==(101,67,33,255))
        document['viewport'] = dict(width=400,height=300,scale=1.0)
        document['visible'] = False
        send({'Reconcile':dict(document=document,generation=4)})
        hidden_deadline = time.monotonic()+5
        while True:
            message = messages.get(timeout=max(0.001, hidden_deadline-time.monotonic()))
            if isinstance(message, BaseException):
                raise message
            assert not (isinstance(message, tuple) and message[0]['generation']==4), 'hidden animation published a frame'
            if isinstance(message, dict) and message.get('Status', {}).get('animating') is False:
                break
            assert time.monotonic() < hidden_deadline, 'hidden animation timer did not stop'
        time.sleep(0.3)
        while not messages.empty():
            message = messages.get_nowait()
            if isinstance(message, BaseException):
                raise message
            assert not isinstance(message, tuple), 'hidden worker continued publishing frames'
        document['visible'] = True
        send({'Reconcile':dict(document=document,generation=5)})
        wait('hidden rAF resumes yellow phase', lambda f: f[0]['generation']==5 and pixel(f,220,220)==(255,255,0,255) and preserved(f) and pixel(f,5,5)==(101,67,33,255))
        wait('hidden rAF resumes magenta phase', lambda f: f[0]['generation']==5 and pixel(f,220,220)==(255,0,255,255) and preserved(f) and pixel(f,5,5)==(101,67,33,255))
        click(80,40)
        wait('counter state survives theme and resize (second click)', lambda f: pixel(f,220,40)==(255,255,0,255))
        idle_deadline = time.monotonic()+5
        while True:
            message = messages.get(timeout=max(0.001, idle_deadline-time.monotonic()))
            if isinstance(message, BaseException):
                raise message
            if isinstance(message, dict) and message.get('Status', {}).get('animating') is False:
                break
            assert time.monotonic() < idle_deadline, 'worker did not stop animating'
        time.sleep(0.2)
        click(80,40)
        stable = wait('static idle worker wakes for third click without Pump', lambda f: pixel(f,220,40)==(255,0,255,255) and pixel(f,5,5)==(101,67,33,255))
        # Scroll suppression hides the worker while the parent keeps its cached
        # texture. Check EVERY activation frame, not just eventual good pixels.
        for cycle in range(12):
            document['viewport'] = dict(width=400,height=300,scale=1.0)
            document['visible'] = False
            send({'Reconcile':dict(document=document,generation=10+cycle*2)})
            time.sleep(0.04 if cycle % 2 == 0 else 0.001)
            document['visible'] = True
            generation = 11+cycle*2
            send({'Reconcile':dict(document=document,generation=generation)})
            deadline = time.monotonic()+0.25
            observed = 0
            while time.monotonic() < deadline:
                try:
                    message = messages.get(timeout=0.02)
                except queue.Empty:
                    continue
                if isinstance(message, BaseException):
                    raise message
                if isinstance(message, tuple) and message[0]['generation']==generation:
                    observed += 1
                    assert pixel(message,5,5)==(101,67,33,255), f'activation {cycle} published blank/stale pixels: {pixel(message,5,5)}, sequence={message[0]["sequence"]}'
                    assert pixel(message,220,40)==(255,0,255,255), 'activation lost DOM counter state'
                    assert message[1] == stable[1], f'activation {cycle} changed static raster pixels'
            assert observed, f'activation {cycle} failed to produce a ready frame'
        print('PASS all pixels across 12 idle/rapid hidden-visible scroll cycles', flush=True)
        # Legitimate white content is allowed: this is scene lifecycle gating,
        # not rejection of white pixels or permanent freezing of cached frames.
        colors['background'] = 0xffffff
        send({'Reconcile':dict(document=document,generation=40)})
        wait('legitimate white theme renders with live DOM state', lambda f: f[0]['generation']==40 and pixel(f,5,5)==(255,255,255,255) and pixel(f,220,40)==(255,0,255,255))
        colors['background'] = 0x654321
        send({'Reconcile':dict(document=document,generation=41)})
        wait('theme changes remain live after scroll cycles', lambda f: f[0]['generation']==41 and pixel(f,5,5)==(101,67,33,255) and pixel(f,220,40)==(255,0,255,255))
        # Author-JS counters encoded in actual source pixels, no host evaluation
        # or privileged testing bridge. Snapshots freeze the values on real input.
        perf_html = '''<!doctype html><style>
        html,body{margin:0;background:var(--background)}
        button{position:absolute;left:20px;width:160px;height:40px}
        #sample{top:20px}#stop{top:80px}
        .probe{position:absolute;left:200px;width:40px;height:40px}
        #raf{top:20px}#timer{top:80px}#serial{top:140px}#motion{top:200px}
        #motion{background:red;animation:pulse .1s infinite alternate}
        @keyframes pulse{to{opacity:.5}}
        </style><button id="sample">sample</button><button id="stop">stop</button>
        <div class="probe" id="raf"></div><div class="probe" id="timer"></div>
        <div class="probe" id="serial"></div><div class="probe" id="motion"></div>
        <script>
        let ticks=0,timers=0,serials=0,running=true;
        const encode=n=>`rgb(${n&255},${(n>>8)&255},${(n>>16)&255})`;
        function tick(){if(!running)return;ticks++;motion.style.background=encode(ticks);requestAnimationFrame(tick)}
        requestAnimationFrame(tick);
        const interval=setInterval(()=>timers++,10);
        sample.onclick=()=>{raf.style.background=encode(ticks);timer.style.background=encode(timers);serial.style.background=encode(++serials)};
        document.getElementById('stop').onclick=()=>{running=false;clearInterval(interval);motion.style.animation='none';serial.style.background=encode(++serials)};
        </script>'''
        send({'Destroy':dict(key='live')})
        document.update(html=perf_html, revision=2, visible=False)
        def cpu_seconds():
            # Linux process stat includes all threads; not just the pipe thread.
            path = Path(f'/proc/{child.pid}/stat')
            if not path.exists():
                return None
            fields = path.read_text().rsplit(')',1)[1].split()
            return (int(fields[11])+int(fields[12]))/os.sysconf('SC_CLK_TCK')
        def cpu_window(seconds):
            before = cpu_seconds()
            start = time.monotonic()
            time.sleep(seconds)
            elapsed = time.monotonic()-start
            after = cpu_seconds()
            return elapsed, None if before is None or after is None else after-before
        def perf_assert(condition, label):
            if baseline:
                print(f'BASELINE {label}: {condition}', flush=True)
            else:
                assert condition, label
        def decode(frame,x,y):
            r,g,b,_ = pixel(frame,x,y)
            return r+(g<<8)+(b<<16)
        serial = 0
        def sample(label):
            click(80,40)
            return wait(label, lambda f: f[0]['revision']==2 and decode(f,220,160)==serial)
        send({'Reconcile':dict(document=document,generation=50)})
        time.sleep(.5) # load settles; initial-hidden must already be throttled
        initial_elapsed, initial_cpu = cpu_window(1.2)
        document['visible'] = True
        send({'Reconcile':dict(document=document,generation=51)})
        wait('initially hidden source resumes without white', lambda f: f[0]['generation']==51 and pixel(f,5,5)==(101,67,33,255))
        serial += 1
        first = sample('initial-hidden JS counter snapshot')
        initial_raf, initial_timer = decode(first,220,40), decode(first,220,100)
        print(f'METRIC initial-hidden: rAF={initial_raf} timers={initial_timer} CPU={initial_cpu}s/{initial_elapsed:.3f}s', flush=True)
        perf_assert(initial_raf<=8 and initial_timer<=8, 'initial hidden JS progress bounded')
        time.sleep(.2)
        start = time.monotonic()
        visible_elapsed, visible_cpu = cpu_window(1.5)
        end = time.monotonic()
        visible_times = [t for t,m in arrivals if start<=t<=end and m['revision']==2]
        fps = len(visible_times)/visible_elapsed
        print(f'METRIC visible: {len(visible_times)} frames/{visible_elapsed:.3f}s = {fps:.2f}fps CPU={visible_cpu}s', flush=True)
        perf_assert(len(visible_times)<=math.ceil(visible_elapsed*30), 'visible publication <=30fps cap')
        serial += 1
        before = sample('visible JS counters before hiding')
        visible_raf_delta = decode(before,220,40)-initial_raf
        visible_timer_delta = decode(before,220,100)-initial_timer
        print(f'METRIC visible author JS progress: rAF delta={visible_raf_delta}, timer delta={visible_timer_delta}', flush=True)
        assert visible_raf_delta>=20 and visible_timer_delta>=50, 'presentation cap stopped author JS instead of pacing readback'
        document['visible'] = False
        send({'Reconcile':dict(document=document,generation=52)})
        time.sleep(.1) # exclude in-flight transition work from idle CPU sample
        colors['background'] = 0x234567
        send({'Reconcile':dict(document=document,generation=53)})
        hidden_elapsed, hidden_cpu = cpu_window(1.2)
        assert not any(m['generation'] in (50,52,53) for _,m in arrivals), 'hidden publication occurred'
        document['visible'] = True
        send({'Reconcile':dict(document=document,generation=54)})
        wait('hidden style update resumes correct source pixels', lambda f: f[0]['generation']==54 and pixel(f,5,5)==(35,69,103,255))
        serial += 1
        after = sample('hidden JS counter delta snapshot')
        raf_delta = decode(after,220,40)-decode(before,220,40)
        timer_delta = decode(after,220,100)-decode(before,220,100)
        print(f'METRIC hidden: rAF delta={raf_delta}, timer delta={timer_delta}, CPU={hidden_cpu}s/{hidden_elapsed:.3f}s', flush=True)
        perf_assert(raf_delta<=8 and timer_delta<=8, 'hidden rAF/timer actual JS progress bounded')
        if hidden_cpu is not None and visible_cpu is not None:
            perf_assert(hidden_cpu/hidden_elapsed < visible_cpu/visible_elapsed*.25 + .02, 'hidden process CPU substantially below visible')
        # Stopping occurs directly after an animated publication, inside its
        # cap deadline. The final one-shot dirty frame must survive that wait.
        # Once settled, static input should not inherit animation pacing.
        serial += 1
        click(80,100)
        wait('final animation frame not stranded', lambda f: f[0]['revision']==2 and decode(f,220,160)==serial)
        idle_deadline = time.monotonic()+5
        while True:
            message = messages.get(timeout=max(.001,idle_deadline-time.monotonic()))
            if isinstance(message, BaseException):
                raise message
            if isinstance(message, dict) and message.get('Status',{}).get('animating') is False:
                break
            assert time.monotonic()<idle_deadline, 'performance fixture did not become static'
        latencies = []
        for trial in range(10):
            serial += 1
            start = time.monotonic()
            sample(f'static one-shot input without parent Pump {trial}')
            latencies.append((time.monotonic()-start)*1000)
        print(f'METRIC static input latencies ms: {", ".join(f"{ms:.1f}" for ms in latencies)}', flush=True)
        perf_assert(max(latencies)<33.334, 'static input <= one 30fps interval end-to-end')
        assert requests == [], f'network escaped: {requests}'
        print('PASS deny network: local HTTP server received zero requests', flush=True)
        send('Shutdown')
        assert child.wait(timeout=15)==0
        print(f'PASS clean shutdown; binary={BINARY}; stderr={log}', flush=True)
        if failures:
            raise AssertionError(f'{len(failures)} pixel regression(s) remain; all other phases completed')
    finally:
        if child.poll() is None:
            child.kill()
            child.wait(timeout=5)
        server.shutdown()
