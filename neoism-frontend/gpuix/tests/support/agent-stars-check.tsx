import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {createSignal,Show} from "solid-js";
import {AgentStars} from "../../src/agent/AgentStars";
import {NativeComposer} from "../../src/agent/NativeComposer";
import {loadConfig} from "../../src/config";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";

const host=createRecordingAppRenderer(),root=createRoot(host.renderer),config=loadConfig([],{},{}),composer=new NativeComposer(config);
const [visible,setVisible]=createSignal(true),[enabled,setEnabled]=createSignal(true),[reduced,setReduced]=createSignal(false);
const callbacks=new Set<()=>void>(),restore=usePresentationScheduler(callback=>{callbacks.add(callback);return()=>callbacks.delete(callback);});
const paint=()=>{
 const node=host.find("agent-stars");assert.ok(node);
 assert.equal(node.style.pointerEvents,"none");
 return String(host.nodes.get(node.children[0])!.props.commands);
};
try{
 root.render(()=><Show when={visible()}><AgentStars config={{...config,agentStars:enabled(),reducedMotion:reduced()}} composerId={composer.id} width={752} height={102} radius={16}/></Show>);
 await host.pump(root);assert.ok(JSON.parse(paint()).length>15);assert.equal(callbacks.size,1);
 setReduced(true);await host.pump(root);assert.ok(JSON.parse(paint()).length>15);assert.equal(callbacks.size,0);
 setReduced(false);await host.pump(root);assert.equal(callbacks.size,1);
 setEnabled(false);await host.pump(root);assert.equal(paint(),"[]");assert.equal(callbacks.size,0);
 setEnabled(true);await host.pump(root);assert.equal(callbacks.size,1);
 setVisible(false);await host.pump(root);assert.equal(callbacks.size,0);
 setVisible(true);await host.pump(root);assert.equal(callbacks.size,1);
 console.log("Agent stars paint without input hit regions; disabling, reduced motion and unmount cancel animation.");
}finally{root.unmount();composer.dispose();restore();assert.equal(callbacks.size,0);}
