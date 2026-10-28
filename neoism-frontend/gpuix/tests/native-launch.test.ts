import {afterEach,expect,test} from "bun:test";
import {mkdtempSync,readFileSync,rmSync,writeFileSync,existsSync,statSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {decodeSession,SessionStore} from "../src/session-store";
import {seedNativeWorkspaceSession,type NativeLaunchOptions} from "./support/native-launch";
const directories:string[]=[];afterEach(()=>{for(const path of directories.splice(0))rmSync(path,{recursive:true,force:true});});
function fixture():NativeLaunchOptions{
 const cwd=mkdtempSync(join(tmpdir(),"neoism-native-launch-seed-"));directories.push(cwd);
 return {command:process.execPath,args:["src/main.tsx","--workspace","a"],cwd,env:{NEOISM_GPUIX_STATE_PATH:"state/recovery.json"},openWorkspaceIds:["b","a"]};
}
test("native fixture opt-in seeds the actual SavedSession schema and explicit membership in caller order",()=>{
 const options=fixture();expect(seedNativeWorkspaceSession(options)).toBe(true);
 const path=join(options.cwd,options.env!.NEOISM_GPUIX_STATE_PATH!);
 const value=JSON.parse(readFileSync(path,"utf8")),decoded=decodeSession(value)!;
 expect(decoded).toBeDefined();expect(decoded.ui.openWorkspaceIds).toEqual(["b","a"]);expect(decoded.activeWorkspaceId).toBe("a");expect(decoded.surfaces).toEqual([]);
 expect(decoded).toMatchObject({version:1,editors:{},providerStates:{},drafts:{},composerDrafts:{}});
 expect(new SessionStore({endpoint:"ws://fixture/session"},{NEOISM_GPUIX_STATE_PATH:path}).read()?.ui.openWorkspaceIds).toEqual(["b","a"]);
 if(process.platform!=="win32")expect(statSync(path).mode&0o777).toBe(0o600);
});
test("relaunch preserves existing session bytes including closed membership and drafts",()=>{
 const options=fixture();seedNativeWorkspaceSession(options);const path=join(options.cwd,options.env!.NEOISM_GPUIX_STATE_PATH!);
 const persisted='{"version":1,"surfaces":[],"ui":{"openWorkspaceIds":["a"]},"composerDrafts":{"one":{"text":"keep draft"}},"profiles":{"keep":"existing"}}\n';
 writeFileSync(path,persisted);expect(seedNativeWorkspaceSession({...options,openWorkspaceIds:["a","b","other"]})).toBe(false);expect(readFileSync(path,"utf8")).toBe(persisted);
});
test("native launches without the option never seed workspace membership and opt-in requires an isolated state path",()=>{
 const options=fixture(),path=join(options.cwd,options.env!.NEOISM_GPUIX_STATE_PATH!);
 expect(seedNativeWorkspaceSession({...options,openWorkspaceIds:undefined})).toBe(false);expect(existsSync(path)).toBe(false);
 expect(()=>seedNativeWorkspaceSession({...options,env:{}})).toThrow("explicit NEOISM_GPUIX_STATE_PATH");expect(existsSync(path)).toBe(false);
});
