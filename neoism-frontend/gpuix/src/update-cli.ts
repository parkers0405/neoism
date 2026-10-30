import {existsSync,realpathSync} from "node:fs";
import {dirname,join} from "node:path";

export function updateArguments(args:readonly string[]):string[]|undefined {
 if(!["update","upgrade","self-update","--update","--macos-update-helper"].includes(args[0]||""))return;
 return [...args];
}
export function updaterPath(executable:string,platform:NodeJS.Platform=process.platform):string {
 const root=dirname(realpathSync(executable)),name=`neoism-update-helper${platform==="win32"?".exe":""}`;
 const paths=[join(root,"web/native",name),join(root,"../Resources/web/native",name)];
 const path=paths.find(existsSync);
 if(!path)throw Error("The installation is missing its update helper. Reinstall the complete Neoism bundle.");
 return path;
}
export async function runUpdate(args:string[]):Promise<number>{
 const helper=updaterPath(process.execPath);
 const child=Bun.spawn([helper,...args,...(args[0]==="--macos-update-helper"?[]:["--invoking-exe",realpathSync(process.execPath),"--parent-pid",String(process.pid)])],{stdin:"inherit",stdout:"inherit",stderr:"inherit"});
 return await child.exited;
}
