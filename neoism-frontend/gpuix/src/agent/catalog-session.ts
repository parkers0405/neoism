import type{Session}from"@neoism/sdk";
import{sdkThread}from"../daemon/sdk";
import{isRecord}from"../daemon/protocol";
/** Keep the canonical catalog fields that the small model Thread projection
 * does not need, so the original sidebar receives busy/pin/time/source data. */
export function catalogThread(session:Session){
 const external=isRecord(session.externalAgent)?session.externalAgent:{};
 const provider=(session.parentId===undefined||session.parentId===null)&&external.runtime==="acp"&&typeof external.provider==="string"&&["opencode","claude","codex"].includes(external.provider)?external.provider:null;
 const activity=typeof session.catalogActivity==="string"&&["idle","running","background","permission"].includes(session.catalogActivity)?session.catalogActivity:null;
 return{...sdkThread(session),updated_at:typeof session.time?.updated==="number"?session.time.updated:0,
  catalog_activity:activity,busy:activity==="running",pinned:session.pinned===true,message_count:0,external_provider:provider};
}
