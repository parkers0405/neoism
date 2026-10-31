import {expect,test} from "bun:test";
import {loadConfig} from "../src/config";
import {DaemonConnection} from "../src/daemon/client";
import {WorkspaceModel,type Surface} from "../src/model";
import {hostPaths} from "../src/workspaces/host-paths";

test("editor identities and locations retain Windows drive, UNC and verbatim workspace roots",async()=>{
 const config=loadConfig(["--config","/nonexistent/host-path-test"],{}),client=new DaemonConnection(config),model=new WorkspaceModel(client,config);
 try{
  for(const root of [String.raw`C:\project`,String.raw`\\server\share\project`,String.raw`\\?\C:\project`,String.raw`\\?\UNC\server\share\project`]){
   const workspace={id:"windows",host_id:"host",title:"Project",root_dir:root};
   const surface:Surface={id:"file:split",kind:"file",title:"split.md",path:"split.md",rootDir:root,workspaceId:workspace.id};
   model.update({workspaces:[workspace],activeWorkspace:workspace,surfaces:[surface]});
   const expected=root+"\\split.md";
   expect(model.hostPath(surface)).toBe(expected);
   let opened:unknown;model.openFile=async(path,root)=>{opened={path,root};};
   await model.openLocation(expected);
   expect(opened).toEqual({path:"split.md",root});
   await expect(model.openLocation(hostPaths(root).resolve(root,"../outside.md"))).rejects.toThrow("outside the current workspace");
  }
  const surface:Surface={id:"posix",kind:"file",title:"literal",path:String.raw`literal\name.md`,rootDir:"/project",workspaceId:"posix"};
  expect(model.hostPath(surface)).toBe(String.raw`/project/literal\name.md`);
 }finally{model.dispose();client.stop();}
});
