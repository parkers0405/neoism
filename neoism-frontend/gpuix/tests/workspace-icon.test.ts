import {expect,test} from "bun:test";
import {cloudWorkspace,workspaceIcon,WORKSPACE_CLOUD_ICON} from "../src/workspaces/workspace-icon";
const local={endpoint:"ws+unix:///tmp/fixture.sock:/session",serverId:"local",collaborative:false};
const workspace={id:"a",host_id:"machine-a",title:"cloud ssh remote shared",root_dir:"/work/remote"};
test("workspace icons distinguish local, joined/SSH and hosted identity without guessing from labels",()=>{
 expect(cloudWorkspace(workspace,local,"machine-a")).toBe(false);expect(workspaceIcon(workspace,local,"folder","machine-a")).toBe("folder");
 for(const config of [{...local,collaborative:true},{...local,transportIdentity:"ssh:stable"},{...local,serverId:"saved-server"},{...local,endpoint:"wss://host.invalid/session"}])expect(workspaceIcon(workspace,config,"folder","machine-a")).toBe(WORKSPACE_CLOUD_ICON);
 for(const metadata of [{host_kind:"tailscale"},{host_kind:"cloud_sandbox"},{running_on_host_id:"machine-b"},{host_id:"machine-b"}])expect(workspaceIcon({...workspace,...metadata},local,"folder","machine-a")).toBe(WORKSPACE_CLOUD_ICON);
 expect(cloudWorkspace({...workspace,host_id:"machine-b",running_on_host_id:"machine-a"},local,"machine-a")).toBe(false);expect(cloudWorkspace(undefined,local)).toBe(false);
});

test("local Home stays local when shared, sandboxed locally, or restored with a legacy host alias",()=>{
 for(const endpoint of ["ws://127.0.0.1:7878/session","ws://localhost:7878/session","ws://[::1]:7878/session","ws+unix:///var/folders/neoism.sock:/session"]){
  for(const metadata of [{visibility:"shared"},{visibility:"team"},{host_kind:"docker_sandbox"},{host_id:"local"},{host_id:"desktop"},{host_id:""}]){
   expect(workspaceIcon({...workspace,title:"Home",...metadata},{...local,endpoint},"folder","machine-a")).toBe("folder");
  }
 }
});
