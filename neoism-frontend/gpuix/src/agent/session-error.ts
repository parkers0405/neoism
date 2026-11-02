import {isRecord} from "../daemon/protocol";

/** Provider errors arrive as strings, v2 errors, or named errors with data. */
export function sessionErrorMessage(properties: unknown): string | undefined {
  const error=isRecord(properties)&&"error" in properties?properties.error:properties;
  const data=isRecord(error)&&isRecord(error.data)?error.data:undefined;
  const message=[typeof error==="string"?error:undefined,isRecord(error)?error.message:undefined,data?.message]
    .find((value):value is string=>typeof value==="string"&&!!value.trim())?.trim();
  if(isRecord(error)&&(error.interrupted===true||error.name==="MessageAbortedError")||data?.interrupted===true||message?.toLowerCase()==="session aborted")return;
  return message||"Agent session failed";
}