import {hostname} from "node:os";
import type {AppConfig} from "../config";
import {coreCall} from "../native-core";

export interface LocalPresenceIdentity {peerId:string;displayName:string;color:[number,number,number]}
/** Same device identity and shared display-name policy as desktop
 * screen/presence.rs. The remote daemon's hostname is not our participant. */
export function localPresenceIdentity(config:Pick<AppConfig,"presenceDisplayName">,env:NodeJS.ProcessEnv=process.env,host=hostname()):LocalPresenceIdentity{
 return coreCall<LocalPresenceIdentity>("agenttimeline.identity",{host,user:env.USER??env.USERNAME??"user",envName:env.NEOISM_DISPLAY_NAME,configName:config.presenceDisplayName});
}
