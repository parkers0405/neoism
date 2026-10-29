import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {EditorView} from "../../src/editor/EditorView";
import {NativeEditor} from "../../src/native-core";
import {loadConfig} from "../../src/config";
import {cssColor,type CanvasOp} from "../../src/ui/canvas";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";

const config={...loadConfig(["--config","/nonexistent/diagnostic-underlines"],{}),cursorBlink:false};
const editor=new NativeEditor("diagnostics.rs","let first = missing;\nlet second = unused;\n"+"long_name + ".repeat(40),true);
const host=createRecordingAppRenderer(),root=createRoot(host.renderer),path="/project/diagnostics.rs";
const ops=()=>JSON.parse(String(host.find("code-viewport")?.props.commands||"[]")) as CanvasOp[];
const diagnostics=(items:unknown[])=>editor.call("lsp.reply",{message:{Diagnostics:{file_path:path,items}}});
try{
 root.render(()=><EditorView editor={editor} config={config} active={false} focusToken={0} prompt={()=>{}} save={async()=>{}} report={error=>{throw error;}} onState={()=>{}}/>);
 await host.pump(root);
 editor.call("layout",{width:240,height:680,cellWidth:9,rowHeight:21});
 editor.call("lsp.enable",{surface:"underline-test",path,openPaths:[path]});
 diagnostics([
  {severity:"Error",message:"Unresolved name",line:0,col:12,end_line:0,end_col:19,lnum:1,source:"fixture"},
  {severity:"Warn",message:"Unused binding",line:1,col:13,end_line:1,end_col:19,lnum:2,source:"fixture"},
  {severity:"Warn",message:"Wrapped range",line:2,col:0,end_line:2,end_col:150,lnum:3,source:"fixture"},
 ]);
 root.flush();
 const frame=editor.frame,paint=ops(),bands=paint.filter(op=>op.k==="r"&&op.b[3]===2);
 assert.ok(frame.lines.some(line=>line.continuation),"Fixture must exercise diagnostic range across visual wraps");
 const runs=frame.lines.flatMap(line=>line.runs.filter(run=>run.severity).map(run=>({line,run})));
 assert.ok(runs.some(({run})=>run.severity==="Error"));assert.ok(runs.some(({run})=>run.severity==="Warn"));
 assert.equal(bands.length,runs.length,"Each original diagnostic run must have exactly one band");
 for(const {line,run} of runs){
  const color=cssColor(run.severity==="Error"?config.theme.red:config.theme.yellow);
  assert.ok(bands.some(op=>op.k==="r"&&op.c===color&&op.b[0]===run.x&&op.b[1]===line.y+frame.rowHeight-3&&op.b[2]===Math.max(1,run.width||run.text.length*frame.cellWidth)),"Band must preserve severity ink, exact run width and original row baseline");
 }
 assert.equal(paint.some(op=>op.k==="l"),false,"Diagnostic paint must contain no zigzag polyline");
 assert.ok(paint.some(op=>op.k==="i"),"Straight diagnostic bands must leave cached syntax glyph image painting intact");
 diagnostics([]);root.flush();assert.equal(ops().filter(op=>op.k==="r"&&op.b[3]===2).length,0,"Clearing diagnostics must remove bands without a remount");
 console.log(JSON.stringify({noWindow:true,actualNativeDiagnostics:true,straightErrorAndWarning:true,exactRunGeometry:true,wrappedRows:true,glyphImagesRetained:true,clearedDiagnosticsRemoveBands:true}));
}finally{root.unmount();editor.dispose();}
