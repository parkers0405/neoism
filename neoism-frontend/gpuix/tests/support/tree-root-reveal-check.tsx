import assert from "node:assert/strict";
import { createRoot } from "@gpuix/solid";
import { createSignal } from "solid-js";
import { usePresentationScheduler } from "../../src/animation/frame-scheduler";
import { createTreeRootReveal } from "../../src/ui/create-tree-root-reveal";
import { TreeRowGlyphs, createTreeGlyphHost } from "../../src/ui/TreeRowGlyphs";
import { loadConfig } from "../../src/config";
import { createRecordingAppRenderer } from "./full-app-recording-renderer";

const config=loadConfig(["--config","/nonexistent/tree-root-reveal"],{});
const owner={},glyphs=createTreeGlyphHost(config),host=createRecordingAppRenderer();
const nativeRow=glyphs.row;let rasters=0;glyphs.row=args=>{rasters++;return nativeRow(args);};
const callbacks=new Set<()=>void>();
const restore=usePresentationScheduler(callback=>{callbacks.add(callback);return()=>{callbacks.delete(callback);};});
const frame={top:50,offset:-2,selected:52,cursorOffset:3};
const [input,setInput]=createSignal({key:'a:/repo',ready:true,total:1000,height:100,rowHeight:20,...frame,reducedMotion:false,active:true});
let root=createRoot(host.renderer);
function Ink(){
  const reveal=createTreeRootReveal(owner,"files",input,()=>config.scale);
  return <div testId="row" style={{position:"absolute",top:0,height:20,backgroundColor:config.theme.surface}}>
    <TreeRowGlyphs host={glyphs} config={config} name="index.ts" chevron="" icon="\uf15b" labelColor={config.theme.fg} iconColor={config.theme.fg} chevronColor={config.theme.muted} initialWidth={200} opacity={reveal(50).opacity} offsetX={reveal(50).offsetX}/>
  </div>;
}
const ink=()=>[...host.nodes.values()].find(node=>node.props["aria-label"]==="index.ts")!;
const render=()=>{root.render(()=><Ink/>);root.flush();};
try{
  render();assert.ok(Number(ink().style.opacity)<.1);assert.ok(Number(ink().style.left)>10*config.scale);assert.equal(callbacks.size,1);
  const commands=[...host.nodes.values()].find(node=>node.type==='canvas')?.props.commands;
  const rasterCount=rasters;
  await Bun.sleep(30);
  const pending=[...callbacks];callbacks.clear();for(const callback of pending)callback();root.flush();
  assert.ok(Number(ink().style.opacity)>0 && Number(ink().style.opacity)<1,"display frame advances the row's paint alpha");
  assert.ok(Number(ink().style.left)<12*config.scale,"display frame slides ink toward its resting location");
  setInput({...input(),key:'b:/repo',ready:false});root.flush();assert.equal(ink().style.opacity,1);assert.equal(callbacks.size,0);
  setInput({...input(),ready:true});root.flush();assert.ok(Number(ink().style.opacity)<.1);assert.equal(callbacks.size,1);
  setInput({...input(),active:false});root.flush();assert.equal(ink().style.opacity,1);assert.equal(ink().style.left,0);assert.equal(callbacks.size,0);
  setInput({...input(),active:true});root.flush();assert.equal(callbacks.size,0);
  assert.equal(rasters,rasterCount,"paint-only alpha/offset do not regenerate glyph rasters");
  assert.equal([...host.nodes.values()].find(node=>node.type==='canvas')?.props.commands,commands);
  assert.deepEqual(frame,{top:50,offset:-2,selected:52,cursorOffset:3});
  root.unmount();assert.equal(callbacks.size,0);
  root=createRoot(host.renderer);render();assert.equal(ink().style.opacity,1);assert.equal(callbacks.size,0);
  setInput({...input(),key:'a:/repo'});root.flush();assert.ok(Number(ink().style.opacity)<.1);assert.equal(callbacks.size,1);
  root.unmount();assert.equal(callbacks.size,0);
  console.log(JSON.stringify({noWindow:true,actualTreeGlyphs:true,rapidSwitch:true,backgroundSettles:true,generationSafe:true,viewportPreserved:true}));
}finally{root.unmount();glyphs.dispose();restore();}