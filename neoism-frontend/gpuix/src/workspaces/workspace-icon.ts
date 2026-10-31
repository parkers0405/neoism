import type {AppConfig} from "../config";
import type {Workspace} from "../daemon/protocol";
import {localDaemon} from "../menu-actions/web-frontend";
export const WORKSPACE_CLOUD_ICON="\uf0c2";
/** A cloud means attached remotely, not merely a local workspace that can be shared. */
export function cloudWorkspace(workspace:Workspace|undefined,config:Pick<AppConfig,"endpoint"|"serverId"|"transportIdentity"|"collaborative">,connectedHostId?:string){
 if(!workspace)return false;
 const owner=workspace.running_on_host_id||workspace.host_id;
 const remoteOwner=!!owner&&owner!=="local"&&owner!=="desktop"&&!!connectedHostId&&owner!==connectedHostId;
 return config.collaborative===true||!!config.transportIdentity||!!config.serverId&&config.serverId!=="local"||!localDaemon(config.endpoint)
  ||workspace.host_kind==="tailscale"||workspace.host_kind==="cloud_sandbox"||remoteOwner;
}
export function workspaceIcon(workspace:Workspace|undefined,config:Pick<AppConfig,"endpoint"|"serverId"|"transportIdentity"|"collaborative">,folderGlyph:string,connectedHostId?:string){return cloudWorkspace(workspace,config,connectedHostId)?WORKSPACE_CLOUD_ICON:folderGlyph;}
