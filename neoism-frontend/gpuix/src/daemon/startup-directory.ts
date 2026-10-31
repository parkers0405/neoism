import {homedir} from "node:os";
import {posix,win32} from "node:path";

/** Desktop launchers can supply a root, system, or app-bundle cwd. Those are
 * launch machinery, not a project choice. Explicit CLI directories win. */
export function startupDirectory(env:NodeJS.ProcessEnv=process.env,options:{platform?:string;cwd?:string;home?:string;executable?:string}={}) {
  const platform=options.platform??process.platform,path=platform==="win32"?win32:posix;
  const home=options.home??homedir();
  let cwd=options.cwd;
  if(cwd===undefined){try{cwd=process.cwd();}catch{cwd=home;}}
  if(env.NEOISM_LAUNCH_DIRECTORY)return path.resolve(cwd,env.NEOISM_LAUNCH_DIRECTORY);
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
