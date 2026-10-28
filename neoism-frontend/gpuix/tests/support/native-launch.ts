import { spawn } from "node:child_process";
import {mkdirSync,writeFileSync} from "node:fs";
import {dirname,resolve} from "node:path";
import type {SavedSession} from "../../src/session-store";
import { connectStdio } from "@gpuix/solid/automation";
export interface NativeLaunchOptions{command:string;args:string[];cwd:string;env?:NodeJS.ProcessEnv;openWorkspaceIds?:readonly string[]}
/** Fixture-only membership is explicit and never overwrites a relaunch's
 * recovery file. Production workspace defaults remain in the real model. */
export function seedNativeWorkspaceSession(options:NativeLaunchOptions):boolean{
 if(options.openWorkspaceIds===undefined)return false;
 const path=options.env?.NEOISM_GPUIX_STATE_PATH;
 if(!path)throw new Error("openWorkspaceIds requires an explicit NEOISM_GPUIX_STATE_PATH fixture file");
 const ids=[...new Set(options.openWorkspaceIds)];
 const index=options.args.indexOf("--workspace"),active=index>=0?options.args[index+1]:ids[0];
 const session:SavedSession={version:1,activeWorkspaceId:active,surfaces:[],editors:{},providerStates:{},drafts:{},composerDrafts:{},ui:{openWorkspaceIds:ids}};
 const file=resolve(options.cwd,path);mkdirSync(dirname(file),{recursive:true,mode:0o700});
 try{writeFileSync(file,JSON.stringify(session),{flag:"wx",mode:0o600});return true;}
 catch(error){if((error as NodeJS.ErrnoException).code==="EEXIST")return false;throw error;}
}

// Keep native stderr visible: otherwise assertion timeouts hide Rust/JS errors.
export async function launchNative(options:NativeLaunchOptions) {
  seedNativeWorkspaceSession(options);
  const child = spawn(options.command, options.args, { cwd: options.cwd, env: { ...process.env, ...options.env }, stdio: ["pipe", "pipe", "inherit"] });
  return connectStdio({
    write: (chunk) => { child.stdin.write(chunk); },
    feed: (listener) => { child.stdout.on("data", (data) => listener(data.toString("utf8"))); },
    close: async () => {
      if (child.exitCode !== null || child.signalCode !== null) return;
      await new Promise<void>((resolve) => {
        const timeout = setTimeout(() => child.kill("SIGKILL"), 3000);
        child.once("exit", () => { clearTimeout(timeout); resolve(); });
        child.kill("SIGTERM");
      });
    },
  });
}
