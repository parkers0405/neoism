import {homedir} from "node:os";
import {posix,win32} from "node:path";

/** Desktop launchers can supply a root, system, or app-bundle cwd. Those are
 * launch machinery, not a project choice. Explicit CLI directories win. */
export function startupDirectory(env:NodeJS.ProcessEnv=process.env,options:{platform?:string;cwd?:string;home?:string;executable?:string}={}) {
  const platform=options.platform??process.platform,path=platform==="win32"?win32:posix;
  const home=options.home??homedir();
  let cwd=options.cwd;
  if(cwd===undefined){try{cwd=process.cwd();}catch{cwd=home;}}
  const explicit=env.NEOISM_LAUNCH_DIRECTORY||env.NEOISM_WORKSPACE_ROOT;
  if(explicit)return path.resolve(cwd,explicit);
  const normalized=path.normalize(cwd),root=path.parse(normalized).root;
  const same=(a:string,b:string)=>platform==="win32"?path.resolve(a).toLowerCase()===path.resolve(b).toLowerCase():path.resolve(a)===path.resolve(b);
  if((platform==="darwin"||platform==="win32")&&same(normalized,root))return home;
  if(platform==="darwin"&&/\.app\/Contents(?:\/|$)/i.test(normalized))return home;
  if(platform==="win32"){
    const windows=env.SystemRoot||env.WINDIR||"C:\\Windows";
    const executable=options.executable??process.execPath;
    if([windows,path.join(windows,"System32"),path.join(windows,"SysWOW64"),path.dirname(executable)].some(directory=>same(normalized,directory)))return home;
  }
  return normalized;
}

/** Old Finder defaults can survive in daemon and window recovery snapshots. */
export function recoveredStartupDirectory(root:string|undefined|null,env:NodeJS.ProcessEnv=process.env,options:Parameters<typeof startupDirectory>[1]={}) {
  if((options.platform??process.platform)!=="darwin"||env.NEOISM_LAUNCH_DIRECTORY||env.NEOISM_WORKSPACE_ROOT)return;
  if(root!=="/"&&!(root&&/\.app\/Contents(?:\/|$)/i.test(root)))return;
  const home=options.home??homedir();
  if(startupDirectory(env,options)===home)return home;
}
