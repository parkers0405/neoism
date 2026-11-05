import {expect,test} from "bun:test";
import {startupDirectory,recoveredStartupDirectory} from "../src/daemon/startup-directory";
import {localDaemonArgs} from "../src/daemon/local-startup";

test("Finder launches start in the user's home, while project and explicit root launches keep their directory",()=>{
 const options={platform:"darwin",home:"/Users/parkersettle",executable:"/Applications/Neoism.app/Contents/MacOS/neoism"};
 for(const cwd of ["/","/Applications/Neoism.app/Contents/MacOS"])
  expect(startupDirectory({},{...options,cwd})).toBe(options.home);
 expect(startupDirectory({},{...options,cwd:"/Users/parkersettle/projects/work"})).toBe("/Users/parkersettle/projects/work");
 expect(startupDirectory({NEOISM_LAUNCH_DIRECTORY:"/"},{...options,cwd:"/"})).toBe("/");
  expect(startupDirectory({NEOISM_LAUNCH_DIRECTORY:"/work/project"},{...options,cwd:"/"})).toBe("/work/project");
  expect(startupDirectory({NEOISM_WORKSPACE_ROOT:"/"},{...options,cwd:"/"})).toBe("/");
  expect(startupDirectory({NEOISM_WORKSPACE_ROOT:"/work/project"},{...options,cwd:"/"})).toBe("/work/project");
  expect(startupDirectory({NEOISM_WORKSPACE_ROOT:"/work/project",NEOISM_LAUNCH_DIRECTORY:"/selected"},{...options,cwd:"/"})).toBe("/selected");
});

test("Windows shell launch directories use the user profile, respecting drive letters and case",()=>{
 const options={platform:"win32",home:"C:\\Users\\Parker",executable:"C:\\Program Files\\Neoism\\neoism.exe"};
 for(const cwd of ["C:\\","D:\\","C:\\Windows","c:\\windows\\SYSTEM32","C:\\Windows\\SysWOW64","C:\\Program Files\\Neoism"])
  expect(startupDirectory({SystemRoot:"C:\\Windows"},{...options,cwd})).toBe(options.home);
 expect(startupDirectory({},{...options,cwd:"D:\\projects\\work"})).toBe("D:\\projects\\work");
 expect(startupDirectory({NEOISM_LAUNCH_DIRECTORY:"C:\\"},{...options,cwd:"C:\\Windows"})).toBe("C:\\");
});

test("Linux and explicit launch directories retain their existing behavior",()=>{
 expect(startupDirectory({},{platform:"linux",cwd:"/",home:"/home/parker"})).toBe("/");
 expect(startupDirectory({NEOISM_LAUNCH_DIRECTORY:"child"},{platform:"linux",cwd:"/project",home:"/home/parker"})).toBe("/project/child");
});

test("Windows defaults use USERPROFILE without HOME, on any drive and with spaces",()=>{
 const env={USERPROFILE:"E:\\Users\\Parker Settle",SystemRoot:"D:\\Windows"};
 const options={platform:"win32",executable:"F:\\Apps\\Neoism\\neoism.exe"};
 for(const cwd of ["D:\\","d:\\WINDOWS\\system32","D:\\Windows\\SysWOW64","F:\\Apps\\Neoism"]){
  const directory=startupDirectory(env,{...options,cwd});
  expect(directory).toBe(env.USERPROFILE);
   expect(localDaemonArgs(directory,env,123,"win32")).toEqual(["--workspace",env.USERPROFILE,"--auto-port","--auto-agent-port"]);
 }
 expect(startupDirectory({...env,HOME:"C:\\wrong"},{...options,cwd:"D:\\Windows"})).toBe(env.USERPROFILE);
 expect(startupDirectory({USERPROFILE:env.USERPROFILE,WINDIR:"D:\\Windows"},{...options,cwd:"D:\\Windows\\System32"})).toBe(env.USERPROFILE);
 expect(startupDirectory(env,{...options,cwd:"E:\\projects\\my project"})).toBe("E:\\projects\\my project");
 // A Windows installation is not assumed to live on C:.
 expect(startupDirectory(env,{...options,cwd:"C:\\Windows"})).toBe("C:\\Windows");
 for(const key of ["NEOISM_LAUNCH_DIRECTORY","NEOISM_WORKSPACE_ROOT"]){
  for(const selected of ["D:\\","D:\\Windows\\System32","F:\\Apps\\Neoism","E:\\projects\\my project"])
   expect(startupDirectory({...env,[key]:selected},{...options,cwd:"D:\\Windows"})).toBe(selected);
  expect(startupDirectory({...env,[key]:"child folder"},{...options,cwd:"E:\\project"})).toBe("E:\\project\\child folder");
 }
});

test("Windows recovery corrects only implicit launcher roots, not meaningful or explicit selections",()=>{
 const env={USERPROFILE:"E:\\Users\\Parker Settle",SystemRoot:"D:\\Windows"};
 const options={platform:"win32",cwd:"D:\\Windows\\System32",executable:"F:\\Apps\\Neoism\\neoism.exe"};
 for(const root of ["D:\\","D:\\Windows","d:\\WINDOWS\\system32","D:\\Windows\\SysWOW64","F:\\Apps\\Neoism"]){
  expect(recoveredStartupDirectory(root,env,options)).toBe(env.USERPROFILE);
  expect(recoveredStartupDirectory(root,env,{...options,cwd:env.USERPROFILE})).toBe(env.USERPROFILE);
  expect(recoveredStartupDirectory(root,env,{...options,cwd:"E:\\project"})).toBeUndefined();
  for(const key of ["NEOISM_LAUNCH_DIRECTORY","NEOISM_WORKSPACE_ROOT"])
   expect(recoveredStartupDirectory(root,{...env,[key]:root},options)).toBeUndefined();
 }
 for(const root of [undefined,null,"","E:\\projects\\my project",env.USERPROFILE,"D:\\Windows\\my project","F:\\Apps\\Neoism\\my project"])
  expect(recoveredStartupDirectory(root,env,options)).toBeUndefined();
 expect(recoveredStartupDirectory(env.USERPROFILE,env,{...options,executable:env.USERPROFILE+"\\neoism.exe"})).toBeUndefined();
});

test("only implicit macOS home launches recover old root and app-bundle defaults",()=>{
 const options={platform:"darwin",cwd:"/",home:"/Users/person"};
 for(const root of ["/","/Applications/Neoism.app/Contents/MacOS"])
  expect(recoveredStartupDirectory(root,{},options)).toBe(options.home);
 expect(recoveredStartupDirectory("/",{},{...options,cwd:options.home})).toBe(options.home);
 expect(recoveredStartupDirectory("/project",{},options)).toBeUndefined();
 expect(recoveredStartupDirectory("/",{NEOISM_LAUNCH_DIRECTORY:"/"},options)).toBeUndefined();
 expect(recoveredStartupDirectory("/",{NEOISM_WORKSPACE_ROOT:"/"},options)).toBeUndefined();
 expect(recoveredStartupDirectory("/",{},{...options,cwd:"/project"})).toBeUndefined();
 expect(recoveredStartupDirectory("/",{},{...options,platform:"linux"})).toBeUndefined();
});
