import assert from "node:assert/strict";
import {mock} from "bun:test";
import {createRoot} from "@gpuix/solid";
import {createSignal} from "solid-js";
import * as native from "../../src/native-core";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
import {loadConfig} from "../../src/config";
const original=native.coreCall,calls=new Map<string,number>();
mock.module("../../src/native-core",()=>({...native,coreCall:(op:string,args?:Record<string,unknown>)=>{calls.set(op,(calls.get(op)||0)+1);return original(op,args);}}));
const {InlinePicker}=await import("../../src/ui/InlinePicker");
const host=createRecordingAppRenderer(),root=createRoot(host.renderer),config=loadConfig(["--config","/nonexistent/picker-budget"],{});
const [query,setQuery]=createSignal(""),[anchor,setAnchor]=createSignal<[number,number,number,number]>([30,700,700,100]);
const options=Array.from({length:2000},(_,i)=>({title:`Model ${i}`,value:`model-${i}`}));
try{
 root.render(()=><InlinePicker config={config} kind="model" options={options} query={query()} anchor={anchor()} onClose={()=>{}}/>);await host.pump(root);
 assert.equal(calls.get("picker.paint"),1,"Initial effects share a single paint");
 calls.clear();setQuery("Model 1999");await host.pump(root);
 assert.equal(calls.get("picker.options")||0,0,"Typing must not retransmit the catalog");
 assert.ok(host.find("picker-option:model-1999"));assert.equal(calls.get("picker.paint"),1);
 calls.clear();setAnchor([40,700,700,100]);await host.pump(root);
 assert.equal(calls.get("picker.options")||0,0,"Placement must not retransmit the catalog");
 assert.equal(calls.get("picker.query")||0,0);assert.equal(calls.get("picker.paint"),1);
 console.log(JSON.stringify({noWindow:true,catalogSize:options.length,initialPaints:1,queryCatalogWrites:0,layoutCatalogWrites:0}));
}finally{root.unmount();}
