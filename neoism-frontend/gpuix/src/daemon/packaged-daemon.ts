import {accessSync,chmodSync,constants,copyFileSync,existsSync,mkdirSync,renameSync,statSync} from "node:fs";
import {join,resolve} from "node:path";
/** Bundle the standalone host, never overwrite or restart the running source
 * daemon. Replace only the distribution inode after the complete copy lands. */
export function packageWorkspaceDaemon(root:string,env:NodeJS.ProcessEnv=process.env,platform:NodeJS.Platform=process.platform){
 const suffix=platform==="win32"?".exe":"";
 const candidates=env.NEOISM_GPUIX_WORKSPACE_DAEMON?[resolve(root,env.NEOISM_GPUIX_WORKSPACE_DAEMON)]:[join(root,"artifacts",`neoism-workspace-daemon${suffix}`),join(root,"../../target/activation",`neoism-workspace-daemon${suffix}`)];
 const source=candidates.find(path=>existsSync(path)&&statSync(path).isFile());
 if(!source)throw new Error("The workspace daemon is missing. Build the workspace daemon with make build or set NEOISM_GPUIX_WORKSPACE_DAEMON before packaging server support.");
 if(platform!=="win32")try{accessSync(source,constants.X_OK);}catch{throw new Error("The workspace daemon must be executable before packaging server support.");}
 const output=join(root,"dist",`neoism-workspace-daemon${suffix}`);mkdirSync(join(root,"dist"),{recursive:true});
 copyFileSync(source,`${output}.next`);chmodSync(`${output}.next`,statSync(source).mode&0o777);renameSync(`${output}.next`,output);
 return output;
}
