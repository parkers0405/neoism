import{isRecord}from"../daemon/protocol";
/** Same event projection as daemon agent/events::forward_catalog_event. The SDK
 * owns SSE parsing/replay/dedupe; hosts receive the unchanged canonical messages. */
export function catalogAgentMessage(event:unknown):unknown|undefined{
 if(!isRecord(event)||typeof event.type!=="string")return;
 const data=isRecord(event.data)?event.data:isRecord(event.properties)?event.properties:{};
 if(event.type==="session.catalog.family"&&typeof data.sessionID==="string"&&isRecord(data.snapshot))return{CatalogFamily:{session_id:data.sessionID,snapshot:data.snapshot}};
 if(event.type==="session.catalog.activity"&&typeof data.sessionID==="string"&&typeof data.activity==="string"&&["idle","running","background","permission"].includes(data.activity))return{CatalogActivity:{session_id:data.sessionID,activity:data.activity}};
 if(["session.created","session.updated","session.deleted"].includes(event.type)){
  const info=isRecord(data.info)?data.info:{},id=typeof data.sessionID==="string"?data.sessionID:typeof info.id==="string"?info.id:undefined;if(!id)return;
  return event.type==="session.deleted"?{ThreadDeleted:{session_id:id}}:{ThreadUpdated:{session_id:id,title:typeof info.title==="string"?info.title:null,pinned:typeof info.pinned==="boolean"?info.pinned:null}};
 }
}
