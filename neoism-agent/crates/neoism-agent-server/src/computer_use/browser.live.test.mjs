import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';

// Opt-in: launches only a throwaway headless test profile, never the user's browser.
const binary = process.env.NEOISM_TEST_CHROMIUM;
const observe = await readFile(new URL('./browser_observe.js', import.meta.url), 'utf8');
const act = await readFile(new URL('./browser_action.js', import.meta.url), 'utf8');
test('isolated-world browser refs, action effects, deltas and stale protections', {skip: !binary, timeout:30000}, async () => {
  const profile = await mkdtemp(join(tmpdir(), 'neoism-browser-test-'));
  const server = createServer((_req, res) => {
    res.writeHead(200, {'content-type':'text/html'});
    res.end(`<html><body><label for="q">Search</label><input id="q"><button id="go">Search now</button><select aria-label="Sort"><option value="a">Alpha</option><option value="b">Beta</option></select><input type="password" aria-label="Secret"><p id="result">Ready</p><script>document.querySelector('#go').onclick=()=>{document.querySelector('#result').textContent='Results '+document.querySelector('#q').value};</script></body></html>`);
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const child = spawn(binary, ['--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check', '--remote-debugging-address=127.0.0.1', '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank'], {stdio:['ignore','ignore','pipe']});
  let stderr = '';
  child.stderr.on('data', data => { stderr = (stderr + data).slice(-8000); });
  let socket;
  let connect;
  try {
    let endpoint;
    for (let i=0; i<100; i++) {
      try {
        const [port,path] = (await readFile(join(profile,'DevToolsActivePort'),'utf8')).trim().split('\n');
        endpoint = `ws://127.0.0.1:${port}${path}`;
        break;
      } catch { await new Promise(resolve => setTimeout(resolve,100)); }
    }
    assert.ok(endpoint, `Browser did not start: ${stderr}`);
    let next=0;
    connect = async () => {
      socket = new WebSocket(endpoint);
      await once(socket,'open');
      const pending = new Map();
      socket.addEventListener('message', ({data}) => {
        const message = JSON.parse(data);
        const waiter = pending.get(message.id);
        if (!waiter) return;
        pending.delete(message.id);
        clearTimeout(waiter.timer);
        message.error ? waiter.reject(new Error(JSON.stringify(message.error))) : waiter.resolve(message.result);
      });
      return (method,params={},sessionId) => new Promise((resolve,reject) => {
        const id=++next;
        const timer=setTimeout(() => {pending.delete(id);reject(new Error(`Timed out: ${method}`));},3000);
        pending.set(id,{resolve,reject,timer});
        socket.send(JSON.stringify({id,method,params,sessionId}));
      });
    };
    let call = await connect();
    const url = `http://127.0.0.1:${server.address().port}/`;
    const {targetId} = await call('Target.createTarget',{url});
    let {sessionId} = await call('Target.attachToTarget',{targetId,flatten:true});
    await call('Page.bringToFront',{},sessionId);
    await new Promise(resolve => setTimeout(resolve,250));
    const tree = await call('Page.getFrameTree',{},sessionId);
    const {executionContextId} = await call('Page.createIsolatedWorld',{frameId:tree.frameTree.frame.id,worldName:'neoism-computer-use'},sessionId);
    const evaluate = async expression => {
      const result=await call('Runtime.evaluate',{contextId:executionContextId,expression,returnByValue:true},sessionId);
      assert.equal(result.exceptionDetails,undefined,JSON.stringify(result));
      return result.result.value;
    };
    let page = await evaluate(`(${observe})("one")`);
    assert.equal(page.focused,true);
    assert.equal(page.visible,true);
    assert.ok(page.elements.some(e => e.name==='Search'));
    assert.ok(!page.elements.some(e => e.name==='Secret'));
    const search = page.elements.find(e => e.name==='Search');
    const payload = {token:'one',ref:search.ref,action:'fill',value:'literal "quotes" and \\slashes',url};
    assert.equal((await evaluate(`(${act})(${JSON.stringify(payload)})`)).ok,true);
    assert.equal((await evaluate(`(${act})(${JSON.stringify(payload)})`)).ok,false,'no replay');
    assert.equal(await evaluate('document.querySelector("#q").value'),payload.value);
    page = await evaluate(`(${observe})("two")`);
    assert.equal(page.elements.find(e=>e.name==='Search').ref,search.ref,'stable refs');
    const click = {token:'two',ref:page.elements.find(e=>e.name==='Search now').ref,action:'click',url};
    assert.equal((await evaluate(`(${act})(${JSON.stringify(click)})`)).ok,true);
    page = await evaluate(`(${observe})("three")`);
    assert.ok(page.text.includes('Results literal'));
    const select = {token:'three',ref:page.elements.find(e=>e.name==='Sort').ref,action:'select',value:'b',url};
    assert.equal((await evaluate(`(${act})(${JSON.stringify(select)})`)).ok,true);
    assert.equal(await evaluate('document.querySelector("select").value'),'b');
    assert.equal(page.elements.find(e=>e.name==='Search').value,payload.value,'filled value is observable');
    page = await evaluate(`(${observe})("occluded")`);
    await evaluate('document.body.insertAdjacentHTML("beforeend",\'<div id="overlay" style="position:fixed;inset:0;z-index:999;background:white"></div>\')');
    assert.equal((await evaluate(`(${act})(${JSON.stringify({...payload,token:'occluded'})})`)).ok,false,'covered control rejected');
    await evaluate('document.querySelector("#overlay").remove()');
    page = await evaluate(`(${observe})("disabled")`);
    await evaluate('document.querySelector("#q").disabled=true');
    assert.equal((await evaluate(`(${act})(${JSON.stringify({...payload,token:'disabled'})})`)).ok,false,'disabled control rejected');
    await evaluate('document.querySelector("#q").disabled=false');
    page = await evaluate(`(${observe})("four")`);
    await evaluate('document.querySelector("#go").textContent="Different action"');
    assert.equal((await evaluate(`(${act})(${JSON.stringify({...click,token:'four'})})`)).ok,false,'changed label rejected');
    page = await evaluate(`(${observe})("five")`);
    // Context and reference identity must survive the per-tool WebSocket reconnect.
    socket.close();
    await once(socket,'close');
    call = await connect();
    ({sessionId} = await call('Target.attachToTarget',{targetId,flatten:true}));
    assert.equal((await evaluate(`(${act})(${JSON.stringify({...payload,token:'five',value:'after reconnect'})})`)).ok,true);
    await evaluate('history.pushState({},"","/navigated")');
    page = await evaluate(`(${observe})("six")`);
    assert.equal((await evaluate(`(${act})(${JSON.stringify({...payload,token:'six'})})`)).ok,false,'URL mismatch rejected');
    // Page code cannot read the element registry in the isolated world.
    const main = await call('Runtime.evaluate',{expression:'typeof globalThis.__neoismComputer',returnByValue:true},sessionId);
    assert.equal(main.result.value,'undefined');
  } finally {
    socket?.close();
    child.kill('SIGTERM');
    if (child.exitCode === null) await once(child,'exit');
    server.closeAllConnections();
    await new Promise(resolve=>server.close(resolve));
    await rm(profile,{recursive:true,force:true,maxRetries:5,retryDelay:100});
  }
});
