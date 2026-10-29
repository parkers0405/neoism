import assert from"node:assert/strict";
import{createRoot}from"@gpuix/solid";
import{GitRestoreDialog}from"../../src/git/GitRestoreDialog";
import{setOverlayViewport}from"../../src/ui/overlay-viewport";
import{loadConfig}from"../../src/config";
import{createRecordingAppRenderer}from"./full-app-recording-renderer";
const config=loadConfig(["--config","/nonexistent/gpuix-git-restore-test"],{}),area={x:287,y:149,width:805,height:590};
for(const route of["confirm","escape","outside"]){const host=createRecordingAppRenderer(),root=createRoot(host.renderer,{}),answers:boolean[]=[];setOverlayViewport(host.renderer,area);try{
 root.render(()=><GitRestoreDialog target={{root:"/fixture/repo",path:"src/a.rs",block:{old_start:1,new_start:1,old_lines:["before"],new_lines:["after"]}}} config={config} onDone={answer=>answers.push(answer)}/>);await host.pump(root);const dialog=host.find("git-restore-confirmation");assert.ok(dialog);assert.equal(host.focused,dialog.id);assert.equal(dialog.props.role,"dialog");assert.equal(dialog.style.left,0);assert.equal(dialog.style.top,area.y);assert.equal(dialog.style.width,1440);assert.equal(dialog.style.height,900-area.y);const canvas=host.find("original-paint-layer");assert.ok(canvas);const commands=JSON.parse(String(canvas.props.commands));assert.ok(commands.some((op:{k:string})=>op.k==="i"),"original font glyph rasters are actually uploaded to native canvas commands");
 if(route==="outside")root.dispatch({elementId:dialog.id,eventType:"mouseDown",button:0,x:1,y:area.y+1});else root.dispatch({elementId:dialog.id,eventType:"keyDown",key:route==="confirm"?"enter":"escape",keyChar:"",modifiers:{alt:false,ctrl:false,shift:false,cmd:false}});await host.pump(root);assert.deepEqual(answers,[route==="confirm"]);
 root.dispatch({elementId:dialog.id,eventType:"keyDown",key:"enter",keyChar:"",modifiers:{alt:false,ctrl:false,shift:false,cmd:false}});await host.pump(root);assert.equal(answers.length,1,"one decision even after repeated native event");
 }finally{root.unmount();}}
console.log(JSON.stringify({noWindow:true,actualSolid:true,originalPainter:true,windowBoundsBelowChrome:true,confirmCancelAndPointer:true,oneDecision:true,noGitIO:true}));
