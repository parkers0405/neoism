import {expect,test} from "bun:test";
import {cloudWorkspace,workspaceIcon,WORKSPACE_CLOUD_ICON} from "../src/workspaces/workspace-icon";
const local={endpoint:"ws+unix:///tmp/fixture.sock:/session",serverId:"local",collaborative:false};
const workspace={id:"a",host_id:"machine-a",title:"cloud ssh remote shared",root_dir:"/work/remote"};
test("workspace icons distinguish local, joined/SSH and hosted/shared identity without guessing from labels",()=>{
 expect(cloudWorkspace(workspace,local,"machine-a")).toBe(false);expect(workspaceIcon(workspace,local,"folder","machine-a")).toBe("folder");
 for(const config of [{...local,collaborative:true},{...local,transportIdentity:"ssh:stable"},{...local,serverId:"saved-server"},{...local,endpoint:"wss://host.invalid/session"}])expect(workspaceIcon(workspace,config,"folder","machine-a")).toBe(WORKSPACE_CLOUD_ICON);
 for(const metadata of [{visibility:"shared"},{visibility:"team"},{host_kind:"tailscale"},{host_kind:"docker_sandbox"},{host_kind:"cloud_sandbox"},{running_on_host_id:"machine-b"},{host_id:"machine-b"}])expect(workspaceIcon({...workspace,...metadata},local,"folder","machine-a")).toBe(WORKSPACE_CLOUD_ICON);
 expect(cloudWorkspace({...workspace,host_id:"machine-b",running_on_host_id:"machine-a"},local,"machine-a")).toBe(false);expect(cloudWorkspace(undefined,local)).toBe(false);
});
