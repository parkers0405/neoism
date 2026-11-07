import {mkdtempSync,mkdirSync,rmSync,writeFileSync,readFileSync,existsSync} from "node:fs";
import {join} from "node:path";
import {tmpdir} from "node:os";
import {launchNative} from "../tests/support/native-launch";
import {createAgentDaemon} from "../tests/support/agent-daemon";
if(process.platform==="win32")throw new Error("This clipboard-process fixture requires a POSIX shell");
const directory=mkdtempSync(join(tmpdir(),"neoism-terminal-clipboard-")),bin=join(directory,"bin"),clipboard=join(directory,"clipboard"),config=join(directory,"config.json");mkdirSync(bin);
// Exercise the host clipboard process boundary without replacing the user's clipboard.
for(const command of ["wl-copy","pbcopy"])writeFileSync(join(bin,command),'#!/bin/sh\ncat > "$NEOISM_TEST_CLIPBOARD"\n',{mode:0o700});
for(const command of ["wl-paste","pbpaste"])writeFileSync(join(bin,command),'#!/bin/sh\ncat "$NEOISM_TEST_CLIPBOARD"\n',{mode:0o700});
writeFileSync(join(bin,"xclip"),'#!/bin/sh\ncase "$*" in *-o*) cat "$NEOISM_TEST_CLIPBOARD";; *) cat > "$NEOISM_TEST_CLIPBOARD";; esac\n',{mode:0o700});
writeFileSync(config,JSON.stringify({appearance:{fonts:{size:14}},keybinds:{keys:[{key:"c",with:"Control",action:"Copy"},{key:"v",with:"Control",action:"Paste"},{key:"v",with:"Command",action:"None"}]}}));
const server=createAgentDaemon({deferCreations:false});
const app=await launchNative({openWorkspaceIds:["a"],command:process.execPath,args:["src/main.tsx","--daemon",server.endpoint,"--workspace","a","--config",config],cwd:process.cwd(),env:{...process.env,PATH:bin+":"+process.env.PATH,NEOISM_TEST_CLIPBOARD:clipboard,NEOISM_GPUIX_STATE_PATH:join(directory,"state.json")}});
const until=async(check:()=>boolean|Promise<boolean>)=>{const end=Date.now()+10000;while(!await check()){if(Date.now()>end)throw new Error("Terminal shortcut check timed out");await Bun.sleep(20);}};
const input=()=>server.ptyRequests.filter(row=>row.tag==="PtyInput").flatMap(row=>row.body.bytes as number[]);
let stage="open terminal";
try{
 await app.getByTestId("activity-rail").waitFor({timeoutMs:15000});await app.call("keystrokes",{keys:"alt-t"});
 const terminal=app.getByTestId("terminal");await terminal.waitFor();await until(()=>server.ptyRequests.some(row=>row.tag==="AttachPty"));
 server.pushPty({PtyOutput:{session_id:"fixture-pty",bytes:[...new TextEncoder().encode("\x1b[2J\x1b[HSELECT_ME\r\n\x1b[?2004h")]}});await Bun.sleep(150);
 stage="Command+C copies terminal selection";
 const box=await terminal.bounds();await app.mouse.down({x:box.x+1,y:box.y+8});await app.mouse.move({x:box.x+180,y:box.y+8},{pressedButton:0});await app.mouse.up({x:box.x+180,y:box.y+8});
 await terminal.press("cmd-c");await until(()=>existsSync(clipboard)&&readFileSync(clipboard,"utf8").trimEnd()==="SELECT_ME");
 if(input().length)throw new Error("Copy leaked keystrokes to PTY");
 stage="Control+C respects configured Copy with selection";
 writeFileSync(clipboard,"");await terminal.press("ctrl-c");await until(()=>readFileSync(clipboard,"utf8").trimEnd()==="SELECT_ME");
 if(input().length)throw new Error("Configured Copy leaked an interrupt to PTY");
 server.ptyRequests.splice(0);
 stage="Control+V respects configured Paste using Ghostty bracketed paste";
 writeFileSync(clipboard,"pasted one\npasted two");await terminal.press("ctrl-v");await until(()=>input().length>0);
 if(new TextDecoder().decode(Uint8Array.from(input()))!=="\x1b[200~pasted one\npasted two\x1b[201~")throw new Error(`Wrong paste: ${input()}`);
 server.ptyRequests.splice(0);await terminal.press("cmd-v");await Bun.sleep(150);if(input().length)throw new Error("Explicit None was ignored by terminal paste");
 stage="unbound Control+Shift+V retains terminal paste fallback";
 await terminal.press("ctrl-shift-v");await until(()=>input().length>0);
 if(new TextDecoder().decode(Uint8Array.from(input()))!=="\x1b[200~pasted one\npasted two\x1b[201~")throw new Error("Terminal paste fallback failed");
  server.ptyRequests.splice(0);
  stage="Hyprland universal paste arrives as Shift+Insert";
  await terminal.press("shift-insert");await until(()=>input().length>0);
  if(new TextDecoder().decode(Uint8Array.from(input()))!=="\x1b[200~pasted one\npasted two\x1b[201~")throw new Error(`Compositor paste leaked Insert instead of clipboard: ${input()}`);
  server.ptyRequests.splice(0);
  for(const chord of ["cmd-p","alt-p","ctrl-shift-p"]){
   stage=`${chord} opens the command modal from the terminal`;
   await terminal.press(chord);await app.getByTestId("command-dialog").waitFor();
   if(await app.getByTestId("finder").count())throw new Error(`${chord} opened file search`);
   await app.getByTestId("command-palette").press("escape");await terminal.waitFor();
  }
  if(input().length)throw new Error("Command modal shortcuts leaked input to PTY");
  stage="Control+P remains file search";
  await terminal.press("ctrl-p");await app.getByTestId("finder").waitFor();
  if(await app.getByTestId("command-dialog").count())throw new Error("Control+P unexpectedly opened commands");
  console.log(JSON.stringify({ok:true,checks:["native selected terminal copy","Ctrl+C respects configured Copy","Ctrl+V brackets exact clipboard bytes once","explicit None disables default paste","unbound terminal clipboard fallback","Shift+Insert pastes without escape leakage","Command+P, Alt+P and Ctrl+Shift+P open command modal","Ctrl+P remains file search"]}));
}catch(error){console.error(`Terminal shortcut check failed at ${stage}`);throw error;}
finally{await app.close();server.stop();rmSync(directory,{recursive:true,force:true});}
