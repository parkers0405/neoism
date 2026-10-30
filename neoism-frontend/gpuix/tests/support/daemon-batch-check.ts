import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import type WebSocket from "ws";
import { createRoot, createSignal, createEffect } from "solid-js";
import { DaemonConnection } from "../../src/daemon/client";

class Socket extends EventEmitter {
  readyState=1;
  send() {}
  terminate() { this.readyState=3;this.emit("close"); }
  reply(family:string,message:unknown) { this.emit("message",Buffer.from(JSON.stringify({[family+"Reply"]:{request_id:0,message}}))); }
}
const socket=new Socket(),client=new DaemonConnection({endpoint:"ws://localhost/session"},()=>socket as unknown as WebSocket);
const observed:number[][]=[];
let dispose=()=>{};
try {
  client.start();socket.emit("open");socket.reply("Workspace",{HelloAck:{accepted:true}});
  createRoot(d=>{
    dispose=d;
    const [a,setA]=createSignal(0),[b,setB]=createSignal(0);
    createEffect(()=>{observed.push([a(),b()]);});
    client.subscribe(()=>setA(v=>v+1));
    client.subscribe(()=>setB(v=>v+1));
  });
  assert.deepEqual(observed,[[0,0]]);
  socket.reply("Git",{Branch:{name:"fixture"}});
  socket.reply("Git",{Branch:{name:"next"}});
  assert.deepEqual(observed,[[0,0],[1,1],[2,2]],"each reply publishes one coherent update, in arrival order");
  console.log(JSON.stringify({updates:observed.length-1,ordered:true,noWindow:true}));
} finally { dispose();client.stop(); }
