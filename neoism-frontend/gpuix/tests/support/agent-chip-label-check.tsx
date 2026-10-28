import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {createSignal,Show} from "solid-js";
import {AgentChipHosts,AgentChipLabel} from "../../src/agent/AgentChipLabel";
import {WordmarkHosts} from "../../src/ui/AnimatedWordmark";
import {coreCall} from "../../src/native-core";
import {loadConfig} from "../../src/config";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
const host=createRecordingAppRenderer(),uploads:number[]=[],drops:number[]=[];
const imageRenderer=host.renderer as unknown as {putPaintImage:(id:number)=>void;dropPaintImage:(id:number)=>void};
imageRenderer.putPaintImage=(id:number)=>{uploads.push(id);};imageRenderer.dropPaintImage=(id:number)=>{drops.push(id);};
const root=createRoot(host.renderer),config=loadConfig(["--config","/nonexistent/agent-chip"],{}),hosts=new AgentChipHosts(),pins=new WordmarkHosts();
const [visible,setVisible]=createSignal(true),[label,setLabel]=createSignal("Build"),[loading,setLoading]=createSignal(true),[reduced,setReduced]=createSignal(false);
const callbacks=new Set<()=>void>(),restore=usePresentationScheduler(callback=>{callbacks.add(callback);return()=>callbacks.delete(callback);});
const pulse=async()=>{const pending=[...callbacks];callbacks.clear();for(const callback of pending)callback();await host.pump(root);};
const current=()=>{const node=host.find("agent-chip-label:agent")!;assert.ok(node);const canvas=host.nodes.get(node.children[0])!;return JSON.parse(String(canvas.props.commands))as Array<{k:string;id?:number}>;};
try{
 root.render(()=><Show when={visible()}><AgentChipLabel config={{...config,reducedMotion:reduced()}} hosts={hosts} paintPins={pins} scope="home:a" kind="agent" label={label()} size={14} loading={loading()}/></Show>);await host.pump(root);
 assert.ok(current().length>=4,"Original loading animation paints individual glyphs");assert.ok(callbacks.size>0);
 setLoading(false);await Bun.sleep(350);await pulse();assert.equal(callbacks.size,0);
 const id=hosts.acquire("home:a","agent",config),inspect=()=>coreCall<{phase:string}>("agentcomposer.chip-frame",{id,kind:"agent",label:label(),fontSize:14*config.scale});
 assert.equal(inspect().phase,"idle");const beforeUploads=uploads.length,beforeDrops=drops.length;
 setVisible(false);await host.pump(root);setVisible(true);await host.pump(root);assert.equal(inspect().phase,"idle");assert.equal(callbacks.size,0);assert.equal(uploads.length,beforeUploads);assert.equal(drops.length,beforeDrops);
 setLabel("Plan");await host.pump(root);assert.equal(inspect().phase,"changing");assert.ok(current().length>=4);assert.ok(callbacks.size>0);
 setReduced(true);await pulse();assert.equal(callbacks.size,0);assert.ok(current().length>0);
 console.log(JSON.stringify({noWindow:true,originalLoadingColors:true,originalModeScramble:true,settledRemountNoReplay:true,noTextureChurn:true,reducedMotion:true}));
}finally{root.unmount();hosts.dispose();pins.dispose();restore();assert.equal(callbacks.size,0);}
