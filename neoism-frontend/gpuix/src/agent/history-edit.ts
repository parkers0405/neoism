import type {HistoryMessage} from "../daemon/protocol";

export type HistoryEdit = "undo" | "redo";
export interface EditableHistory { messages: readonly HistoryMessage[]; redo: readonly (readonly HistoryMessage[])[] }

/** Mirror the server's user-turn boundary, retaining removed rows for instant redo. */
export function stepHistoryEdit(state: EditableHistory, action: HistoryEdit): EditableHistory {
 if(action==="redo"){
  const restored=state.redo.at(-1);
  return restored?{messages:[...state.messages,...restored],redo:state.redo.slice(0,-1)}:state;
 }
 const index=state.messages.findLastIndex(message=>message.kind==="User");
 return index<0?state:{messages:state.messages.slice(0,index),redo:[...state.redo,state.messages.slice(index)]};
}