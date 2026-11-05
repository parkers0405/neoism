import {expect,test} from "bun:test";
import {startupDirectory,recoveredStartupDirectory} from "../src/daemon/startup-directory";

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
