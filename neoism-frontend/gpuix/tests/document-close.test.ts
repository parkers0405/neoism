import {expect,test} from "bun:test";
import {loadConfig} from "../src/config";
import type {DaemonConnection} from "../src/daemon/client";
import type {PresenceController} from "../src/collaboration/PresenceController";
import {DocumentController} from "../src/collaboration/DocumentController";
import {coreCall,NativeEditor} from "../src/native-core";

function fixture(seed=true){
 const sent:any[]=[],errors:unknown[]=[];let receive:(event:any)=>void=()=>{},status:(state:any)=>void=()=>{};
 let rejectUpdates=false;
 const client={subscribe(fn:typeof receive){receive=fn;return()=>{};},subscribeStatus(fn:typeof status){status=fn;fn({phase:"connected"});return()=>{};},send(_family:string,message:any){if(rejectUpdates&&message.Owned?.message.ApplySync)throw new Error("send failed");sent.push(message);}} as unknown as DaemonConnection;
 const presence={subscribe(){return()=>{};},filePeers(){return[];}} as unknown as PresenceController;
 const config={...loadConfig(["--config","/nonexistent/document-close","--daemon","ws://127.0.0.1:1/session"],{}),collaborative:true};
 const controller=new DocumentController(client,config,presence,error=>errors.push(error));
 const editor=new NativeEditor("/tmp/lease-test.md","",false);
 controller.attach(editor,"/tmp/lease-test.md",{id:"workspace",host_id:"host",title:"Workspace",visibility:"shared"});
 const acquired=sent.find(message=>message.AcquireBuffer).AcquireBuffer;
 if(seed)receive({family:"Crdt",message:{Snapshot:{buffer_id:acquired.buffer_id,update_v1:[0,0],state_vector_v1:[0]}}});
 return {sent,errors,editor,controller,acquired,offline:()=>status({phase:"closed"}),online:()=>status({phase:"connected"}),seed:()=>receive({family:"Crdt",message:{Snapshot:{buffer_id:acquired.buffer_id,update_v1:[0,0],state_vector_v1:[0]}}}),reject:()=>{rejectUpdates=true;},recover:()=>{rejectUpdates=false;}};
}

test("last local view detach flushes real native Yrs edits before releasing its document lease",()=>{
 const f=fixture(),view=f.editor.cloneView();
 try{
  expect(f.controller.isBound(f.editor)).toBe(true);
  expect(f.sent.some(message=>message.OpenBuffer)).toBe(false);
  f.controller.attachView(view,f.editor);
  // Native mutation without a JS publish leaves the edit pending until detach.
  coreCall("editor.paste",{id:f.editor.id,text:"pending text"});
  f.controller.detach(view);
  expect(f.sent.some(message=>message.CloseBuffer)).toBe(false);
  f.controller.detach(f.editor);
  expect(f.sent.findIndex(message=>message.Owned?.message.ApplySync)).toBeGreaterThan(0);
  expect(f.sent.findIndex(message=>message.Owned?.message.ApplySync)).toBeLessThan(f.sent.findIndex(message=>message.CloseBuffer));
  expect(f.sent.at(-1)).toEqual({CloseBuffer:{buffer_id:f.acquired.buffer_id,owner_id:f.acquired.owner_id}});
  f.controller.detach(f.editor);
  expect(f.sent.filter(message=>message.CloseBuffer)).toHaveLength(1);
  expect(f.errors).toEqual([]);
 }finally{f.controller.dispose();view.dispose();f.editor.dispose();}
});

test("detaching a canonical consumer keeps cloned consumers leased until their last detach",()=>{
 const f=fixture(),view=f.editor.cloneView();
 try{
  f.controller.attachView(view,f.editor);f.controller.detach(f.editor);
  expect(f.sent.some(message=>message.CloseBuffer)).toBe(false);
  expect(f.controller.bufferId(view)).toBe(f.acquired.buffer_id);
  f.controller.detach(view);
  expect(f.sent.filter(message=>message.CloseBuffer)).toHaveLength(1);
 }finally{f.controller.dispose();view.dispose();f.editor.dispose();}
});

test("offline detach, dispose, and failed edit transport never authorize eviction",()=>{
 for(const mode of ["offline","dispose","send-failure"]){
  const f=fixture();
  try{
   coreCall("editor.paste",{id:f.editor.id,text:"unsent"});
   if(mode==="offline")f.offline();
   if(mode==="send-failure")f.reject();
   if(mode==="dispose")f.controller.dispose();else f.controller.detach(f.editor);
   expect(f.sent.some(message=>message.CloseBuffer)).toBe(false);
   if(mode==="send-failure")expect(f.errors.length).toBeGreaterThan(0);
  }finally{f.controller.dispose();f.editor.dispose();}
 }
});

test("last detach replays previously drained but unacknowledged native edits",()=>{
 const f=fixture();
 try{
  f.reject();f.editor.call("paste",{text:"failed earlier"});
  expect(f.errors.length).toBeGreaterThan(0);
  expect(f.sent.some(message=>message.Owned?.message.ApplySync)).toBe(false);
  f.recover();f.controller.detach(f.editor);
  expect(f.sent.some(message=>message.Owned?.message.ApplySync?.envelope.update_v1.length>0)).toBe(true);
  expect(f.sent.at(-1).CloseBuffer.owner_id).toBe(f.acquired.owner_id);
 }finally{f.controller.dispose();f.editor.dispose();}
});

test("an unseeded native replica cannot authorize daemon eviction",()=>{
 const f=fixture(false);
 try{f.controller.detach(f.editor);expect(f.sent.some(message=>message.CloseBuffer)).toBe(false);}
 finally{f.controller.dispose();f.editor.dispose();}
});

test("unseeded detach keeps a recovery owner after the UI owner is disposed, then flushes on seed",()=>{
 const f=fixture(false);let disposed=false;
 try{
  f.editor.call("paste",{text:"typed before seed"});f.controller.detach(f.editor);f.editor.dispose();disposed=true;
  expect(f.sent.some(message=>message.CloseBuffer)).toBe(false);
  f.seed();
  expect(f.sent.some(message=>message.Owned?.message.ApplySync?.envelope.update_v1.length>0)).toBe(true);
  expect(f.sent.at(-1).CloseBuffer.owner_id).toBe(f.acquired.owner_id);
  expect(f.errors).toEqual([]);
 }finally{f.controller.dispose();if(!disposed)f.editor.dispose();}
});

test("offline detach keeps pending edits independent of the disposed UI owner until reconnect",()=>{
 const f=fixture();let disposed=false;
 try{
  f.offline();f.editor.call("paste",{text:"offline pending"});f.controller.detach(f.editor);f.editor.dispose();disposed=true;
  expect(f.sent.some(message=>message.CloseBuffer)).toBe(false);
  f.online();
  expect(f.sent.some(message=>message.Owned?.message.ApplySync?.envelope.update_v1.length>0)).toBe(true);
  expect(f.sent.at(-1).CloseBuffer.owner_id).toBe(f.acquired.owner_id);
  expect(f.errors).toEqual([]);
 }finally{f.controller.dispose();if(!disposed)f.editor.dispose();}
});