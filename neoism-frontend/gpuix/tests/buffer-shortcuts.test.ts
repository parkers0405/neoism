import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
import {bufferTabShortcut} from "../src/commands/chrome-keyboard";
test("desktop CtrlShift horizontal defaults require exactly their own modifiers",()=>{
 expect(bufferTabShortcut({key:"ArrowLeft",modifiers:{ctrl:true,shift:true}})).toBe("previous");expect(bufferTabShortcut({key:"right",modifiers:{ctrl:true,shift:true}})).toBe("next");
 for(const key of ["up","down"])expect(bufferTabShortcut({key,modifiers:{ctrl:true,shift:true}})).toBeUndefined();
 for(const modifiers of [{ctrl:true},{shift:true},{ctrl:true,shift:true,alt:true},{ctrl:true,shift:true,cmd:true},{cmd:true,shift:true}])expect(bufferTabShortcut({key:"left",modifiers})).toBeUndefined();
});
test("actual App restores CtrlShift arrows across buffer types and split panes",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/buffer-shortcuts-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});
 const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({noWindow:true,actualApp:true,allBufferTypesCycle:true,wrapsBothDirections:true,selectsWithoutReordering:true,altFocusUnchanged:true,stripFocusRetained:true,verticalEditingNotIntercepted:true,splitScoped:true,dirtyBytesPreserved:true,actualHostTypes:8,agentHistoryAndPicker:true,commandAndFinderInputs:true,noShellInputOrPrompt:true});
},15000);
