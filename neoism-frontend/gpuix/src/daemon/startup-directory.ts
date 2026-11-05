import {homedir} from "node:os";
import {posix,win32} from "node:path";

type StartupOptions={platform?:string;cwd?:string;home?:string;executable?:string};
function startupHome(env:NodeJS.ProcessEnv,options:StartupOptions){
  return options.home??(((options.platform??process.platform)==="win32"?env.USERPROFILE:undefined)||homedir());
}
function windowsLauncherDirectory(directory:string,env:NodeJS.ProcessEnv,executable:string){
  const normalized=win32.resolve(directory).toLowerCase();
  const windows=env.SystemRoot||env.WINDIR;
  const candidates=[win32.parse(normalized).root,win32.dirname(executable)];
  if(windows)candidates.push(windows,win32.join(windows,"System32"),win32.join(windows,"SysWOW64"));
  return candidates.some(candidate=>win32.resolve(candidate).toLowerCase()===normalized);
}

/** Desktop launchers can supply a root, system, or app-bundle cwd. Those are
 * launch machinery, not a project choice. Explicit CLI directories win. */
export function startupDirectory(env:NodeJS.ProcessEnv=process.env,options:StartupOptions={}) {
  const platform=options.platform??process.platform,path=platform==="win32"?win32:posix;
  const home=startupHome(env,options);
  let cwd=options.cwd;
  if(cwd===undefined){try{cwd=process.cwd();}catch{cwd=home;}}
  const explicit=env.NEOISM_LAUNCH_DIRECTORY||env.NEOISM_WORKSPACE_ROOT;
  if(explicit)return path.resolve(cwd,explicit);
  const normalized=path.normalize(cwd),root=path.parse(normalized).root;
  const same=(a:string,b:string)=>platform==="win32"?path.resolve(a).toLowerCase()===path.resolve(b).toLowerCase():path.resolve(a)===path.resolve(b);
  if((platform==="darwin"||platform==="win32")&&same(normalized,root))return home;
  if(platform==="darwin"&&/\.app\/Contents(?:\/|$)/i.test(normalized))return home;
  if(platform==="win32"){
    const executable=options.executable??process.execPath;
    if(windowsLauncherDirectory(normalized,env,executable))return home;
  }
  return normalized;
}

/** Old desktop launcher defaults can survive in daemon and window recovery snapshots.
 * The caller protects explicit workspace selections and retains the old workspace. */
export function recoveredStartupDirectory(root:string|undefined|null,env:NodeJS.ProcessEnv=process.env,options:Parameters<typeof startupDirectory>[1]={}) {
  const platform=options.platform??process.platform;
  if(env.NEOISM_LAUNCH_DIRECTORY||env.NEOISM_WORKSPACE_ROOT)return;
  if(platform==="darwin"){
    if(root!=="/"&&!(root&&/\.app\/Contents(?:\/|$)/i.test(root)))return;
  }else if(platform==="win32"){
    if(!root||!win32.isAbsolute(root)||!windowsLauncherDirectory(root,env,options.executable??process.execPath))return;
  }else return;
  const home=startupHome(env,options);
  if(platform==="win32"&&root&&win32.resolve(root).toLowerCase()===win32.resolve(home).toLowerCase())return;
  if(startupDirectory(env,options)===home)return home;
}
