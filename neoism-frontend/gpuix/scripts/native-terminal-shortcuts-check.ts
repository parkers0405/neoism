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
writeFileSync(config,JSON.stringify({appearance:{fonts:{size:14}},keybinds:{keys:[{key:"c",with:"Control",action:"Copy"},{key:"v",with:"Command",action:"None"}]}}));
const server=createAgentDaemon({deferCreations:false});
const app=await launchNative({openWorkspaceIds:["a"],command:process.execPath,args:["src/main.tsx","--daemon",server.endpoint,"--workspace","a","--config",config],cwd:process.cwd(),env:{...process.env,PATH:bin+":"+process.env.PATH,NEOISM_TEST_CLIPBOARD:clipboard,NEOISM_GPUIX_STATE_PATH:join(directory,"state.json")}});
const until=async(check:()=>boolean|Promise<boolean>)=>{const end=Date.now()+10000;while(!await check()){if(Date.now()>end)throw new Error("Terminal shortcut check timed out");await Bun.sleep(20);}};
const input=()=>server.ptyRequests.filter(row=>row.tag==="PtyInput").flatMap(row=>row.body.bytes as number[]);
let stage="open terminal";
try{
 await app.getByTestId("original-topbar").waitFor({timeoutMs:15000});await app.call("keystrokes",{keys:"alt-t"});
 const terminal=app.getByTestId("terminal");await terminal.waitFor();await until(()=>server.ptyRequests.some(row=>row.tag==="AttachPty"));
 server.pushPty({PtyOutput:{session_id:"fixture-pty",bytes:[...new TextEncoder().encode("\x1b[2J\x1b[HSELECT_ME\r\n\x1b[?2004h")]}});await Bun.sleep(150);
 stage="Command+C copies terminal selection";
 const box=await terminal.bounds();await app.mouse.down({x:box.x+1,y:box.y+8});await app.mouse.move({x:box.x+180,y:box.y+8},{pressedButton:0});await app.mouse.up({x:box.x+180,y:box.y+8});
 await terminal.press("cmd-c");await until(()=>existsSync(clipboard)&&readFileSync(clipboard,"utf8").trimEnd()==="SELECT_ME");
 if(input().length)throw new Error("Copy leaked keystrokes to PTY");
 stage="Control+C bypasses global Copy with selection";
 await terminal.press("ctrl-c");await until(()=>input().length>0);if(JSON.stringify(input())!=="[3]")throw new Error(`Wrong interrupt bytes: ${input()}`);
 server.ptyRequests.splice(0);
 stage="Command+V pastes once using Ghostty bracketed paste despite global None";
 writeFileSync(clipboard,"pasted one\npasted two");await terminal.press("cmd-v");await until(()=>input().length>0);
 if(new TextDecoder().decode(Uint8Array.from(input()))!=="\x1b[200~pasted one\npasted two\x1b[201~")throw new Error(`Wrong paste: ${input()}`);
 server.ptyRequests.splice(0);await terminal.press("ctrl-v");await until(()=>input().length>0);if(JSON.stringify(input())!=="[22]")throw new Error("Plain Ctrl+V lost its shell meaning");
 console.log(JSON.stringify({ok:true,checks:["native selected terminal copy","Ctrl+C is interrupt despite global Copy","Command+V brackets exact clipboard bytes once","Ctrl+V remains shell input"]}));
}catch(error){console.error(`Terminal shortcut check failed at ${stage}`);throw error;}
finally{await app.close();server.stop();rmSync(directory,{recursive:true,force:true});}
