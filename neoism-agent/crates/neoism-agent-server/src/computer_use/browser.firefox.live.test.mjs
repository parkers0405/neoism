import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';

// Opt-in, isolated profile only. The normal Firefox profile is never opened.
const binary = process.env.NEOISM_TEST_FIREFOX;
const observe = await readFile(new URL('./browser_observe.js', import.meta.url), 'utf8');
const act = await readFile(new URL('./browser_action.js', import.meta.url), 'utf8');
test('Firefox BiDi sandbox, persistent refs, actions and session cleanup', {skip: !binary, timeout:process.env.NEOISM_TEST_BROWSER_RUST ? 300000 : 30000}, async () => {
  const profile = await mkdtemp(join(tmpdir(), 'neoism-firefox-test-'));
  const server = createServer((_req, res) => {
    res.writeHead(200, {'content-type':'text/html'});
    res.end(`<html><body><label for="q">Search</label><input id="q"><button id="go">Search now</button><select aria-label="Sort"><option value="a">Alpha</option><option value="b">Beta</option></select><input type="password" aria-label="Secret"><p id="result">Ready</p><script>document.querySelector('#go').onclick=()=>{document.querySelector('#result').textContent='Results '+document.querySelector('#q').value};</script></body></html>`);
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const child = spawn(binary, ['--headless', '--no-remote', '--profile', profile, '--remote-debugging-port', '0', 'about:blank'], {stdio:['ignore','ignore','pipe']});
  let stderr = '';
  child.stderr.on('data', data => { stderr = (stderr + data).slice(-8000); });
  let socket, call;
  try {
    let endpoint;
    for (let i=0; i<200; i++) {
      try {
        const {ws_host, ws_port} = JSON.parse(await readFile(join(profile,'WebDriverBiDiServer.json'),'utf8'));
        endpoint = `ws://${ws_host}:${ws_port}/session`;
        break;
      } catch { await new Promise(resolve => setTimeout(resolve,100)); }
    }
    assert.ok(endpoint, `Firefox did not start: ${stderr}`);
    if (process.env.NEOISM_TEST_BROWSER_RUST) {
      const rust = spawn('cargo',['test','-p','neoism-agent-server','firefox_live_protocol_roundtrip','--lib','--','--ignored','--test-threads=1'], {
        env:{...process.env,NEOISM_TEST_BIDI_URL:endpoint,NEOISM_TEST_BROWSER_PAGE:`http://127.0.0.1:${server.address().port}/`},
        stdio:['ignore','pipe','pipe'],
      });
      let output='';
      rust.stdout.on('data',data=>{output=(output+data).slice(-32000);});
      rust.stderr.on('data',data=>{output=(output+data).slice(-32000);});
      const [code]=await once(rust,'exit');
      assert.equal(code,0,output);
      console.log('Production Rust Firefox transport/session/action roundtrip passed');
    }
    socket = new WebSocket(endpoint);
    await once(socket,'open');
    let next=0;
    const pending = new Map();
    socket.addEventListener('message', ({data}) => {
      const message = JSON.parse(data);
      const waiter = pending.get(message.id);
      if (!waiter) return;
      pending.delete(message.id);
      clearTimeout(waiter.timer);
      message.type === 'error' ? waiter.reject(new Error(JSON.stringify(message))) : waiter.resolve(message.result);
    });
    call = (method,params={}) => new Promise((resolve,reject) => {
      const id=++next;
      const timer=setTimeout(() => {pending.delete(id);reject(new Error(`Timed out: ${method}`));},3000);
      pending.set(id,{resolve,reject,timer});
      socket.send(JSON.stringify({id,method,params}));
    });
    await call('session.new', {capabilities:{alwaysMatch:{acceptInsecureCerts:false,unhandledPromptBehavior:'ignore'}}});
    const url = `http://127.0.0.1:${server.address().port}/`;
    const {context} = await call('browsingContext.create',{type:'tab',background:false});
    await call('browsingContext.navigate',{context,url,wait:'complete'});
    await call('browsingContext.activate',{context});
    const sandbox = await call('script.evaluate',{expression:'null',target:{context,sandbox:'neoism-computer-use'},awaitPromise:false,resultOwnership:'none'});
    assert.equal(sandbox.type,'success');
    const realm = sandbox.realm;
    const evaluate = async expression => {
      const result=await call('script.evaluate',{target:{realm},expression:`JSON.stringify(${expression})`,awaitPromise:false,resultOwnership:'none',serializationOptions:{maxObjectDepth:0}});
      assert.equal(result.type,'success',JSON.stringify(result));
      assert.equal(result.result.type,'string',JSON.stringify(result));
      return JSON.parse(result.result.value);
    };
    let page = await evaluate(`(${observe})("one")`);
    assert.equal(page.focused,true);
    assert.equal(page.visible,true);
    const search = page.elements.find(e => e.name==='Search');
    assert.ok(search,'named field');
    assert.ok(!page.elements.some(e => e.name==='Secret'));
    const fill = {token:'one',ref:search.ref,action:'fill',value:'literal "quotes" and \\slashes',url};
    assert.equal((await evaluate(`(${act})(${JSON.stringify(fill)})`)).ok,true);
    assert.equal((await evaluate(`(${act})(${JSON.stringify(fill)})`)).ok,false,'no replay');
    page = await evaluate(`(${observe})("two")`);
    assert.equal(page.elements.find(e=>e.name==='Search').ref,search.ref,'stable element refs');
    assert.equal(page.elements.find(e=>e.name==='Search').value,fill.value);
    const click = {token:'two',ref:page.elements.find(e=>e.name==='Search now').ref,action:'click',url};
    assert.equal((await evaluate(`(${act})(${JSON.stringify(click)})`)).ok,true);
    page = await evaluate(`(${observe})("three")`);
    assert.ok(page.text.includes('Results literal'));
    const select = {token:'three',ref:page.elements.find(e=>e.name==='Sort').ref,action:'select',value:'b',url};
    assert.equal((await evaluate(`(${act})(${JSON.stringify(select)})`)).ok,true);
    assert.equal(await evaluate('document.querySelector("select").value'),'b');
    page = await evaluate(`(${observe})("four")`);
    await evaluate('(document.querySelector("#go").textContent="Changed")');
    assert.equal((await evaluate(`(${act})(${JSON.stringify({...click,token:'four'})})`)).ok,false,'changed label rejected');
    page = await evaluate(`(${observe})("disabled")`);
    await evaluate('(document.querySelector("#q").disabled=true)');
    assert.equal((await evaluate(`(${act})(${JSON.stringify({...fill,token:'disabled'})})`)).ok,false,'disabled field rejected');
    await evaluate('(document.querySelector("#q").disabled=false)');
    page = await evaluate(`(${observe})("covered")`);
    await evaluate('(()=>{const overlay=document.createElement("div");overlay.id="overlay";overlay.style="position:fixed;inset:0;z-index:999;background:white";document.body.append(overlay);return true})()');
    assert.equal((await evaluate(`(${act})(${JSON.stringify({...fill,token:'covered'})})`)).ok,false,'covered field rejected');
    await evaluate('(()=>{document.querySelector("#overlay").remove();return true})()');
    page = await evaluate(`(${observe})("persistent")`);
    // Independent tool rounds reuse the session and named sandbox without resetting refs.
    assert.ok((await call('browsingContext.getTree',{maxDepth:0})).contexts.some(t=>t.context===context));
    const nextRealm = await call('script.evaluate',{expression:'null',target:{context,sandbox:'neoism-computer-use'},awaitPromise:false});
    assert.equal(nextRealm.realm,realm);
    assert.equal((await evaluate(`(${act})(${JSON.stringify({...fill,token:'persistent',value:'next call'})})`)).ok,true);
    const main = await call('script.evaluate',{expression:'typeof globalThis.__neoismComputer',target:{context},awaitPromise:false});
    assert.equal(main.result.value,'undefined','page cannot read sandbox registry');
    page = await evaluate(`(${observe})("navigation")`);
    await evaluate('(()=>{history.pushState({},"","/navigated");return true})()');
    assert.equal((await evaluate(`(${act})(${JSON.stringify({...fill,token:'navigation'})})`)).ok,false,'URL mismatch rejected');
    const other = await call('browsingContext.create',{type:'tab',background:false});
    await call('browsingContext.activate',{context:other.context});
    const hidden = await call('script.evaluate',{expression:`(${observe})("hidden")`,target:{realm},awaitPromise:false});
    assert.equal(hidden.type,'exception','hidden or unfocused page is not observed');
    await call('session.end');
    call = null;
  } finally {
    if (call && socket?.readyState === WebSocket.OPEN) await call('session.end').catch(()=>{});
    socket?.close();
    child.kill('SIGTERM');
    if (child.exitCode === null && child.signalCode === null) await once(child,'exit');
    server.closeAllConnections();
    await new Promise(resolve=>server.close(resolve));
    await rm(profile,{recursive:true,force:true,maxRetries:5,retryDelay:100});
  }
});
