#!/usr/bin/env node
// Launch the complete local production stack without replacing a running chat.
import {spawn} from 'node:child_process';
import {existsSync,mkdtempSync,openSync,closeSync} from 'node:fs';
import {createServer} from 'node:net';
import {tmpdir,homedir} from 'node:os';
import {dirname,isAbsolute,join,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';

const repository=fileURLToPath(new URL('..',import.meta.url));
const userConfig=process.argv.includes('--user-config');
const workspace=resolve(process.argv.slice(2).find(arg=>arg!=='--user-config')||repository);
const suffix=process.platform==='win32'?'.exe':'';
const binary=join(repository,`neoism-frontend/gpuix/dist/neoism${suffix}`);
const daemon=join(dirname(binary),`neoism-workspace-daemon${suffix}`);
if(!existsSync(binary)||!existsSync(daemon))throw new Error('Build and stage the production bundle first.');
const freePort=()=>new Promise((resolve,reject)=>{
 const server=createServer();server.once('error',reject);
 server.listen(0,'127.0.0.1',()=>{const port=server.address().port;server.close(error=>error?reject(error):resolve(port));});
});
const profile=mkdtempSync(join(tmpdir(),'neoism-local-production-'));
const env={...process.env};
const userState=env.XDG_STATE_HOME||join(homedir(),'.local/state');
const mcpAuth=env.NEOISM_AGENT_MCP_AUTH_PATH||join(env.NEOISM_AGENT_STATE_DIR||join(userState,'neoism'),'mcp-auth.json');
for(const key of Object.keys(env))if(key.startsWith('NEOISM_GPUIX_NATIVE_CHECK')||key.startsWith('NEOISM_GPUIX_TEST_'))delete env[key];
for(const key of ['NEOISM_GPUIX_DAEMON_URL','NEOISM_DAEMON_TOKEN','NEOISM_PAIRING_TOKEN','NEOISM_AGENT_TOKEN','NEOISM_AGENT_AUTH_CONFIG','NEOISM_WEB_ROOT','NEOISM_AGENT_GUI_ROOT'])delete env[key];
// Wayland/X11 still belong to the real desktop, not the isolated runtime dir.
if(env.WAYLAND_DISPLAY&&!isAbsolute(env.WAYLAND_DISPLAY)&&env.XDG_RUNTIME_DIR)env.WAYLAND_DISPLAY=join(env.XDG_RUNTIME_DIR,env.WAYLAND_DISPLAY);
if(!env.XAUTHORITY&&existsSync(join(homedir(),'.Xauthority')))env.XAUTHORITY=join(homedir(),'.Xauthority');
Object.assign(env,{
 ...(userConfig?{NEOISM_AGENT_MCP_AUTH_PATH:mcpAuth}:{
  XDG_CONFIG_HOME:join(profile,'config'),XDG_DATA_HOME:join(profile,'data'),XDG_STATE_HOME:join(profile,'state'),
  NEOISM_CONFIG_HOME:join(profile,'config/neoism'),NEOISM_CONFIG_DIR:join(profile,'config/neoism'),
  NEOISM_NOTES_HOME:join(profile,'notes'),
 }),
 XDG_RUNTIME_DIR:profile,
 NEOISM_STATE_DIR:join(profile,'state'),NEOISM_GPUIX_STATE_PATH:join(profile,'session.json'),
 NEOISM_DAEMON_DATA_DIR:join(profile,'daemon'),
 NEOISM_AGENT_STATE_DIR:join(profile,'agent'),NEOISM_DAEMON_SOCKET:join(profile,'neoism.sock'),
 NEOISM_DAEMON_ADDR:`127.0.0.1:${process.platform==='win32'?await freePort():0}`,
 NEOISM_AGENT_SERVER:`http://127.0.0.1:${await freePort()}`,
 NEOISM_GPUIX_WORKSPACE_DAEMON:daemon,NEOISM_LAUNCH_DIRECTORY:workspace,NEOISM_WORKSPACE_ROOT:workspace,
});
const log=join(profile,'app.log'),fd=openSync(log,'a',0o600);
let child;
try{
 child=spawn(binary,[],{cwd:workspace,env,detached:true,stdio:['ignore',fd,fd]});
 await new Promise((resolve,reject)=>{child.once('spawn',resolve);child.once('error',reject);});
 child.unref();
}finally{closeSync(fd);}
console.log(JSON.stringify({pid:child.pid,binary,workspace,profile,userConfig,agentUrl:env.NEOISM_AGENT_SERVER,log}));