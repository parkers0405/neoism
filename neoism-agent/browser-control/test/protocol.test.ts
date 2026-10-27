import {test} from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import {once} from "node:events"
import {WebSocket} from "ws"
import {startServer} from "../src/server.ts"
import {safeHttpUrl} from "../src/protocol.ts"
import {validateFirefoxEndpoint} from "../src/firefox-transport.ts"

test("Firefox accepts only a loopback new-session endpoint",()=> {
  assert.equal(validateFirefoxEndpoint("ws://127.0.0.1:9223/session"),"ws://127.0.0.1:9223/session")
  for(const value of ["ws://localhost:9223/session","ws://example.com/session","ws://127.0.0.1:9223/session/foreign","ws://u:p@127.0.0.1/session","wss://127.0.0.1/session"])assert.throws(()=>validateFirefoxEndpoint(value))
})
test("page URLs do not accept executable schemes or embedded credentials",()=> {
  assert.equal(safeHttpUrl("https://neoism.dev"),"https://neoism.dev/")
  for(const value of ["javascript:alert(1)","file:///etc/passwd","https://u:p@neoism.dev"])assert.throws(()=>safeHttpUrl(value))
})
test("relay authentication, request correlation, isolation and no action replay",async()=> {
  const directory=await fs.mkdtemp(path.join(os.tmpdir(),"neoism-protocol-")), previous=process.env.NEOISM_BROWSER_STATE_DIR
  process.env.NEOISM_BROWSER_STATE_DIR=directory
  const server=await startServer("chrome",0), url=`http://127.0.0.1:${server.port}`
  let socket:WebSocket|undefined
  try {
    assert.equal((await fs.stat(path.join(directory,"chrome-pairing-token"))).mode & 0o777,0o600)
    assert.equal((await fetch(`${url}/rpc`,{method:"POST",body:'{"method":"status"}'})).status,401)
    assert.equal((await fetch(`${url}/health`,{headers:{Origin:"https://page.example"}})).status,403)
    const bad=new WebSocket(url.replace("http:","ws:")+"/bridge")
    await once(bad,"open");bad.send(JSON.stringify({type:"hello",browser:"chrome",protocol:1,token:"0".repeat(64)}));const [code]=await once(bad,"close");assert.equal(code,4003)
    socket=new WebSocket(url.replace("http:","ws:")+"/bridge");await once(socket,"open")
    const ready=once(socket,"message");socket.send(JSON.stringify({type:"hello",browser:"chrome",protocol:1,token:server.token}));await ready
    const message=once(socket,"message"), operation=server.bridge.call("input.once",{value:"one"})
    const [raw]=await message, command=JSON.parse(raw.toString())
    socket.send(JSON.stringify({type:"reply",id:command.id,result:{ok:true}}))
    assert.deepEqual(await operation,{ok:true})
    const received=once(socket,"message"), uncertain=server.bridge.call("input.uncertain")
    const rejected=assert.rejects(uncertain,/may already have run/)
    await received;socket.close();await rejected
    assert.equal(server.bridge.connected,false)
    assert.deepEqual(await server.driver.invoke("session_new",{name:"independent"}),{id:"independent"})
    await server.driver.invoke("session_use",{session:"independent"})
    assert.deepEqual(await server.driver.invoke("session_current"),{id:"independent"})
    await assert.rejects(server.driver.invoke("session_adopt",{tabId:999}),/Pair/)
    socket=new WebSocket(url.replace("http:","ws:")+"/bridge");await once(socket,"open")
    const second=once(socket,"message");socket.send(JSON.stringify({type:"hello",browser:"chrome",protocol:1,token:server.token}));await second
    let replayed=false;socket.on("message",()=>{replayed=true});await new Promise(resolve=>setTimeout(resolve,100));assert.equal(replayed,false)
  }finally {socket?.terminate();await server.close();if(previous===undefined)delete process.env.NEOISM_BROWSER_STATE_DIR;else process.env.NEOISM_BROWSER_STATE_DIR=previous;await fs.rm(directory,{recursive:true,force:true})}
})
